const CONNECTOR_ID = "wallet-walletconnect";
const connectButton = document.querySelector("#wallet-connect");
const statusNode = document.querySelector("#wallet-status");
const stateNode = document.querySelector("#wallet-state");
const accountsNode = document.querySelector("#wallet-accounts");
const requestsNode = document.querySelector("#wallet-requests");
const frameHomeToken = readQueryParam("home_token");
const ceremonyMode = readQueryParam("presentation") === "sheet";

let walletProvider = null;

boot();

function boot() {
  applyCeremonyMode();
  if (connectButton) {
    connectButton.addEventListener("click", onConnect);
  }
  if (accountsNode) {
    accountsNode.addEventListener("click", onAccountClick);
  }
  if (requestsNode) {
    requestsNode.addEventListener("click", onRequestClick);
  }
  setState("0 linked");
  if (!ceremonyMode) {
    refreshWalletState().catch((error) => showStatus(String(error.message || error), "error"));
  }
}

function applyCeremonyMode() {
  if (!ceremonyMode) {
    return;
  }
  document.body.dataset.presentation = "sheet";
  const eyebrow = document.querySelector(".eyebrow");
  if (eyebrow) {
    eyebrow.textContent = "Continue with WalletConnect";
  }
  document.querySelectorAll(".wallet-panel").forEach((panel, index) => {
    if (index > 0) {
      panel.hidden = true;
    }
  });
}

async function onConnect() {
  setButtonBusy(connectButton, true);
  showStatus("Opening wallet.", "muted");
  try {
    const config = await connectorConfig();
    const provider = await loadWalletConnectProvider(config);
    walletProvider = provider;
    const { address, chainId } = await connectProvider(provider);
    const challenge = await fetchJson("/api/auth/evm/challenge", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ address, chain_id: chainId }),
    });
    const signer = await currentProviderAddress(provider);
    ensureSameAddress(signer, address, "Wallet account changed before signing.");
    const signature = await signProviderMessage(provider, challenge.message, signer);
    await fetchJson("/api/auth/evm/verify", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ message: challenge.message, signature }),
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

async function connectorConfig() {
  const config = await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/config`, {
    headers: shellHeaders(),
  });
  const walletconnect = config && config.walletconnect;
  if (!walletconnect || !readText(walletconnect.sdk_asset_path) || !readText(walletconnect.project_id)) {
    throw new Error("WalletConnect is not configured.");
  }
  walletconnect.evm_chains = Array.isArray(config && config.evm_chains) ? config.evm_chains : [];
  return walletconnect;
}

async function loadWalletConnectProvider(config) {
  const module = await import(readText(config.sdk_asset_path));
  if (typeof module.connectWalletConnectEvm !== "function") {
    throw new Error("Pinned WalletConnect SDK adapter is missing connectWalletConnectEvm.");
  }
  const provider = await module.connectWalletConnectEvm({
    projectId: readText(config.project_id),
    chains: [1, 20, 8453],
    metadata: {
      name: "ElastOS Home",
      description: "Runtime-mediated WalletConnect proof",
      url: window.location.origin,
      icons: [`${window.location.origin}/apps/home/elastos-home-icon.svg`],
    },
  });
  if (!provider || typeof provider.request !== "function") {
    throw new Error("WalletConnect adapter returned no EIP-1193 provider.");
  }
  return provider;
}

async function connectProvider(provider) {
  const accounts = await provider.request({ method: "eth_requestAccounts" });
  const address = Array.isArray(accounts) && accounts[0] ? String(accounts[0]) : "";
  if (!address) {
    throw new Error("Wallet returned no account.");
  }
  const chainHex = await provider.request({ method: "eth_chainId" });
  const chainId = Number.parseInt(String(chainHex), 16);
  if (!Number.isFinite(chainId) || chainId <= 0) {
    throw new Error("Wallet returned an invalid chain.");
  }
  return { address, chainId };
}

async function currentProviderAddress(provider) {
  const accounts = await provider.request({ method: "eth_accounts" });
  const address = Array.isArray(accounts) && accounts[0] ? String(accounts[0]) : "";
  if (!address) {
    throw new Error("Wallet has no selected account.");
  }
  return address;
}

async function signProviderMessage(provider, message, signer) {
  const signature = await provider.request({
    method: "personal_sign",
    params: [message, signer],
  });
  return readText(signature);
}

async function refreshWalletState() {
  if (!frameHomeToken) {
    renderAccounts([]);
    renderRequests([]);
    showStatus("Open from Wallet to add an approval method.", "error");
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
  setState(`${accounts.length} linked`);
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
  const externalRequests = requests.filter(isWalletConnectSignableRequest);
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
  setButtonBusy(button, true);
  showStatus("Preparing request.", "muted");
  try {
    const provider = walletProvider || await loadWalletConnectProvider(await connectorConfig());
    walletProvider = provider;
    const handoffSummary = await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals/${encodeURIComponent(requestId)}/approve`, {
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
      await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals/${encodeURIComponent(requestId)}/complete`, {
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
      const signature = await signProviderMessage(provider, message, activeSigner);
      await fetchJson(`/api/apps/${CONNECTOR_ID}/wallet/approvals/${encodeURIComponent(requestId)}/complete`, {
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
  const config = await connectorConfig();
  const chains = Array.isArray(config && config.evm_chains) ? config.evm_chains : [];
  return chains.find((chain) => normalizeChainId(chain && chain.chainId) === normalizeChainId(chainId)) || null;
}

function isWalletConnectSignableRequest(request) {
  const connectorId = readText(request && request.connector_id);
  const intent = readText(request && request.intent);
  const proofType = readText(request && request.proof_type);
  return connectorId === CONNECTOR_ID
    && intent !== "bitcoin_bip322_proof"
    && (proofType === "siwe" || proofType === "siwe_erc1271");
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
    case "revocation":
      return "Revoke";
    default:
      return "Wallet request";
  }
}

function chainLabel(value) {
  switch (readText(value)) {
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

function ensureSameAddress(actual, expected, message) {
  if (normalizeAddress(actual) !== normalizeAddress(expected)) {
    throw new Error(message);
  }
}

function normalizeAddress(address) {
  return readText(address).toLowerCase();
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
