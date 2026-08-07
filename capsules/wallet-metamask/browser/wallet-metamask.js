import {
  createHomeClipboardClient,
} from "/apps/home/home-clipboard-client.js?v=home-20260807a";

const CONNECTOR_ID = "wallet-metamask";
const HOST_EFFECT_TYPE = "home:wallet-connector-effect";
const HOST_EFFECT_SCHEMA = "elastos.home.wallet-connector-effect/v1";
const HOST_RESULT_TYPE = "home:wallet-connector-effect-result";
const HOST_RESULT_SCHEMA = "elastos.home.wallet-connector-effect-result/v1";
const HOST_EFFECT_TIMEOUT_MS = 60_000;
const connectButton = document.querySelector("#wallet-connect");
const statusNode = document.querySelector("#wallet-status");
const stateNode = document.querySelector("#wallet-state");
const accountsNode = document.querySelector("#wallet-accounts");
const requestsNode = document.querySelector("#wallet-requests");
const frameHomeToken = readLaunchToken();
const homeOrigin = readQueryParam("home_origin");
let hostEffectInFlight = false;
let hostRequestSequence = 0;
let refreshInFlight = false;
// How often to pick up newly-queued approvals (e.g. a mint tx just enqueued by the Create
// portal) without the user having to reconnect/reopen the Wallet.
const APPROVAL_POLL_MS = 5000;

const homeClipboard = createHomeClipboardClient({
  targetId: CONNECTOR_ID,
  homeOrigin,
  homeToken: frameHomeToken,
});
homeClipboard.start();

boot();

function boot() {
  connectButton?.addEventListener("click", onConnect);
  accountsNode?.addEventListener("click", onAccountClick);
  requestsNode?.addEventListener("click", onRequestClick);
  setState("0 linked");
  refreshWalletState().catch((error) => {
    showStatus(errorMessage(error), "error");
  });
  startApprovalAutoRefresh();
}

// Poll for newly-queued approvals so a mint/trade tx enqueued elsewhere (e.g. Create) appears
// here on its own. Quiet by design: only when the tab is visible, we have a launch token, and
// no host wallet effect (connect/approve) is already in flight; errors are swallowed (the next
// tick retries).
function startApprovalAutoRefresh() {
  window.setInterval(() => {
    if (!frameHomeToken || hostEffectInFlight || refreshInFlight) {
      return;
    }
    if (typeof document !== "undefined" && document.visibilityState === "hidden") {
      return;
    }
    refreshWalletState().catch(() => {});
  }, APPROVAL_POLL_MS);
}

async function onConnect() {
  setButtonBusy(connectButton, true);
  showStatus("Approve in MetaMask or Brave Wallet.", "muted");
  try {
    await requestHomeWalletEffect({ kind: "link" });
    showStatus("Approval method added.", "success");
    notifyHomeSummaryChanged();
    await refreshWalletState();
  } catch (error) {
    showStatus(errorMessage(error), "error");
  } finally {
    setButtonBusy(connectButton, false);
  }
}

async function refreshWalletState() {
  if (!frameHomeToken) {
    renderAccounts([]);
    renderRequests([]);
    showStatus("Open from Wallet to review approval requests.", "error");
    return;
  }
  if (refreshInFlight) {
    return;
  }
  refreshInFlight = true;
  try {
    const [accountSummary, requestSummary] = await Promise.all([
      fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/accounts`, {
        headers: shellHeaders(),
      }),
      fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals`, {
        headers: shellHeaders(),
      }),
    ]);
    const accounts = Array.isArray(accountSummary?.accounts) ? accountSummary.accounts : [];
    const requests = Array.isArray(requestSummary?.approval_requests)
      ? requestSummary.approval_requests
      : [];
    renderAccounts(accounts);
    renderRequests(requests);
    setState(accounts.length > 0 ? `${accounts.length} linked` : "0 linked");
  } finally {
    refreshInFlight = false;
  }
}

function renderAccounts(accounts) {
  if (!accountsNode) {
    return;
  }
  accountsNode.replaceChildren();
  if (accounts.length === 0) {
    accountsNode.append(emptyNode("No connected accounts."));
    return;
  }
  for (const account of accounts) {
    accountsNode.append(accountCard(account));
  }
}

function accountCard(account) {
  const card = document.createElement("div");
  card.className = "wallet-account";
  const main = document.createElement("div");
  main.className = "wallet-account-main";
  const title = document.createElement("strong");
  title.textContent = `Connected wallet · ${chainLabel(account?.chain_namespace)}`;
  const address = document.createElement("code");
  const addressText = readText(account?.address);
  address.className = "wallet-address";
  address.textContent = addressText || "Unknown address";
  main.append(title, address);
  const copy = document.createElement("button");
  copy.className = "wallet-button wallet-button-secondary wallet-copy-button";
  copy.type = "button";
  copy.textContent = "Copy";
  copy.dataset.walletCopyAddress = addressText;
  copy.disabled = !addressText;
  card.append(main, copy);
  return card;
}

function renderRequests(requests) {
  if (!requestsNode) {
    return;
  }
  requestsNode.replaceChildren();
  const connectorRequests = requests.filter((request) => (
    !isManagedWalletRequest(request) && isMetaMaskSignableRequest(request)
  ));
  if (connectorRequests.length === 0) {
    requestsNode.append(emptyNode("No approval requests."));
    return;
  }
  for (const request of connectorRequests) {
    requestsNode.append(requestCard(request));
  }
}

function requestCard(request) {
  const card = document.createElement("div");
  card.className = "wallet-request";
  const main = document.createElement("div");
  main.className = "wallet-request-main";
  const title = document.createElement("strong");
  title.textContent = walletIntentLabel(request?.intent);
  const meta = document.createElement("span");
  const capsule = readText(request?.capsule_id) || "capsule";
  const address = shortAddress(request?.address);
  meta.textContent = address ? `${capsule} - ${address}` : capsule;
  const reason = document.createElement("small");
  reason.textContent = readText(request?.reason)
    || readText(request?.resource)
    || "Approval requested.";
  main.append(title, meta, reason);
  const sign = document.createElement("button");
  sign.className = "wallet-button";
  sign.type = "button";
  sign.textContent = "Review";
  sign.dataset.walletRequestSign = readText(request?.request_id);
  card.append(main, sign);
  return card;
}

async function onAccountClick(event) {
  const button = event.target?.closest?.("[data-wallet-copy-address]");
  const address = readText(button?.dataset?.walletCopyAddress);
  if (!button || !address) {
    return;
  }
  setButtonBusy(button, true);
  try {
    await copyText(address);
    showStatus("Address copied.", "success");
  } catch (error) {
    showStatus(errorMessage(error), "error");
  } finally {
    setButtonBusy(button, false);
  }
}

async function onRequestClick(event) {
  const button = event.target?.closest?.("[data-wallet-request-sign]");
  const approvalRequestId = readText(button?.dataset?.walletRequestSign);
  if (!button || !approvalRequestId) {
    return;
  }
  setButtonBusy(button, true);
  showStatus("Preparing request.", "muted");
  try {
    const result = await requestHomeWalletEffect({ kind: "approve", approvalRequestId });
    showStatus(
      result.effect === "transaction" ? "Transaction sent." : "Request signed.",
      "success",
    );
    notifyHomeSummaryChanged();
    await refreshWalletState();
  } catch (error) {
    showStatus(errorMessage(error), "error");
  } finally {
    setButtonBusy(button, false);
  }
}

function requestHomeWalletEffect(action) {
  if (!frameHomeToken || !homeOrigin || window.top === window) {
    return Promise.reject(new Error("Open this approval method from Wallet."));
  }
  if (hostEffectInFlight) {
    return Promise.reject(new Error("A wallet request is already active."));
  }
  const requestId = nextHostRequestId();
  hostEffectInFlight = true;
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      finish();
      reject(new Error("Wallet request timed out."));
    }, HOST_EFFECT_TIMEOUT_MS);
    const finish = () => {
      window.clearTimeout(timeout);
      window.removeEventListener("message", onMessage);
      hostEffectInFlight = false;
    };
    const onMessage = (event) => {
      const data = event.data;
      if (
        event.source !== window.top
        || event.origin !== homeOrigin
        || data?.type !== HOST_RESULT_TYPE
        || data.schema !== HOST_RESULT_SCHEMA
        || data.requestId !== requestId
        || data.connectorId !== CONNECTOR_ID
      ) {
        return;
      }
      const hasResult = hasExactKeys(data, [
        "type",
        "schema",
        "requestId",
        "connectorId",
        "result",
      ]);
      const hasError = hasExactKeys(data, [
        "type",
        "schema",
        "requestId",
        "connectorId",
        "error",
      ]);
      if (!hasResult && !hasError) {
        return;
      }
      finish();
      if (hasError) {
        reject(new Error(readText(data.error) || "Wallet request failed."));
        return;
      }
      const expectedResultKeys = action.kind === "link" ? ["status"] : ["status", "effect"];
      if (
        !hasExactKeys(data.result, expectedResultKeys)
        || data.result.status !== (action.kind === "link" ? "linked" : "completed")
        || (
          action.kind === "approve"
          && !["signature", "transaction"].includes(data.result.effect)
        )
      ) {
        reject(new Error("Home returned an invalid wallet result."));
        return;
      }
      resolve(data.result);
    };
    window.addEventListener("message", onMessage);
    window.top.postMessage({
      type: HOST_EFFECT_TYPE,
      schema: HOST_EFFECT_SCHEMA,
      requestId,
      connectorId: CONNECTOR_ID,
      connectorToken: frameHomeToken,
      action,
    }, homeOrigin);
  });
}

function nextHostRequestId() {
  hostRequestSequence += 1;
  const random = typeof window.crypto?.randomUUID === "function"
    ? window.crypto.randomUUID()
    : `${Date.now().toString(36)}-${hostRequestSequence}`;
  return `wallet-effect:${random}`;
}

function walletIntentLabel(intent) {
  switch (readText(intent)) {
    case "auth_challenge": return "Sign in";
    case "browser_personal_sign": return "Sign message";
    case "browser_typed_data_sign": return "Sign typed data";
    case "capability_grant": return "Grant access";
    case "credential": return "Issue credential";
    case "publish_envelope": return "Publish";
    case "transaction_intent": return "Transaction";
    case "revocation": return "Revoke";
    default: return "Wallet request";
  }
}

function chainLabel(value) {
  switch (readText(value)) {
    case "eip155:1": return "Ethereum";
    case "eip155:20": return "Elastos Smart Chain";
    case "eip155:8453": return "Base";
    default: {
      const chainId = readText(value).replace(/^eip155:/, "");
      return chainId ? `EVM ${chainId}` : "EVM";
    }
  }
}

function isManagedWalletRequest(request) {
  return ["managed_evm", "managed_btc_p2wpkh"].includes(readText(request?.proof_type));
}

function isMetaMaskSignableRequest(request) {
  return readText(request?.connector_id) === CONNECTOR_ID
    && readText(request?.intent) !== "bitcoin_bip322_proof"
    && ["siwe", "siwe_erc1271"].includes(readText(request?.proof_type));
}

async function copyText(value) {
  const text = readText(value);
  if (!text) {
    throw new Error("Clipboard is unavailable.");
  }
  await homeClipboard.writeText(text, { purpose: "wallet.address" });
}

async function fetchJson(url, init) {
  const response = await fetch(url, init);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    throw new Error(`request failed: ${response.status}${detail.trim() ? ` ${detail.trim()}` : ""}`);
  }
  return response.json();
}

function shellHeaders(extra = {}) {
  return { ...extra, "x-elastos-home-token": frameHomeToken };
}

function notifyHomeSummaryChanged() {
  if (frameHomeToken && homeOrigin && window.top !== window) {
    window.top.postMessage({ type: "home:refresh-summary", homeToken: frameHomeToken }, homeOrigin);
  }
}

function hasExactKeys(value, expectedKeys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length
    && actual.every((key, index) => key === expected[index]);
}

function readQueryParam(name) {
  return readText(new URLSearchParams(window.location.search).get(name));
}

function readLaunchToken() {
  return readText(
    new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token"),
  );
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function shortAddress(value) {
  const address = readText(value);
  return address.length <= 14 ? address : `${address.slice(0, 6)}...${address.slice(-4)}`;
}

function emptyNode(message) {
  const empty = document.createElement("div");
  empty.className = "wallet-empty";
  empty.textContent = message;
  return empty;
}

function setState(message) {
  if (stateNode) stateNode.textContent = message;
}

function showStatus(message, tone) {
  if (!statusNode) return;
  const text = readText(message);
  statusNode.hidden = !text;
  statusNode.textContent = text;
  statusNode.dataset.tone = tone || "muted";
}

function setButtonBusy(button, busy) {
  if (button) button.disabled = Boolean(busy);
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error || "Wallet request failed.");
}
