const CONNECTOR_ID = "wallet-unisat";
const BITCOIN_CHAIN_NAMESPACE = "bip122:000000000019d6689c085ae165831e93";
const connectButton = document.querySelector("#wallet-connect");
const popupButton = document.querySelector("#wallet-open-popup");
const statusNode = document.querySelector("#wallet-status");
const stateNode = document.querySelector("#wallet-state");
const accountsNode = document.querySelector("#wallet-accounts");
const requestsNode = document.querySelector("#wallet-requests");
const frameHomeToken = readQueryParam("home_token");
const ceremonyMode = readQueryParam("presentation") === "sheet";

boot();

function boot() {
  applyCeremonyMode();
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
  }
}

function applyCeremonyMode() {
  if (!ceremonyMode) {
    return;
  }
  document.body.dataset.presentation = "sheet";
  const eyebrow = document.querySelector(".eyebrow");
  if (eyebrow) {
    eyebrow.textContent = "Continue with UniSat";
  }
  document.querySelectorAll(".wallet-panel").forEach((panel, index) => {
    if (index > 0) {
      panel.hidden = true;
    }
  });
}

async function onConnect() {
  const provider = unisatProvider();
  if (!provider) {
    handleMissingProvider();
    return;
  }
  setButtonBusy(connectButton, true);
  showStatus("Approve in UniSat.", "muted");
  try {
    const account = await connectProvider(provider);
    const challenge = await fetchJson("/api/auth/btc/challenge", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ address: account.address, network: "bitcoin" }),
    });
    const signer = await currentProviderAccount(provider);
    ensureSameAddress(signer.address, account.address, "UniSat account changed before signing.");
    const proof = await signBitcoinProof(provider, account, challenge.message);
    await fetchJson("/api/auth/btc/verify", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({
        message: challenge.message,
        signature: proof.signature,
        signature_type: proof.signatureType,
        public_key: proof.publicKey,
      }),
    });
    showStatus(ceremonyMode ? "Connected. Returning to Wallet…" : "Approval method added.", "success");
    notifyHomeSummaryChanged();
    await refreshWalletState();
  } catch (error) {
    showStatus(String(error.message || error), "error");
  } finally {
    setButtonBusy(connectButton, false);
  }
}

async function connectProvider(provider) {
  const account = accountFromProviderAccounts(await provider.requestAccounts());
  if (!account.address) {
    throw new Error("UniSat returned no account.");
  }
  const network = typeof provider.getNetwork === "function"
    ? String(await provider.getNetwork()).trim()
    : "livenet";
  if (network !== "livenet") {
    throw new Error("Switch UniSat to Bitcoin mainnet.");
  }
  if (!isSupportedBitcoinAddress(account.address)) {
    throw new Error(`Unsupported Bitcoin address type: ${account.type}.`);
  }
  return account;
}

async function currentProviderAccount(provider) {
  const accounts = typeof provider.getAccounts === "function"
    ? await provider.getAccounts()
    : await provider.requestAccounts();
  const account = accountFromProviderAccounts(accounts);
  if (!account.address) {
    throw new Error("UniSat has no selected account.");
  }
  return account;
}

async function signBitcoinProof(provider, account, message) {
  if (account.type === "native-segwit" || account.type === "taproot") {
    return {
      signature: await provider.signMessage(message, "bip322-simple"),
      signatureType: "bip322_simple",
      publicKey: null,
    };
  }
  if (account.type === "nested-segwit" || account.type === "legacy") {
    if (typeof provider.getPublicKey !== "function") {
      throw new Error("UniSat public key API is unavailable for this address type.");
    }
    const publicKey = readText(await provider.getPublicKey());
    if (!publicKey) {
      throw new Error("UniSat returned no public key for the selected Bitcoin account.");
    }
    return {
      signature: await provider.signMessage(message, "ecdsa"),
      signatureType: "bitcoin_signed_message",
      publicKey,
    };
  }
  throw new Error(`Unsupported Bitcoin address type: ${account.type}.`);
}

async function refreshWalletState() {
  if (!frameHomeToken) {
    renderAccounts([]);
    renderRequests([]);
    showStatus("Open from Wallet to review approval requests.", "error");
    return;
  }
  const [accountSummary, requestSummary] = await Promise.all([
    fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/accounts`, {
      headers: shellHeaders(),
    }),
    fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals`, {
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
  setState(accounts.length > 0 ? `${accounts.length} linked` : "0 linked");
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
  title.textContent = "Connected wallet · Bitcoin";

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
  const bitcoinRequests = requests.filter(isUniSatSignableRequest);
  if (bitcoinRequests.length === 0) {
    requestsNode.append(emptyNode("No approval requests."));
    return;
  }
  for (const request of bitcoinRequests) {
    requestsNode.append(requestCard(request));
  }
}

function requestCard(request) {
  const card = document.createElement("div");
  card.className = "wallet-request";

  const main = document.createElement("div");
  main.className = "wallet-request-main";

  const title = document.createElement("strong");
  title.textContent = "Bitcoin approval";

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
  const provider = unisatProvider();
  if (!provider) {
    handleMissingProvider();
    return;
  }
  setButtonBusy(button, true);
  showStatus("Preparing request.", "muted");
  try {
    const handoffSummary = await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals/${encodeURIComponent(requestId)}/approve`, {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ reason: "Approved in UniSat" }),
    });
    const handoff = handoffSummary && handoffSummary.handoff;
    const message = readText(handoff && handoff.message);
    const signer = readText(handoff && handoff.signer);
    const payloadHash = readText(handoff && handoff.payload_hash);
    if (!message || !signer || !payloadHash) {
      throw new Error("Wallet handoff is incomplete.");
    }
    const activeAccount = await currentProviderAccount(provider);
    ensureSameAddress(activeAccount.address, signer, "Switch to the linked Bitcoin account before signing.");
    showStatus("Approve in UniSat.", "muted");
    const proof = await signBitcoinProof(provider, activeAccount, message);
    await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals/${encodeURIComponent(requestId)}/complete`, {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({
        payload_hash: payloadHash,
        signature: proof.signature,
        signature_type: proof.signatureType,
        public_key: proof.publicKey,
        signer: activeAccount.address,
      }),
    });
    showStatus("Request signed.", "success");
    notifyHomeSummaryChanged();
    await refreshWalletState();
  } catch (error) {
    showStatus(String(error.message || error), "error");
  } finally {
    setButtonBusy(button, false);
  }
}

function unisatProvider() {
  const provider = injectedUniSatProvider();
  if (!provider || typeof provider.requestAccounts !== "function") {
    return null;
  }
  if (typeof provider.signMessage !== "function") {
    return null;
  }
  return provider;
}

function injectedUniSatProvider() {
  for (const candidateWindow of providerWindows()) {
    try {
      const provider = candidateWindow && candidateWindow.unisat;
      if (provider) {
        return provider;
      }
    } catch {
      // Cross-origin or sandboxed parent access is not part of the connector contract.
    }
  }
  return null;
}

function providerWindows() {
  const windows = [window];
  try {
    if (window.parent && window.parent !== window) {
      windows.push(window.parent);
    }
  } catch {
    // Ignore inaccessible embedding contexts.
  }
  try {
    if (window.top && !windows.includes(window.top)) {
      windows.push(window.top);
    }
  } catch {
    // Ignore inaccessible top contexts.
  }
  return windows;
}

function handleMissingProvider() {
  if (isEmbeddedFrame()) {
    showStatus("UniSat was not injected into this embedded connector. Open the UniSat window and approve there.", "error");
    if (popupButton) {
      popupButton.hidden = false;
    }
    openTopLevelConnector();
    return;
  }
  showStatus("UniSat extension not found in this browser profile.", "error");
}

function isEmbeddedFrame() {
  try {
    return window.self !== window.top;
  } catch {
    return true;
  }
}

function openTopLevelConnector() {
  const popup = window.open(window.location.href, "elastos-wallet-unisat", "popup,width=460,height=720");
  if (!popup) {
    showStatus("Open this connector in a top-level browser window so UniSat can inject.", "error");
  }
}

function ensureSameAddress(actual, expected, message) {
  if (normalizeAddress(actual) !== normalizeAddress(expected)) {
    throw new Error(message);
  }
}

function normalizeAddress(address) {
  return readText(address).toLowerCase();
}

function isSupportedBitcoinAddress(address) {
  return bitcoinAddressType(address) !== "unsupported";
}

function accountFromProviderAccounts(accounts) {
  const value = Array.isArray(accounts) ? accounts[0] : accounts;
  const address = providerAccountAddress(value);
  return {
    address,
    type: bitcoinAddressType(address),
  };
}

function providerAccountAddress(value) {
  if (typeof value === "string") {
    return value.trim();
  }
  if (!value || typeof value !== "object") {
    return "";
  }
  for (const key of ["address", "walletAddress", "account", "value"]) {
    if (typeof value[key] === "string" && value[key].trim()) {
      return value[key].trim();
    }
  }
  return "";
}

function bitcoinAddressType(address) {
  const normalized = normalizeAddress(address);
  if (normalized.startsWith("bc1q")) {
    return "native-segwit";
  }
  if (normalized.startsWith("bc1p")) {
    return "taproot";
  }
  if (normalized.startsWith("3")) {
    return "nested-segwit";
  }
  if (normalized.startsWith("1")) {
    return "legacy";
  }
  return "unsupported";
}

function isUniSatSignableRequest(request) {
  return readText(request && request.connector_id) === CONNECTOR_ID
    && readText(request && request.intent) === "bitcoin_bip322_proof"
    && ["bip322_simple", "bitcoin_signed_message"].includes(readText(request && request.proof_type))
    && readText(request && request.chain_namespace) === BITCOIN_CHAIN_NAMESPACE;
}

function setState(value) {
  if (stateNode) {
    stateNode.textContent = value;
  }
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
  if (!frameHomeToken || window.parent === window) {
    return;
  }
  window.parent.postMessage({
    type: "home:refresh-summary",
    homeToken: frameHomeToken,
  }, window.location.origin);
}

function readQueryParam(name) {
  const value = new URLSearchParams(window.location.search).get(name);
  return typeof value === "string" ? value.trim() : "";
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function shortAddress(value) {
  const address = readText(value);
  if (address.length <= 18) {
    return address || "account";
  }
  return `${address.slice(0, 8)}...${address.slice(-6)}`;
}

function emptyNode(message) {
  const empty = document.createElement("div");
  empty.className = "wallet-empty";
  empty.textContent = message;
  return empty;
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
