const connectButton = document.querySelector("#wallet-connect");
const popupButton = document.querySelector("#wallet-open-popup");
const statusNode = document.querySelector("#wallet-status");
const stateNode = document.querySelector("#wallet-state");
const accountsNode = document.querySelector("#wallet-accounts");
const requestsNode = document.querySelector("#wallet-requests");
const frameHomeToken = readLaunchToken();
const homeOrigin = readQueryParam("home_origin");
const ceremonyMode = readQueryParam("presentation") === "sheet";
// Top-level popup ceremony: the extension injects into a real top-level page,
// but the gateway fail-closes token'd API calls from a non-opaque origin
// ("home launch token requires an opaque capsule origin"), and a sandboxed
// sheet opening a window that escapes its sandbox gets an implicit noopener —
// no handle in either direction. So the popup relays over a same-origin
// BroadcastChannel to the Home host (both are the gateway's real origin);
// the host forwards to this opaque sheet by launch token. The popup only
// drives the provider; every API call stays in the sheet.
const CONNECTOR_POPUP_CHANNEL = "elastos:connector-popup";
const CONNECTOR_POPUP_RELAY_TYPE = "elastos:connector-popup-relay";
const popupRelayMode = !isEmbeddedFrame() && ceremonyMode && Boolean(frameHomeToken);
const discoveredWalletProviders = [];
// Skip the background poll while the user is mid-connect/mid-sign so we never tear down an
// in-flight approval's button state, and a re-entrancy guard so polls can't overlap.
let interactionBusy = false;
let refreshInFlight = false;
// How often to pick up newly-queued approvals (e.g. a mint tx just enqueued by the Create
// portal) without the user having to reconnect/reopen the Wallet.
const APPROVAL_POLL_MS = 5000;

if (frameHomeToken && homeOrigin && window.top !== window) {
  window.top.postMessage({ type: "home:app-ready", homeToken: frameHomeToken }, homeOrigin);
}

boot();

function boot() {
  applyCeremonyMode();
  configureMetaMaskDiscovery();
  if (ceremonyMode && isEmbeddedFrame()) {
    bindSheetPopupRelay();
  }
  if (connectButton) {
    connectButton.addEventListener("click", onConnect);
  }
  if (popupButton) {
    popupButton.addEventListener("click", openTopLevelConnector);
  }
  if (accountsNode) {
    accountsNode.addEventListener("click", onAccountClick);
  }
  if (requestsNode) {
    requestsNode.addEventListener("click", onRequestClick);
  }
  setState("0 linked");
  if (!ceremonyMode) {
    refreshWalletState().catch((error) => {
      showStatus(String(error.message || error), "error");
    });
    startApprovalAutoRefresh();
  } else {
    // Wallet Connect / Add account already clicked — start MetaMask immediately.
    // Button stays as "Try again" if discovery fails or the user cancels.
    queueCeremonyAutostart();
  }
}

/* Sheet presentation: this capsule is only the EIP-1193 ceremony. Connected
   accounts and approval requests live on the main Wallet surface — hide the
   duplicate chrome so the user is not managing a second product. */
function applyCeremonyMode() {
  if (!ceremonyMode) {
    return;
  }
  document.documentElement.dataset.presentation = "sheet";
  document.body.dataset.presentation = "sheet";
  const eyebrow = document.querySelector(".eyebrow");
  if (eyebrow) {
    eyebrow.textContent = "Continue with MetaMask";
  }
  if (connectButton) {
    connectButton.textContent = "Continue in MetaMask";
  }
  document.querySelectorAll(".wallet-panel").forEach((panel, index) => {
    if (index > 0) {
      panel.hidden = true;
    }
  });
}

function queueCeremonyAutostart() {
  if (!connectButton) {
    return;
  }
  showStatus("Opening MetaMask…", "muted");
  window.requestAnimationFrame(() => {
    onConnect().catch(() => {});
  });
}

function markCeremonyRetry() {
  if (ceremonyMode && connectButton) {
    connectButton.textContent = "Try again";
  }
}

// Poll for newly-queued approvals so a mint/trade tx enqueued elsewhere appears here on its
// own. Quiet by design: only when the tab is visible, we have a launch token, and the user
// isn't mid-interaction; errors are swallowed (the next tick retries).
function startApprovalAutoRefresh() {
  if (typeof window?.setInterval !== "function") {
    return;
  }
  window.setInterval(() => {
    if (!frameHomeToken || interactionBusy || refreshInFlight) {
      return;
    }
    if (typeof document !== "undefined" && document.visibilityState === "hidden") {
      return;
    }
    refreshWalletState().catch(() => {});
  }, APPROVAL_POLL_MS);
}

function configureMetaMaskDiscovery() {
  window.addEventListener("eip6963:announceProvider", (event) => {
    const detail = event && event.detail;
    const provider = detail && detail.provider;
    if (!provider || typeof provider.request !== "function") {
      return;
    }
    if (!discoveredWalletProviders.some((entry) => entry.provider === provider)) {
      discoveredWalletProviders.push({ info: detail.info || {}, provider });
    }
  });
  window.dispatchEvent(new Event("eip6963:requestProvider"));
}

// Re-request EIP-6963 announcements and wait briefly for them to arrive. Resolves as
// soon as a MetaMask provider is seen, or after a short timeout. Makes connect robust
// to a slow injection or a co-installed wallet that grabbed window.ethereum.
function ensureWalletDiscovery(timeoutMs = 400) {
  window.dispatchEvent(new Event("eip6963:requestProvider"));
  return new Promise((resolve) => {
    const start = Date.now();
    const tick = () => {
      const haveMetaMask = discoveredWalletProviders.some(({ info, provider }) => {
        const rdns = readText(info && info.rdns).toLowerCase();
        return rdns.includes("metamask") || Boolean(provider && provider.isMetaMask && !provider.isPhantom);
      });
      if (haveMetaMask || Date.now() - start >= timeoutMs) {
        resolve();
        return;
      }
      setTimeout(tick, 50);
    };
    tick();
  });
}

async function onConnect() {
  // Extension content scripts crash in this opaque-sandboxed sheet, so no
  // usable provider can EVER exist here — don't burn the user's click on
  // discovery/probing (transient activation expires and the browser then
  // blocks the window we know we need). Open the MetaMask companion window
  // synchronously while the click is still live; the ceremony continues
  // there and relays back to this sheet.
  if (ceremonyMode && isEmbeddedFrame()) {
    if (popupButton) {
      popupButton.hidden = false;
    }
    if (navigator.userActivation?.isActive) {
      openTopLevelConnector();
    } else {
      // Ceremony autostart has no gesture — invite the click instead of
      // greeting the user with a blocked-popup error.
      showStatus('MetaMask opens in its own window. Click "Continue in MetaMask".', "muted");
    }
    return;
  }
  // Re-run EIP-6963 discovery and give announcements a tick to arrive, so a slow
  // MetaMask injection doesn't fall through to a window.ethereum a co-installed wallet
  // (e.g. Phantom) has overridden. Discovery is the authoritative, per-wallet source.
  await ensureWalletDiscovery();
  const provider = metaMaskProvider();
  if (!provider) {
    handleMissingProvider();
    markCeremonyRetry();
    return;
  }
  // In the opaque-sandboxed sheet MetaMask's inpage provider announces while
  // its content-script transport is dead — requests then hang forever, never
  // reject. Probe with a promptless call before committing the button to its
  // busy state; a hung probe means "no usable provider here", go top-level.
  if (isEmbeddedFrame() && !(await providerTransportAlive(provider))) {
    handleMissingProvider();
    markCeremonyRetry();
    return;
  }
  setButtonBusy(connectButton, true);
  interactionBusy = true;
  showStatus("In MetaMask, pick the account you want to link, then approve.", "muted");
  try {
    const { address, chainId } = await connectProvider(provider);
    showStatus("Approve the signature in MetaMask.", "muted");
    if (popupRelayMode) {
      // The sheet owns the API ceremony; this popup only signs.
      const challengeMessage = await relayPopupStage("connected", { address, chainId });
      const signer = await currentProviderAddress(provider);
      ensureSameAddress(signer, address, "Wallet account changed before signing.");
      const signature = await provider.request({
        method: "personal_sign",
        params: [challengeMessage, signer],
      });
      await relayPopupStage("signature", { message: challengeMessage, signature });
      showStatus("Connected. You can close this window.", "success");
      window.setTimeout(() => window.close(), 1200);
      return;
    }
    const challenge = await fetchJson("/api/auth/evm/challenge", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ address, chain_id: chainId }),
    });
    const signer = await currentProviderAddress(provider);
    ensureSameAddress(signer, address, "Wallet account changed before signing.");
    const signature = await provider.request({
      method: "personal_sign",
      params: [challenge.message, signer],
    });
    await fetchJson("/api/auth/evm/verify", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ message: challenge.message, signature }),
    });
    showStatus("Connected. Returning to Wallet…", "success");
    notifyHomeSummaryChanged();
    if (!ceremonyMode) {
      await refreshWalletState();
    }
  } catch (error) {
    // MetaMask can inject its inpage provider into the opaque-sandboxed sheet
    // while the content-script transport behind it is dead (the extension's
    // sessionStorage crash). The provider then announces but every request
    // fails "extension not found" — treat that exactly like a missing
    // provider and continue in the top-level popup.
    if (isEmbeddedFrame() && isDeadProviderTransportError(error)) {
      handleMissingProvider();
      markCeremonyRetry();
    } else {
      showStatus(String(error.message || error), "error");
      markCeremonyRetry();
    }
  } finally {
    setButtonBusy(connectButton, false);
    interactionBusy = false;
  }
}

const PROVIDER_PROBE_TIMEOUT_MS = 1500;

async function providerTransportAlive(provider) {
  try {
    await Promise.race([
      provider.request({ method: "eth_chainId" }),
      new Promise((_, reject) => {
        window.setTimeout(
          () => reject(new Error("MetaMask transport probe timed out")),
          PROVIDER_PROBE_TIMEOUT_MS,
        );
      }),
    ]);
    return true;
  } catch (error) {
    return !isDeadProviderTransportError(error) &&
      !String(error?.message || "").includes("probe timed out");
  }
}

function isDeadProviderTransportError(error) {
  const message = String(error?.message || error || "").toLowerCase();
  return (
    message.includes("failed to connect to metamask") ||
    message.includes("extension not found") ||
    message.includes("disconnected from metamask")
  );
}

// Same fallback as the UniSat connector: extension content scripts crash in
// opaque-sandboxed frames (no allow-same-origin), so no provider is ever
// injected in the embedded sheet. A top-level popup of this same route is a
// normal page where MetaMask injects; the ceremony continues there.
function handleMissingProvider() {
  if (isEmbeddedFrame()) {
    if (popupButton) {
      popupButton.hidden = false;
    }
    // Only attempt window.open on a real user gesture — the ceremony
    // autostart has none, so the browser would block it and we'd greet the
    // user with a scary failure for a popup we knew could not open.
    if (navigator.userActivation?.isActive) {
      openTopLevelConnector();
      return;
    }
    showStatus(
      'MetaMask runs in its own window here. Click "Open MetaMask window" to continue.',
      "muted",
    );
    return;
  }
  showStatus(
    "MetaMask not found. If another wallet (e.g. Phantom) is set as your default Ethereum wallet, MetaMask may not announce — disable that wallet's Ethereum default, or unlock MetaMask, and retry.",
    "error",
  );
}

function isEmbeddedFrame() {
  try {
    return window.self !== window.top;
  } catch {
    return true;
  }
}

function openTopLevelConnector() {
  const popup = window.open(window.location.href, "elastos-wallet-metamask", "popup,width=460,height=720");
  if (!popup) {
    showStatus(
      "The browser blocked the MetaMask window. Allow pop-ups for this site (address-bar icon), then try again.",
      "error",
    );
    return;
  }
  showStatus("Continue in the MetaMask window, then approve there.", "muted");
}

/* ---- Popup relay: provider ceremony in the popup, API calls in this opaque
   sheet. A window opened from a sandboxed frame (allow-popups-to-escape-
   sandbox) carries an implicit noopener, and BroadcastChannel is partitioned
   by origin — the opaque sheet and the real-origin popup share nothing. The
   Home host shares the popup's real origin, so the popup rides a same-origin
   BroadcastChannel to the host, and the host forwards to this sheet over its
   token-bound frame bridge. Stage messages carry no launch token — only a
   token tail as correlation id — and no secrets: an address, the SIWE
   challenge text, and its signature. ---- */

const POPUP_RELAY_TIMEOUT_MS = 120_000;

function popupTokenTail() {
  return frameHomeToken.slice(-32);
}

function relayPopupStage(stage, payload) {
  return new Promise((resolve, reject) => {
    const channel = new BroadcastChannel(CONNECTOR_POPUP_CHANNEL);
    const timeout = window.setTimeout(() => {
      channel.close();
      reject(new Error("The Wallet sheet did not answer. Reopen the connector from Wallet."));
    }, POPUP_RELAY_TIMEOUT_MS);
    channel.onmessage = (event) => {
      const message = event.data || {};
      if (
        message.type !== CONNECTOR_POPUP_RELAY_TYPE ||
        message.from !== "sheet" ||
        message.tokenTail !== popupTokenTail()
      ) {
        return;
      }
      window.clearTimeout(timeout);
      channel.close();
      if (message.stage === "error") {
        reject(new Error(readText(message.error) || "The Wallet sheet reported an error."));
        return;
      }
      resolve(readText(message.message));
    };
    channel.postMessage({
      type: CONNECTOR_POPUP_RELAY_TYPE,
      from: "popup",
      tokenTail: popupTokenTail(),
      stage,
      ...payload,
    });
  });
}

function bindSheetPopupRelay() {
  window.addEventListener("message", (event) => {
    // Stage messages are forwarded by the Home host — the top window, real
    // origin. Nothing else may drive the ceremony.
    if (event.source !== window.top || event.origin !== homeOrigin) {
      return;
    }
    const message = event.data || {};
    if (message.type !== CONNECTOR_POPUP_RELAY_TYPE || message.from !== "popup") {
      return;
    }
    handleSheetPopupStage(message).catch((error) => {
      const text = String(error?.message || error);
      showStatus(text, "error");
      markCeremonyRetry();
      postRelayToPopup({ stage: "error", error: text });
    });
  });
}

function postRelayToPopup(payload) {
  if (!frameHomeToken || !homeOrigin || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:connector-popup-relay",
    homeToken: frameHomeToken,
    ...payload,
  }, homeOrigin);
}

async function handleSheetPopupStage(message) {
  if (message.stage === "connected") {
    const address = readText(message.address);
    const chainId = Number(message.chainId);
    if (!address || !Number.isFinite(chainId) || chainId <= 0) {
      throw new Error("MetaMask window returned an invalid account.");
    }
    showStatus("Approve the signature in the MetaMask window.", "muted");
    const challenge = await fetchJson("/api/auth/evm/challenge", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ address, chain_id: chainId }),
    });
    postRelayToPopup({ stage: "challenge", message: challenge.message });
    return;
  }
  if (message.stage === "signature") {
    const challengeMessage = readText(message.message);
    const signature = readText(message.signature);
    if (!challengeMessage || !signature) {
      throw new Error("MetaMask window returned an incomplete signature.");
    }
    await fetchJson("/api/auth/evm/verify", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ message: challengeMessage, signature }),
    });
    postRelayToPopup({ stage: "done" });
    showStatus("Connected. Returning to Wallet…", "success");
    notifyHomeSummaryChanged();
  }
}

async function connectProvider(provider) {
  // MetaMask site permissions are sticky per origin. After Account A is
  // connected, wallet_requestPermissions often only lists A (with an "Edit
  // accounts" affordance) even when the MetaMask chrome is on Account B.
  // Revoke eth_accounts for this origin first so the next permission prompt
  // is a fresh picker — same as a first-time connect. This does NOT remove
  // already-linked ElastOS wallet accounts; only the browser↔MetaMask grant.
  await refreshAccountPermissionPrompt(provider);
  const accounts = await provider.request({ method: "eth_requestAccounts" });
  const address = preferredConnectAddress(provider, accounts);
  if (!address) {
    throw new Error("Wallet returned no account.");
  }
  assertGrantedMatchesChromeSelection(provider, address);
  const chainHex = await provider.request({ method: "eth_chainId" });
  const chainId = Number.parseInt(String(chainHex), 16);
  if (!Number.isFinite(chainId) || chainId <= 0) {
    throw new Error("Wallet returned an invalid chain.");
  }
  return { address, chainId };
}

async function refreshAccountPermissionPrompt(provider) {
  await revokeAccountPermissions(provider);
  await requestAccountPermissions(provider);
}

async function revokeAccountPermissions(provider) {
  try {
    await provider.request({
      method: "wallet_revokePermissions",
      params: [{ eth_accounts: {} }],
    });
  } catch (error) {
    if (isUserRejectedProviderError(error)) {
      throw error instanceof Error ? error : new Error(String(error.message || error));
    }
    // Unsupported revoke: still try requestPermissions below.
  }
}

async function requestAccountPermissions(provider) {
  try {
    await provider.request({
      method: "wallet_requestPermissions",
      params: [{ eth_accounts: {} }],
    });
  } catch (error) {
    if (isUserRejectedProviderError(error)) {
      throw error instanceof Error ? error : new Error(String(error.message || error));
    }
    // Unsupported / unavailable method: fall through to eth_requestAccounts.
    if (isUnsupportedProviderMethodError(error)) {
      return;
    }
    // Other provider quirks — still attempt eth_requestAccounts rather than
    // blocking the whole connect path.
  }
}

function preferredConnectAddress(provider, accounts) {
  const list = Array.isArray(accounts)
    ? accounts.map((item) => String(item || "").trim()).filter(Boolean)
    : [];
  if (list.length === 0) {
    return "";
  }
  // Prefer the chrome-selected account when MetaMask exposes it and it is in
  // the granted set (covers providers that keep a stale accounts[0] order).
  const selected = readText(provider?.selectedAddress);
  if (selected) {
    const match = list.find((item) => normalizeAddress(item) === normalizeAddress(selected));
    if (match) {
      return match;
    }
  }
  return list[0];
}

function assertGrantedMatchesChromeSelection(provider, grantedAddress) {
  const selected = readText(provider?.selectedAddress);
  if (!selected || !grantedAddress) {
    return;
  }
  if (normalizeAddress(selected) === normalizeAddress(grantedAddress)) {
    return;
  }
  throw new Error(
    "MetaMask is on a different account than the one granted to this site. "
      + "In the MetaMask connect dialog, use Edit accounts, select the account "
      + "you are on, then Connect again.",
  );
}

function isUserRejectedProviderError(error) {
  const code = providerErrorCode(error);
  if (code === 4001) {
    return true;
  }
  const message = String(error?.message || error || "").toLowerCase();
  return message.includes("user rejected") || message.includes("user denied") || message.includes("rejected the request");
}

function isUnsupportedProviderMethodError(error) {
  const code = providerErrorCode(error);
  if (code === 4200 || code === -32601) {
    return true;
  }
  const message = String(error?.message || error || "").toLowerCase();
  return (
    message.includes("method not found")
    || message.includes("does not exist")
    || message.includes("not supported")
    || message.includes("unsupported method")
  );
}

function providerErrorCode(error) {
  const direct = error && error.code;
  if (typeof direct === "number") {
    return direct;
  }
  const nested = error && error.error && error.error.code;
  return typeof nested === "number" ? nested : null;
}

async function currentProviderAddress(provider) {
  const accounts = await provider.request({ method: "eth_accounts" });
  const address = preferredConnectAddress(provider, accounts);
  if (!address) {
    throw new Error("Wallet has no selected account.");
  }
  return address;
}

function ensureSameAddress(actual, expected, message) {
  if (normalizeAddress(actual) !== normalizeAddress(expected)) {
    throw new Error(message);
  }
}

function normalizeAddress(address) {
  return readText(address).toLowerCase();
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
      fetchJson("/api/apps/wallet-metamask/wallet/accounts", {
        headers: shellHeaders(),
      }),
      fetchJson("/api/apps/wallet-metamask/wallet/approvals", {
        headers: shellHeaders(),
      }),
    ]);
    const accounts = Array.isArray(accountSummary && accountSummary.accounts)
      ? accountSummary.accounts
      : [];
    const requests = Array.isArray(requestSummary && requestSummary.approval_requests)
      ? requestSummary.approval_requests
      : [];
    renderAccounts(accounts);
    renderRequests(requests);
    if (accounts.length > 0) {
      setState(`${accounts.length} linked`);
    } else {
      setState("0 linked");
    }
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
    const empty = document.createElement("div");
    empty.className = "wallet-empty";
    empty.textContent = "No connected accounts.";
    accountsNode.append(empty);
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
  title.textContent = `Connected wallet · ${chainLabel(account.chain_namespace)}`;

  const address = document.createElement("code");
  const addressText = readText(account.address);
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
  const externalRequests = requests.filter((request) => (
    !isManagedWalletRequest(request) && isMetaMaskSignableRequest(request)
  ));
  if (externalRequests.length === 0) {
    const empty = document.createElement("div");
    empty.className = "wallet-empty";
    empty.textContent = "No approval requests.";
    requestsNode.append(empty);
    return;
  }
  for (const request of externalRequests) {
    requestsNode.append(requestCard(request));
  }
}

function requestCard(request) {
  const card = document.createElement("div");
  card.className = "wallet-request";

  const main = document.createElement("div");
  main.className = "wallet-request-main";

  const title = document.createElement("strong");
  title.textContent = walletIntentLabel(request.intent);

  const meta = document.createElement("span");
  const capsule = readText(request.capsule_id) || "capsule";
  const address = shortAddress(request.address);
  meta.textContent = address ? `${capsule} - ${address}` : capsule;

  const reason = document.createElement("small");
  reason.textContent = readText(request.reason) || readText(request.resource) || "Approval requested.";

  main.append(title, meta, reason);

  const sign = document.createElement("button");
  sign.className = "wallet-button";
  sign.type = "button";
  sign.textContent = "Review";
  sign.dataset.walletRequestSign = readText(request.request_id);

  card.append(main, sign);
  return card;
}

async function onAccountClick(event) {
  const button = event.target && event.target.closest("[data-wallet-copy-address]");
  if (!button) {
    return;
  }
  const address = readText(button.dataset.walletCopyAddress);
  if (!address) {
    return;
  }
  setButtonBusy(button, true);
  try {
    await copyText(address);
    showStatus("Address copied.", "success");
  } catch (error) {
    showStatus(String(error.message || error), "error");
  } finally {
    setButtonBusy(button, false);
  }
}

async function onRequestClick(event) {
  const button = event.target && event.target.closest("[data-wallet-request-sign]");
  if (!button) {
    return;
  }
  const requestId = readText(button.dataset.walletRequestSign);
  if (!requestId) {
    return;
  }
  const provider = metaMaskProvider();
  if (!provider) {
    showStatus("No compatible wallet found.", "error");
    return;
  }
  setButtonBusy(button, true);
  interactionBusy = true;
  showStatus("Preparing request.", "muted");
  try {
    const handoffSummary = await fetchJson(`/api/apps/wallet-metamask/wallet/approvals/${encodeURIComponent(requestId)}/approve`, {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ reason: "Approved in wallet" }),
    });
    const handoff = handoffSummary && handoffSummary.handoff;
    const signer = readText(handoff && handoff.signer);
    const payloadHash = readText(handoff && handoff.payload_hash);
    if (!signer || !payloadHash) {
      throw new Error("Wallet handoff is incomplete.");
    }
    if (readText(handoff.intent) === "transaction_intent") {
      const transaction = handoff.transaction && typeof handoff.transaction === "object"
        ? handoff.transaction
        : null;
      if (!transaction || !readText(transaction.chainId)) {
        throw new Error("Wallet transaction handoff is incomplete.");
      }
      await ensureProviderChain(provider, readText(transaction.chainId));
      const activeSigner = await currentProviderAddress(provider);
      ensureSameAddress(activeSigner, signer, "Switch to the linked account before approving.");
      showStatus("Approve transaction in your wallet.", "muted");
      const transactionHash = await provider.request({
        method: "eth_sendTransaction",
        params: [transaction],
      });
      await fetchJson(`/api/apps/wallet-metamask/wallet/approvals/${encodeURIComponent(requestId)}/complete`, {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ payload_hash: payloadHash, transaction_hash: transactionHash, signer: activeSigner }),
      });
      showStatus("Transaction sent.", "success");
    } else {
      const message = readText(handoff && handoff.message);
      if (!message) {
        throw new Error("Wallet signature handoff is incomplete.");
      }
      const activeSigner = await currentProviderAddress(provider);
      ensureSameAddress(activeSigner, signer, "Switch to the linked account before signing.");
      showStatus("Approve in your wallet.", "muted");
      const signature = await provider.request({
        method: "personal_sign",
        params: [message, activeSigner],
      });
      await fetchJson(`/api/apps/wallet-metamask/wallet/approvals/${encodeURIComponent(requestId)}/complete`, {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ payload_hash: payloadHash, signature, signer: activeSigner }),
      });
      showStatus("Request signed.", "success");
    }
    notifyHomeSummaryChanged();
    await refreshWalletState();
  } catch (error) {
    showStatus(String(error.message || error), "error");
  } finally {
    setButtonBusy(button, false);
    interactionBusy = false;
  }
}

async function ensureProviderChain(provider, chainId) {
  const targetChainId = normalizeChainId(chainId);
  const active = normalizeChainId(await provider.request({ method: "eth_chainId" }));
  if (active === targetChainId) {
    return;
  }
  try {
    await provider.request({
      method: "wallet_switchEthereumChain",
      params: [{ chainId: targetChainId }],
    });
  } catch (error) {
    if (!isUnknownChainError(error)) {
      throw error;
    }
    const chain = await ethereumChainConfig(targetChainId);
    if (!chain) {
      throw error;
    }
    await provider.request({
      method: "wallet_addEthereumChain",
      params: [chain],
    });
  }
  const current = normalizeChainId(await provider.request({ method: "eth_chainId" }));
  if (current !== targetChainId) {
    throw new Error(`Switch to ${chainLabel(`eip155:${Number.parseInt(targetChainId, 16)}`)} before approving.`);
  }
}

function normalizeChainId(value) {
  const text = readText(value).toLowerCase();
  if (/^0x[0-9a-f]+$/.test(text)) {
    return text;
  }
  const number = Number.parseInt(text, 10);
  if (!Number.isFinite(number) || number <= 0) {
    throw new Error("Wallet returned an invalid chain.");
  }
  return `0x${number.toString(16)}`;
}

function isUnknownChainError(error) {
  return Number(error && error.code) === 4902
    || String(error && error.message || "").toLowerCase().includes("unrecognized chain")
    || String(error && error.message || "").toLowerCase().includes("unknown chain");
}

async function ethereumChainConfig(chainId) {
  const config = await fetchJson("/api/apps/wallet-metamask/wallet/config", {
    headers: shellHeaders(),
  });
  const chains = Array.isArray(config && config.evm_chains) ? config.evm_chains : [];
  return chains.find((chain) => normalizeChainId(chain && chain.chainId) === normalizeChainId(chainId)) || null;
}

function metaMaskProvider() {
  const discovered = selectedMetaMaskProvider(discoveredWalletProviders);
  if (discovered) {
    return discovered;
  }
  if (window.ethereum && typeof window.ethereum.request === "function") {
    return selectedMetaMaskProvider(
      Array.isArray(window.ethereum.providers)
        ? window.ethereum.providers.map((provider) => ({ info: {}, provider }))
        : [{ info: {}, provider: window.ethereum }],
    );
  }
  return null;
}

function selectedMetaMaskProvider(entries) {
  const list = Array.isArray(entries) ? entries : [];
  const usable = ({ provider }) =>
    provider && typeof provider.request === "function" && !provider.isPhantom;
  // Prefer the authoritative EIP-6963 identity (rdns), which a wallet cannot spoof as
  // another wallet's. Only then fall back to the isMetaMask flag — and never trust it
  // on a provider that also identifies as Phantom (Phantom sets isMetaMask for compat).
  const byRdns = list.find(
    (entry) => usable(entry) && readText(entry.info && entry.info.rdns).toLowerCase().includes("metamask"),
  );
  if (byRdns) {
    return byRdns.provider;
  }
  const byFlag = list.find(
    (entry) => usable(entry) && Boolean(entry.provider.isMetaMask),
  );
  return byFlag ? byFlag.provider : null;
}

function walletIntentLabel(intent) {
  switch (readText(intent)) {
    case "auth_challenge":
      return "Sign in";
    case "capability_grant":
      return "Grant access";
    case "credential":
      return "Issue credential";
    case "publish_envelope":
      return "Publish";
    case "transaction_intent":
      return "Transaction";
    case "bitcoin_bip322_proof":
      return "Bitcoin approval";
    case "revocation":
      return "Revoke";
    default:
      return "Wallet request";
  }
}

function chainLabel(value) {
  switch (readText(value)) {
    case "bip122:000000000019d6689c085ae165831e93":
      return "Bitcoin";
    case "eip155:1":
      return "Ethereum";
    case "eip155:20":
      return "Elastos Smart Chain";
    case "eip155:8453":
      return "Base";
    default: {
      const chainId = readText(value).replace(/^eip155:/, "");
      return chainId ? `EVM ${chainId}` : "EVM";
    }
  }
}

function isManagedWalletRequest(request) {
  const proofType = readText(request && request.proof_type);
  return proofType === "managed_evm" || proofType === "managed_btc_p2wpkh";
}

function isMetaMaskSignableRequest(request) {
  const connectorId = readText(request && request.connector_id);
  const intent = readText(request && request.intent);
  const proofType = readText(request && request.proof_type);
  return connectorId === "wallet-metamask"
    && intent !== "bitcoin_bip322_proof"
    && (proofType === "siwe" || proofType === "siwe_erc1271");
}

async function copyText(value) {
  const text = readText(value);
  if (!text) {
    throw new Error("Nothing to copy.");
  }
  if (!navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
    throw new Error("Clipboard is unavailable.");
  }
  await navigator.clipboard.writeText(text);
}

async function fetchJson(url, init) {
  const response = await fetch(url, init);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const suffix = detail.trim() ? ` ${detail.trim()}` : ` ${response.statusText}`;
    throw new Error(`request failed: ${response.status}${suffix}`);
  }
  return response.json();
}

function shellHeaders(extra = {}) {
  return {
    ...extra,
    "x-elastos-home-token": frameHomeToken,
  };
}

function notifyHomeSummaryChanged() {
  if (!frameHomeToken || !homeOrigin || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:refresh-summary",
    homeToken: frameHomeToken,
  }, homeOrigin);
}

function readQueryParam(name) {
  const value = new URLSearchParams(window.location.search).get(name);
  return typeof value === "string" ? value.trim() : "";
}

function readLaunchToken() {
  const value = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token");
  return typeof value === "string" ? value.trim() : "";
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function shortAddress(value) {
  const address = readText(value);
  if (address.length <= 14) {
    return address;
  }
  return `${address.slice(0, 6)}...${address.slice(-4)}`;
}

function setState(message) {
  if (stateNode) {
    stateNode.textContent = message;
  }
}

function showStatus(message, tone) {
  if (!statusNode) {
    return;
  }
  const text = readText(message);
  statusNode.hidden = text.length === 0;
  statusNode.textContent = text;
  statusNode.dataset.tone = tone || "muted";
}

function setButtonBusy(button, busy) {
  if (button) {
    button.disabled = Boolean(busy);
  }
}
