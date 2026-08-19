import { createWalletActivity } from "./wallet-activity.js?v=wallet-20260523a";
import {
  createWalletApi,
  readHomeOrigin,
  readLaunchToken,
  readQueryParam,
} from "./wallet-api.js?v=wallet-20260715a";
import { createWalletAccountActions } from "./wallet-account-actions.js?v=wallet-20260523a";
import {
  BALANCE_NETWORKS,
  MANAGED_CHAIN_NAMESPACES,
  METHOD_LABELS,
  METHOD_MONOGRAMS,
  accountDisplayBalance,
  accountName,
  cardAddressDisplay,
  cardNetworkLabel,
  chainLabel,
  delta24h,
  formatAmount,
  formatMoney,
  isEvmChainNamespace,
  isPasskeyManagedAccount,
  methodForAccount,
  nextAccountName,
  readText,
  shortAddress,
  validateAddress,
} from "./wallet-format.js?v=wallet-20260819f";
import { createWalletFlows } from "./wallet-flows.js?v=wallet-20260819c";
import { createWalletCreateAccountFlow } from "./wallet-create-account-flow.js?v=wallet-20260711b";
import { createWalletReceiveFlow } from "./wallet-receive-flow.js?v=wallet-20260523a";
import { createWalletRequests } from "./wallet-requests.js?v=wallet-20260731a";
import { createWalletSendFlow } from "./wallet-send-flow.js?v=wallet-20260711b";
import { createWalletStateLoader } from "./wallet-state.js?v=wallet-20260819f";
import { createWalletPreferences } from "./wallet-preferences.js?v=wallet-20260819d";
import {
  accountCard,
  copyButton,
  copyIconButton,
  createWalletRender,
  emptyHero,
  setBusy,
  textNode,
} from "./wallet-render.js?v=wallet-20260819f";
import {
  createHomeClipboardClient,
} from "/apps/home/home-clipboard-client.js?v=home-20260726a";

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
const accountCardNode = document.querySelector("#wallet-account-card");
const accountCardAddressNode = document.querySelector("#wallet-account-card-address");
const accountCardCopyNode = document.querySelector("#wallet-account-card-copy");
const accountCardNameNode = document.querySelector("#wallet-account-card-name");
const accountCardNetworkNode = document.querySelector("#wallet-account-card-network");
const heroSceneNode = document.querySelector(".wallet-hero-scene");
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
const homeClipboard = createHomeClipboardClient({
  targetId: "wallet",
  homeOrigin: homeParentOrigin,
  homeToken: activeHomeToken,
});
homeClipboard.start();
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
const { fetchJson, notifyHomeSummaryChanged, requestPasskeyStepUp, shellHeaders } = createWalletApi({
  getHomeToken: () => activeHomeToken,
});
const { showStatus } = createWalletRender({ statusNode });
const {
  closeModal,
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
const { renderActivity } = createWalletActivity({ activityNode, textNode });
const { loadBalanceRows, loadPrices } = createWalletStateLoader({ fetchJson, shellHeaders });
const {
  applyCurrencySelection,
  applyPrivacyState,
  bindPreferenceEvents,
  closeDrawers,
  getDisplayCurrency,
  getPrivacyMode,
  openApprovalMethod,
  openDrawer,
  renderMethods,
  togglePrivacy,
} = createWalletPreferences({
  closeModal,
  fetchJson,
  getHomeToken: () => activeHomeToken,
  modalButton,
  notifyHomeSummaryChanged,
  openFlowModal,
  renderAll,
  requestPasskeyStepUp,
  refreshWalletState,
  shellHeaders,
  showStatus,
});
const {
  onRequestClick,
  openPendingReview,
  pendingWalletRequests,
  renderRequests,
} = createWalletRequests({
  fetchJson,
  notifyHomeSummaryChanged,
  openApprovalMethod,
  requestPasskeyStepUp,
  refreshWalletState,
  requestsNode,
  shellHeaders,
  showStatus,
});
const {
  loadAccountQr,
  openReceiveFlow,
  qrForAccount,
  renderReceiveAddress,
} = createWalletReceiveFlow({
  buildViewAccounts,
  closeModal,
  copyButton,
  fetchJson,
  flowRow,
  modalButton,
  onQrReady: () => {},
  openFlowModal,
  openInfoModal,
  selectedOrDefaultAccount,
  shellHeaders,
  textNode,
});
const { onCreateManagedWallet, openCreateAccountFlow, openImportRecoveryKeyFlow } = createWalletCreateAccountFlow({
  MANAGED_CHAIN_NAMESPACES,
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowRow,
  modalButton,
  modalNode,
  nextAccountName,
  notifyHomeSummaryChanged,
  openFlowModal,
  readText,
  refreshWalletState,
  requestPasskeyStepUp,
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
  requestPasskeyStepUp,
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
  flowRow,
  flowStaticRow,
  getSelectedAccountId: () => selectedAccountId,
  modalButton,
  modalNode,
  notifyHomeSummaryChanged,
  openAccountDetail,
  openApprovalMethod,
  openFlowModal,
  openInfoModal,
  refreshWalletState,
  renderReceiveAddress,
  requestPasskeyStepUp,
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
  document.querySelector("#wallet-create-cta")?.addEventListener("click", openCreateAccountFlow);
  modalBackdropNode?.addEventListener("click", closeModal);
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") {
      return;
    }
    const settingsDrawer = document.querySelector("#wallet-settings-drawer");
    const activityDrawer = document.querySelector("#wallet-activity-drawer");
    const hadOverlay = Boolean(
      (modalNode && !modalNode.hidden)
        || (settingsDrawer && !settingsDrawer.hidden)
        || (activityDrawer && !activityDrawer.hidden)
        || heroNode?.classList.contains("is-flipped")
        || accountsSectionNode?.classList.contains("is-flipped"),
    );
    closeModal();
    closeDrawers();
    if (!hadOverlay) {
      clearAccountSelection();
    }
  });
  bindPreferenceEvents();
  document.querySelector("#wallet-activity-open")?.addEventListener("click", openActivityChrome);
  applyRailPresentationChrome();
  window.addEventListener("message", onRuntimeEvents);
  window.addEventListener("message", onWalletRefreshMessage);
  window.addEventListener("message", onShellMenuCommand);
  window.addEventListener("message", onWalletChromeCommand);
  refreshWalletState().catch((error) => showStatus(String(error.message || error), "error"));
}

function applyRailPresentationChrome() {
  const presentation = readQueryParam("presentation") === "rail" ? "rail" : "window";
  document.documentElement.dataset.walletPresentation = presentation;
  if (presentation === "rail") {
    document.querySelector(".wallet-hero-nav")?.setAttribute("hidden", "");
  }
}

function openActivityChrome() {
  const pending = pendingWalletRequests(currentRequests);
  if (pending.length > 0) {
    closeDrawers();
    openPendingReview();
    return;
  }
  openDrawer("activity");
}

function onShellMenuCommand(event) {
  if (event.origin !== "null" || event.source !== window.parent) {
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

function onWalletChromeCommand(event) {
  if (event.origin !== "null" || event.source !== window.parent) {
    return;
  }
  const message = event.data;
  if (message?.type !== "elastos:wallet-chrome-command" || typeof message.cmd !== "string") {
    return;
  }
  switch (message.cmd) {
    case "open-activity":
    case "open-approvals":
      openActivityChrome();
      return;
    case "open-settings":
      openDrawer("settings");
      return;
    case "toggle-privacy":
      togglePrivacy();
      return;
    case "close-overlays":
      closeModal();
      closeDrawers();
      return;
    default:
  }
}

function onWalletRefreshMessage(event) {
  if (event.origin !== "null" || event.source !== window.parent) {
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
  const reviewRequests = reviewWalletRequestId
    ? pending.filter((request) => readText(request.request_id) === reviewWalletRequestId)
    : pending;
  renderHero(allAccounts);
  renderHeroAccount(allAccounts);
  renderAccounts(allAccounts);
  const focusedRequestVisible = renderRequests(reviewRequests, reviewWalletRequestId);
  if (reviewWalletRequestId && focusedRequestVisible) {
    showStatus("Review and approve this request in Wallet.", "muted");
  } else if (reviewWalletRequestId) {
    reviewWalletRequestId = "";
  }
  renderMethods(allAccounts, currentApprovalMethods);
  renderActivity(currentRequests);
  renderApprovalsBadge(pending.length);
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
  if (isEvmChainNamespace(account.chain_namespace)) {
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
  if (records.some((account) => isEvmChainNamespace(account.chain_namespace))) {
    return Object.keys(BALANCE_NETWORKS).filter((namespace) => namespace.startsWith("eip155:"));
  }
  return [...new Set(records.map((account) => account.chain_namespace))];
}

function accountNetworkLabel(records) {
  if (records.some((account) => isEvmChainNamespace(account.chain_namespace))) {
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
  } else if (totalUsd === 0) {
    const funded = accounts.filter((account) => account.balanceAvailable && account.amount > 0);
    const elaAmount = funded
      .filter((account) => account.symbol === "ELA")
      .reduce((sum, account) => sum + account.amount, 0);
    if (elaAmount > 0) {
      totalBalanceNode.textContent = `${formatAmount(elaAmount)} ELA`;
    } else if (funded[0]) {
      totalBalanceNode.textContent = funded[0].symbol
        ? `${formatAmount(funded[0].amount)} ${funded[0].symbol}`
        : formatAmount(funded[0].amount);
    } else {
      totalBalanceNode.textContent = formatMoney(totalUsd, getDisplayCurrency(), currentPrices);
    }
    totalBalanceNode.classList.remove("is-loading");
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
  if (!accountCardNode) {
    return;
  }
  if (!account) {
    accountCardNode.hidden = true;
    heroSceneNode?.classList.remove("has-account-card");
    if (accountCardCopyNode) {
      accountCardCopyNode.replaceChildren();
    }
    return;
  }

  const privacy = getPrivacyMode();
  const addressText = cardAddressDisplay(account.address, { privacy });
  const networkText = cardNetworkLabel(account);
  const selectionLabel = selectedAccountId ? `Selected · ${account.name}` : `Default · ${account.name}`;

  accountCardNode.hidden = false;
  heroSceneNode?.classList.add("has-account-card");
  accountCardNode.classList.toggle("is-selected", Boolean(selectedAccountId));
  accountCardNode.setAttribute("aria-label", `${selectionLabel}. ${networkText}`);
  accountCardNode.title = `${account.name} · ${account.address}`;

  if (accountCardAddressNode) {
    accountCardAddressNode.textContent = addressText;
    accountCardAddressNode.dataset.walletCopyAddress = account.address;
  }
  if (accountCardNameNode) {
    accountCardNameNode.textContent = account.name;
  }
  if (accountCardNetworkNode) {
    accountCardNetworkNode.textContent = networkText;
  }
  if (accountCardCopyNode) {
    accountCardCopyNode.replaceChildren(
      copyIconButton(account.address, `Copy ${account.name} address`),
    );
  }
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

function renderApprovalsBadge(pendingCount) {
  const badge = document.querySelector("#wallet-approvals-badge");
  const button = document.querySelector("#wallet-activity-open");
  const count = Math.max(0, Number(pendingCount) || 0);
  if (badge && button) {
    if (count <= 0) {
      badge.hidden = true;
      badge.textContent = "";
      button.setAttribute("aria-label", "Activity");
    } else {
      badge.hidden = false;
      badge.textContent = count > 9 ? "9+" : String(count);
      button.setAttribute(
        "aria-label",
        count === 1 ? "Activity, 1 pending" : `Activity, ${count} pending`,
      );
    }
  }
  if (window.parent !== window) {
    window.parent.postMessage(
      {
        type: "wallet:pending-count",
        count,
      },
      "*",
    );
  }
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

async function copyText(value, purpose = "wallet.address") {
  const text = readText(value);
  if (!text) {
    throw new Error("Nothing to copy.");
  }
  await homeClipboard.writeText(text, { purpose });
}
