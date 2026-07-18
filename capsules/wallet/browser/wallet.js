import { createWalletActivity } from "./wallet-activity.js?v=wallet-20260719w";
import {
  createWalletApi,
  readHomeOrigin,
  readLaunchToken,
  readQueryParam,
} from "./wallet-api.js?v=wallet-20260719w";
import { createWalletAccountActions } from "./wallet-account-actions.js?v=wallet-20260719w";
import {
  BALANCE_NETWORKS,
  MANAGED_CHAIN_NAMESPACES,
  METHOD_LABELS,
  METHOD_MONOGRAMS,
  accountDisplayBalance,
  accountName,
  chainLabel,
  delta24h,
  formatAmount,
  formatMoney,
  isPasskeyManagedAccount,
  methodForAccount,
  nextAccountName,
  readText,
  shortAddress,
  validateAddress,
} from "./wallet-format.js?v=wallet-20260719w";
import { createWalletFlows } from "./wallet-flows.js?v=wallet-20260719w";
import { createWalletCreateAccountFlow } from "./wallet-create-account-flow.js?v=wallet-20260719w";
import { createWalletReceiveFlow } from "./wallet-receive-flow.js?v=wallet-20260719w";
import { createWalletRequests } from "./wallet-requests.js?v=wallet-20260719w";
import { createWalletSendFlow } from "./wallet-send-flow.js?v=wallet-20260719w";
import { createWalletStateLoader } from "./wallet-state.js?v=wallet-20260719w";
import { createWalletPreferences } from "./wallet-preferences.js?v=wallet-20260719w";
import {
  accountCard,
  copyButton,
  createWalletRender,
  emptyHero,
  methodMark,
  setBusy,
  textNode,
} from "./wallet-render.js?v=wallet-20260719w";

const statusNode = document.querySelector("#wallet-status");
const homeParentOrigin = readHomeOrigin();
const accountsNode = document.querySelector("#wallet-accounts");
const requestsNode = document.querySelector("#wallet-hero-pending");
const stateNode = document.querySelector("#wallet-account-state");
const accountActionsNode = document.querySelector(".wallet-section-actions");
const balanceStateNode = document.querySelector("#wallet-balance-state");
const totalBalanceNode = document.querySelector("#wallet-total-balance");
const deltaNode = document.querySelector("#wallet-delta");
const deltaValueNode = document.querySelector("#wallet-delta-value");
const sendButton = document.querySelector("#wallet-send");
const receiveButton = document.querySelector("#wallet-receive");
const getStartedNode = document.querySelector("#wallet-get-started");
const connectCtaButton = document.querySelector("#wallet-connect-cta");
const signersSection = document.querySelector("#wallet-signers");
const accountDetailNode = document.querySelector("#wallet-account-detail");
const heroNode = document.querySelector(".wallet-hero");
const heroBackNode = document.querySelector("#wallet-hero-back");
const accountsSectionNode = document.querySelector(".wallet-accounts-section");
const accountsBackNode = document.querySelector("#wallet-accounts-back");
const modalBackdropNode = document.querySelector("#wallet-modal-backdrop");
const modalNode = document.querySelector("#wallet-modal");
const activityNode = document.querySelector("#wallet-activity");

let activeHomeToken = readLaunchToken();
if (activeHomeToken && homeParentOrigin && window.top !== window) {
  window.top.postMessage({ type: "home:app-ready", homeToken: activeHomeToken }, homeParentOrigin);
}
let currentAccounts = [];
let currentDefaults = [];
let currentBalanceRows = [];
let currentPrices = {};
let currentApprovalMethods = {};
let pricesStale = false;
let pricesUnavailable = false;
let currentRequests = [];
let selectedAccountId = "";
let reviewWalletRequestId = readQueryParam("wallet_request");
let refreshWalletStateInFlight = null;
const { fetchJson, notifyHomeSummaryChanged, requestFreshPasskeyHomeToken, shellHeaders } = createWalletApi({
  getHomeToken: () => activeHomeToken,
});
const { showStatus } = createWalletRender({ statusNode });
const {
  closeModal,
  flowHost,
  flowRow,
  flowStaticRow,
  modalButton,
  openFlowModal,
  openInfoModal,
} = createWalletFlows({
  modalNode,
  modalBackdropNode,
  heroNode,
  heroBackNode,
  accountsSectionNode,
  accountsBackNode,
  showStatus,
});
const { loadBalanceRows, loadPrices } = createWalletStateLoader({ fetchJson, shellHeaders });
const {
  applyCurrencySelection,
  applyPrivacyState,
  bindPreferenceEvents,
  closeDrawers,
  getDisplayCurrency,
  getPrivacyMode,
  openApprovalMethod,
  renderMethods,
} = createWalletPreferences({
  closeModal,
  fetchJson,
  getHomeToken: () => activeHomeToken,
  modalButton,
  notifyHomeSummaryChanged,
  openFlowModal,
  renderAll,
  requestFreshPasskeyHomeToken,
  refreshWalletState,
  shellHeaders,
  showStatus,
});
const {
  onRequestClick,
  pendingWalletRequests,
  renderRequests,
} = createWalletRequests({
  fetchJson,
  notifyHomeSummaryChanged,
  openApprovalMethod,
  requestFreshPasskeyHomeToken,
  refreshWalletState,
  requestsNode,
  shellHeaders,
  showStatus,
});
const { renderActivity } = createWalletActivity({
  activityNode,
  textNode,
});
const {
  openReceiveFlow,
  renderReceiveAddress,
} = createWalletReceiveFlow({
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowRow,
  modalButton,
  openFlowModal,
  selectedOrDefaultAccount,
  shellHeaders,
  textNode,
});
const { onCreateManagedWallet, openCreateAccountFlow, openImportRecoveryKeyFlow } = createWalletCreateAccountFlow({
  MANAGED_CHAIN_NAMESPACES,
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowHost,
  flowRow,
  modalButton,
  nextAccountName,
  notifyHomeSummaryChanged,
  openFlowModal,
  readText,
  refreshWalletState,
  requestFreshPasskeyHomeToken,
  setBusy,
  shellHeaders,
  showStatus,
});
const { canSendFromAccount, openSendFlow } = createWalletSendFlow({
  METHOD_LABELS,
  accountDisplayBalance,
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowRow,
  flowStaticRow,
  formatAmount,
  getCurrentPrices: () => currentPrices,
  getCurrentRequests: () => currentRequests,
  getDisplayCurrency,
  isPasskeyManagedAccount,
  modalButton,
  notifyHomeSummaryChanged,
  openFlowModal,
  readText,
  refreshWalletState,
  renderActivity,
  requestFreshPasskeyHomeToken,
  selectedOrDefaultAccount,
  setBusy,
  setCurrentRequests: (requests) => {
    currentRequests = requests;
  },
  shellHeaders,
  shortAddress,
  showStatus,
  textNode,
  validateAddress,
});
const { onAccountClick, onDocumentClick } = createWalletAccountActions({
  buildViewAccounts,
  clearAccountSelection,
  closeModal,
  copyText,
  fetchJson,
  flowHost,
  flowRow,
  flowStaticRow,
  getSelectedAccountId: () => selectedAccountId,
  modalButton,
  notifyHomeSummaryChanged,
  openAccountDetail,
  openApprovalMethod,
  openFlowModal,
  openInfoModal,
  refreshWalletState,
  renderReceiveAddress,
  requestFreshPasskeyHomeToken,
  shellHeaders,
  showStatus,
});

boot();

function boot() {
  applyCurrencySelection();
  applyPrivacyState();
  accountsNode?.addEventListener("click", onAccountClick);
  requestsNode?.addEventListener("click", onRequestClick);
  document.addEventListener("click", onWalletActionClick);
  sendButton?.addEventListener("click", openSendFlow);
  receiveButton?.addEventListener("click", openReceiveFlow);
  connectCtaButton?.addEventListener("click", focusSignersSection);
  modalBackdropNode?.addEventListener("click", closeModal);
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeModal();
      closeDrawers();
      clearAccountSelection();
    }
  });
  bindPreferenceEvents();
  window.addEventListener("message", onRuntimeEvents);
  window.addEventListener("message", onWalletRefreshMessage);
  window.addEventListener("message", onShellMenuCommand);
  announceShellMenuManifest();
  refreshWalletState().catch((error) => showStatus(String(error.message || error), "error"));
}

// Shell menu bar: declare File/Account menus to Home; commands come back as
// elastos:menu-command and route to the same flows the buttons open. Every
// entry still ends at the same passkey/approval gates — menus add no authority.
function announceShellMenuManifest() {
  if (!activeHomeToken || !homeParentOrigin || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:menu-manifest",
    homeToken: activeHomeToken,
    menus: [
      {
        title: "File",
        items: [
          { label: "Send...", cmd: "send" },
          { label: "Receive...", cmd: "receive" },
          "-",
          { label: "Refresh", cmd: "refresh" },
          "-",
          { label: "Close Window", cmd: "__close-window" },
        ],
      },
      {
        title: "Account",
        items: [
          { label: "Create Account...", cmd: "create-account" },
          { label: "Import Recovery Key...", cmd: "import-recovery-key" },
        ],
      },
    ],
  }, homeParentOrigin);
}

function onShellMenuCommand(event) {
  if (event.origin !== homeParentOrigin || event.source !== window.top) {
    return;
  }
  const message = event.data;
  if (message?.type !== "elastos:menu-command" || typeof message.cmd !== "string") {
    return;
  }
  switch (message.cmd) {
    case "send":
      openSendFlow();
      return;
    case "receive":
      openReceiveFlow();
      return;
    case "refresh":
      refreshWalletState().catch((error) => showStatus(String(error.message || error), "error"));
      return;
    case "create-account":
      openCreateAccountFlow();
      return;
    case "import-recovery-key":
      openImportRecoveryKeyFlow();
      return;
    default:
  }
}

function onRuntimeEvents(event) {
  if (event.origin !== homeParentOrigin || event.source !== window.top) {
    return;
  }
  const message = event.data || {};
  if (message.type !== "elastos:runtime-events" || !Array.isArray(message.events)) {
    return;
  }
  if (message.events.some(walletRuntimeEventIsRelevant)) {
    refreshWalletState().catch((error) =>
      showStatus(String(error.message || error), "error"),
    );
  }
}

/* Shell pokes this after a connector ceremony succeeds. home:refresh-summary
   only updates Home chrome — it does not reload Wallet accounts by itself. */
function onWalletRefreshMessage(event) {
  if (event.origin !== window.location.origin) {
    return;
  }
  const message = event.data || {};
  if (message.type !== "elastos:wallet-refresh") {
    return;
  }
  refreshWalletState().catch((error) =>
    showStatus(String(error.message || error), "error"),
  );
}

function walletRuntimeEventIsRelevant(event) {
  const kind = String(event && event.kind || "");
  const scope = String(event && event.scope || "");
  return (
    scope === "wallet" ||
    kind.startsWith("wallet.") ||
    kind === "account.balance.changed"
  );
}

function onWalletActionClick(event) {
  const create = event.target && event.target.closest("[data-wallet-create-account]");
  if (create) {
    onCreateManagedWallet(event);
    return;
  }
  const importKey = event.target && event.target.closest("[data-wallet-import-recovery-key]");
  if (importKey) {
    openImportRecoveryKeyFlow();
  }
}

async function refreshWalletState() {
  if (refreshWalletStateInFlight) {
    return refreshWalletStateInFlight;
  }
  refreshWalletStateInFlight = loadWalletState()
    .finally(() => {
      refreshWalletStateInFlight = null;
    });
  return refreshWalletStateInFlight;
}

async function loadWalletState() {
  if (!activeHomeToken) {
    currentAccounts = [];
    currentDefaults = [];
    currentBalanceRows = [];
    currentPrices = {};
    currentRequests = [];
    renderAll();
    showStatus("Open Wallet from Home.", "error");
    return;
  }
  const [summary, prices] = await Promise.all([
    fetchJson("/api/apps/wallet/wallet/summary", { headers: shellHeaders() }),
    loadPrices(),
  ]);
  const walletAccounts = summary && summary.wallet_accounts;
  const walletApprovals = summary && summary.wallet_approvals;
  currentApprovalMethods = summary && summary.approval_methods
    ? summary.approval_methods
    : {};
  currentAccounts = Array.isArray(walletAccounts && walletAccounts.accounts)
    ? walletAccounts.accounts
    : [];
  currentDefaults = Array.isArray(walletAccounts && walletAccounts.default_accounts)
    ? walletAccounts.default_accounts
    : [];
  currentRequests = Array.isArray(walletApprovals && walletApprovals.approval_requests)
    ? walletApprovals.approval_requests
    : [];
  currentPrices = prices.prices || {};
  pricesStale = Boolean(prices.stale);
  pricesUnavailable = Boolean(prices.unavailable);
  currentBalanceRows = await loadBalanceRows(currentAccounts);
  renderAll();
}

function renderAll() {
  const allAccounts = buildViewAccounts();
  const pending = pendingWalletRequests(currentRequests);
  renderHero(allAccounts);
  renderHeroAccount(allAccounts);
  renderAccounts(allAccounts);
  const focusedRequestVisible = renderRequests(pending, reviewWalletRequestId);
  if (reviewWalletRequestId && focusedRequestVisible) {
    showStatus("Review and approve this request in Wallet.", "muted");
  } else if (reviewWalletRequestId) {
    reviewWalletRequestId = "";
  }
  renderMethods(allAccounts, currentApprovalMethods);
  renderActivity(currentRequests);
  updateFlowButtons(allAccounts);
}

function buildViewAccounts() {
  const groups = new Map();
  currentAccounts.forEach((account, index) => {
    const method = methodForAccount(account);
    const groupKey = accountGroupKey(account, method);
    if (!groups.has(groupKey)) {
      groups.set(groupKey, {
        key: groupKey,
        method,
        records: [],
        firstIndex: index,
      });
    }
    groups.get(groupKey).records.push(account);
  });
  return [...groups.values()].map((group) => viewAccountForGroup(group));
}

function accountGroupKey(account, method) {
  const address = readText(account.address).toLowerCase();
  if (account.chain_namespace?.startsWith("eip155:")) {
    return `${method}:eip155:${address}`;
  }
  return `${method}:${account.chain_namespace}:${address || account.account_id}`;
}

function viewAccountForGroup(group) {
  const primary = primaryAccountRecord(group.records);
  const namespaces = namespacesForAccountGroup(group.records);
  const assets = namespaces
    .map((namespace) => assetForNamespace(group.records, namespace))
    .filter(Boolean);
  const usd = assets.reduce((sum, asset) => sum + asset.usd, 0);
  const amount = assets.reduce((sum, asset) => sum + asset.amount, 0);
  const symbol = assets[0]?.symbol || BALANCE_NETWORKS[primary.chain_namespace]?.symbol || "";
  const balanceAvailable = assets.some((asset) => asset.available);
  const priceAvailable = assets.some((asset) => asset.priceAvailable);
  return {
    ...primary,
    account_id: primary.account_id,
    account_ids: group.records.map((account) => account.account_id),
    account_records: group.records,
    chain_namespaces: namespaces,
    name: accountName(primary, group.firstIndex),
    network: accountNetworkLabel(group.records),
    method: group.method,
    monogram: METHOD_MONOGRAMS[group.method] || "?",
    balanceAvailable,
    priceAvailable,
    symbol,
    amount,
    usd,
    assets: assets.filter((asset) => asset.rawValue > 0n),
  };
}

function primaryAccountRecord(records) {
  const defaultIds = new Set(currentDefaults.map((item) => item.account_id));
  return records.find((account) => defaultIds.has(account.account_id)) || records[0];
}

function namespacesForAccountGroup(records) {
  if (records.some((account) => account.chain_namespace?.startsWith("eip155:"))) {
    return Object.keys(BALANCE_NETWORKS).filter((namespace) => namespace.startsWith("eip155:"));
  }
  return [...new Set(records.map((account) => account.chain_namespace))];
}

function accountNetworkLabel(records) {
  if (records.some((account) => account.chain_namespace?.startsWith("eip155:"))) {
    return "EVM";
  }
  return chainLabel(records[0]?.chain_namespace);
}

function assetForNamespace(records, namespace) {
  const account = records.find((item) => item.chain_namespace === namespace) || records[0];
  const key = balanceKey(namespace, account.address);
  const row = currentBalanceRows.find((item) => {
    if (item.balance_key) {
      return item.balance_key === key;
    }
    const rowAccount = item.account || {};
    return balanceKey(rowAccount.chain_namespace, rowAccount.address) === key;
  });
  const config = BALANCE_NETWORKS[namespace];
  const symbol = row?.symbol || config?.symbol || "";
  if (!symbol) {
    return null;
  }
  const amount = row?.available ? row.amount : 0;
  const usd = amount * (currentPrices[symbol]?.usd || 0);
  return {
    symbol,
    amount,
    usd,
    rawValue: row?.rawValue || 0n,
    available: Boolean(row?.available),
    priceAvailable: Boolean(currentPrices[symbol]),
    chain_namespace: namespace,
    account_id: account.account_id,
    network: chainLabel(namespace),
  };
}

function balanceKey(namespace, address) {
  return `${readText(namespace)}:${readText(address).toLowerCase()}`;
}

function renderHero(accounts) {
  const totalUsd = accounts.reduce((sum, account) => sum + account.usd, 0);
  const pricedAssets = accounts.filter((account) => account.balanceAvailable && account.priceAvailable);
  if (accounts.length === 0) {
    totalBalanceNode.textContent = "—";
    totalBalanceNode.classList.remove("is-loading");
    balanceStateNode.textContent = "No accounts yet.";
    balanceStateNode.hidden = false;
    deltaNode.hidden = true;
    return;
  }
  if (getPrivacyMode()) {
    totalBalanceNode.textContent = "••••••";
    totalBalanceNode.classList.remove("is-loading");
  } else if (pricedAssets.length === 0 && Object.keys(currentPrices).length === 0 && pricesUnavailable) {
    totalBalanceNode.textContent = "$—,———";
    totalBalanceNode.classList.remove("is-loading");
  } else if (pricedAssets.length === 0 && Object.keys(currentPrices).length === 0) {
    totalBalanceNode.textContent = "$—,———";
    totalBalanceNode.classList.add("is-loading");
  } else {
    totalBalanceNode.textContent = formatMoney(totalUsd, getDisplayCurrency(), currentPrices);
    totalBalanceNode.classList.remove("is-loading");
  }
  const delta = delta24h(accounts, currentPrices);
  deltaNode.hidden = pricedAssets.length === 0;
  deltaNode.classList.toggle("is-negative", delta < 0);
  if (deltaValueNode) {
    deltaValueNode.textContent = `${delta >= 0 ? "+" : ""}${delta.toFixed(2)}% · 24h`;
  }
  const missingPrices = accounts.filter((account) => account.balanceAvailable && account.amount > 0 && !account.priceAvailable).length;
  if (missingPrices > 0) {
    balanceStateNode.textContent = `${missingPrices} asset${missingPrices === 1 ? "" : "s"} without price; total may be incomplete.`;
  } else if (pricesUnavailable && Object.keys(currentPrices).length === 0) {
    balanceStateNode.textContent = "No approved price source configured.";
  } else if (pricesStale && Object.keys(currentPrices).length > 0) {
    balanceStateNode.textContent = "Using the latest cached market prices.";
  } else {
    balanceStateNode.textContent = "";
    balanceStateNode.hidden = true;
    return;
  }
  balanceStateNode.hidden = false;
}

function renderHeroAccount(accounts) {
  const account = selectedOrDefaultAccount(accounts);
  accountDetailNode.replaceChildren();
  if (!account) {
    accountDetailNode.hidden = true;
    return;
  }
  // Active account meme on the far right (with 24h delta): name + address + copy.
  // Accounts list owns method, balances, and the full roster.
  accountDetailNode.hidden = false;
  accountDetailNode.setAttribute(
    "aria-label",
    selectedAccountId ? `Selected · ${account.name}` : `Default · ${account.name}`,
  );

  const pill = document.createElement("div");
  pill.className = "wallet-hero-address-pill";
  pill.title = `${account.name} · ${account.address}`;

  const label = textNode("span", account.name, "wallet-hero-address-name");
  const address = textNode("code", shortAddress(account.address), "wallet-hero-address-value");
  address.dataset.walletCopyAddress = account.address;
  const copy = document.createElement("button");
  copy.className = "wallet-copy-icon";
  copy.type = "button";
  copy.setAttribute("aria-label", `Copy ${account.name} address`);
  copy.title = "Copy address";
  copy.dataset.walletCopyAddress = account.address;
  copy.innerHTML =
    '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M3.5 10.5V3.5A1 1 0 0 1 4.5 2.5h7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>';

  pill.append(label, address, copy);
  accountDetailNode.append(pill);
}

function selectedOrDefaultAccount(accounts = buildViewAccounts()) {
  const selected = accounts.find((account) => account.account_id === selectedAccountId);
  if (selected) {
    return selected;
  }
  selectedAccountId = "";
  const defaultAccount = defaultWalletAccount(accounts);
  return defaultAccount || accounts[0] || null;
}

function defaultWalletAccount(accounts) {
  const preferredDefault = latestDefault("transaction_intent") || latestDefault("");
  return preferredDefault
    ? accounts.find((account) => accountMatchesDefault(account, preferredDefault)) || null
    : null;
}

function latestDefault(intent) {
  const defaults = intent
    ? currentDefaults.filter((item) => item.intent === intent)
    : currentDefaults;
  return defaults.reduce((latest, item) => {
    if (!latest || Number(item.set_at || 0) > Number(latest.set_at || 0)) {
      return item;
    }
    return latest;
  }, null);
}

function accountMatchesDefault(account, defaultAccount) {
  if (account.account_id === defaultAccount.account_id) {
    return true;
  }
  return Array.isArray(account.account_ids) && account.account_ids.includes(defaultAccount.account_id);
}

function renderAccounts(accounts) {
  accountsNode.replaceChildren();
  stateNode.textContent = `${accounts.length} account${accounts.length === 1 ? "" : "s"}`;
  // Create / Import stay visible even with accounts — never bury the path.
  if (accountActionsNode) {
    accountActionsNode.hidden = false;
  }
  if (accounts.length === 0) {
    accountsNode.append(emptyHero());
    return;
  }
  const displayedAccountId = selectedOrDefaultAccount(accounts)?.account_id || "";
  for (const account of accounts) {
    accountsNode.append(accountCard(account, displayedAccountId, {
      privacyMode: getPrivacyMode(),
      prices: currentPrices,
      displayCurrency: getDisplayCurrency(),
    }));
  }
}

function updateFlowButtons(accounts) {
  const empty = accounts.length === 0;
  const canSend = accounts.some((account) => canSendFromAccount(account));
  // Keep Send clickable when accounts exist but none are built-in sendable —
  // the hero flip explains MetaMask vs Wallet Send instead of a dead control.
  sendButton.disabled = empty;
  receiveButton.disabled = empty;
  if (sendButton) {
    sendButton.hidden = empty;
    sendButton.title = empty || canSend
      ? "Send"
      : "Wallet Send needs a built-in passkey account";
    sendButton.setAttribute("aria-description", sendButton.title);
  }
  if (receiveButton) {
    receiveButton.hidden = empty;
  }
  if (getStartedNode) {
    getStartedNode.hidden = !empty;
  }
}

function focusSignersSection() {
  if (!signersSection) {
    return;
  }
  signersSection.classList.add("is-highlighted");
  signersSection.scrollIntoView({ behavior: "smooth", block: "nearest" });
  window.setTimeout(() => {
    signersSection.classList.remove("is-highlighted");
  }, 1600);
}

function openAccountDetail(accountId) {
  const account = buildViewAccounts().find((item) => item.account_id === accountId);
  if (!account) {
    return;
  }
  closeDrawers();
  selectedAccountId = selectedAccountId === accountId ? "" : accountId;
  renderAll();
}

function clearAccountSelection() {
  if (!selectedAccountId) {
    return;
  }
  selectedAccountId = "";
  renderAll();
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
