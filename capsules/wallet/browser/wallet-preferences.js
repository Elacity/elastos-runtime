import {
  DISPLAY_CURRENCY_STORAGE_KEY,
  METHOD_MONOGRAMS,
  shortAddress,
  namesForMethod,
  readStoredBoolean,
  readStoredValue,
  storeValue,
} from "./wallet-format.js?v=wallet-20260719h";
import {
  actionButton,
  methodMark,
  textNode,
} from "./wallet-render.js?v=wallet-20260719h";

export function createWalletPreferences({
  closeModal,
  fetchJson,
  getHomeToken,
  modalButton,
  notifyHomeSummaryChanged,
  openFlowModal,
  renderAll,
  requestFreshPasskeyHomeToken,
  refreshWalletState,
  shellHeaders,
  showStatus,
}) {
  const homeParentOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";
  const privacyButton = document.querySelector("#wallet-privacy");
  const settingsDrawerNode = document.querySelector("#wallet-settings-drawer");
  const activityDrawerNode = document.querySelector("#wallet-activity-drawer");
  const drawerBackdropNode = document.querySelector("#wallet-drawer-backdrop");
  const activityOpenButton = document.querySelector("#wallet-activity-open");
  const settingsOpenButton = document.querySelector("#wallet-settings-open");
  const methodsNode = document.querySelector("#wallet-methods");

  let privacyMode = readStoredBoolean("wallet.privacy");
  let displayCurrency = readStoredValue(DISPLAY_CURRENCY_STORAGE_KEY, "usd", ["btc", "usd", "ela"]);
  let activeDrawer = "";

  function bindPreferenceEvents() {
    privacyButton?.addEventListener("click", onTogglePrivacy);
    activityOpenButton?.addEventListener("click", () => openDrawer("activity"));
    settingsOpenButton?.addEventListener("click", () => openDrawer("settings"));
    drawerBackdropNode?.addEventListener("click", closeDrawers);
    document.querySelectorAll("[data-wallet-currency]").forEach((button) => {
      button.addEventListener("click", () => setDisplayCurrency(button.dataset.walletCurrency));
    });
    document.querySelectorAll("[data-wallet-close-drawer]").forEach((button) => {
      button.addEventListener("click", closeDrawers);
    });
    methodsNode?.addEventListener("click", onMethodClick);
  }

  function renderMethods(accounts, approvalMethods = {}) {
    methodsNode.replaceChildren();
    const walletConnectConnected = namesForMethod(accounts, "wc");
    const walletConnectAvailable = approvalMethods.walletconnect?.available === true;
    const methods = [
      { id: "passkey", label: "Built-in accounts", hint: "Passkey-controlled · Wallet Send", connected: namesForMethod(accounts, "passkey") },
      // Empty → Connect; already linked → Add account (sheet is link ceremony, not a manage UI).
      { id: "metamask", label: "MetaMask", hint: "Link & approve in MetaMask", connected: namesForMethod(accounts, "metamask"), target: "wallet-metamask", addLabel: "Connect", openLabel: "Add account" },
      { id: "btc", label: "UniSat", hint: "Link & approve in UniSat", connected: namesForMethod(accounts, "btc"), target: "wallet-unisat", addLabel: "Connect", openLabel: "Add account" },
      {
        id: "wc",
        label: "WalletConnect",
        hint: walletConnectAvailable ? "Link & approve via WalletConnect" : "Pinned WalletConnect config required",
        connected: walletConnectConnected,
        target: walletConnectAvailable ? "wallet-walletconnect" : "",
        addLabel: "Connect",
        openLabel: "Add account",
      },
    ].filter((method) => method.id !== "wc" || walletConnectAvailable || method.connected.length > 0);

    // Order: Built-in → linked accounts → connector rows to add more.
    const passkey = methods.find((method) => method.id === "passkey");
    const connectors = methods.filter((method) => method.id !== "passkey");
    const passkeyAccounts = accounts.filter((account) => account.method === "passkey");
    const linkedAccounts = accounts.filter((account) => account.method !== "passkey");

    if (passkey) {
      methodsNode.append(methodRow(passkey));
    }
    for (const account of [...passkeyAccounts, ...linkedAccounts]) {
      methodsNode.append(methodAccountRow(account));
    }
    for (const method of connectors) {
      methodsNode.append(methodRow(method));
    }
  }

  function methodRow(method) {
    const row = document.createElement("article");
    row.className = "wallet-method";
    row.append(methodMark(method.id, METHOD_MONOGRAMS[method.id], true, ""));
    const body = document.createElement("div");
    // Names live on account rows — method row only shows a count when linked.
    const subtitle = method.connected.length > 0
      ? `(${method.connected.length})`
      : method.hint;
    body.append(
      textNode("strong", method.label),
      textNode("span", subtitle),
    );
    row.append(body);
    if (method.target) {
      const label = method.connected.length > 0
        ? method.openLabel || "Open"
        : method.addLabel || "Add";
      row.append(actionButton(label, "walletOpenMethod", method.target, true));
    } else if (method.connected.length > 0 || method.id === "passkey") {
      row.append(textNode("span", "linked", "wallet-method-chip"));
    }
    return row;
  }

  function methodAccountRow(account) {
    const row = document.createElement("article");
    row.className = "wallet-method wallet-method-account";
    // Same mark grammar as method rows — monogram from the account name so
    // linked wallets align with MetaMask/UniSat instead of sitting blank.
    const monogram = accountMonogram(account);
    const body = document.createElement("div");
    body.append(
      textNode("strong", account.name),
      textNode("span", `${account.network} · ${shortAddress(account.address)}`),
    );
    row.append(
      methodMark(account.method || "unknown", monogram, true, account.chain_namespace),
      body,
    );
    const remove = actionButton("Remove", "walletRemoveAccount", account.account_id, true);
    remove.dataset.walletAccountName = account.name;
    row.append(remove);
    return row;
  }

  function accountMonogram(account) {
    const fromName = String(account?.name || "")
      .trim()
      .charAt(0)
      .toUpperCase();
    if (fromName) {
      return fromName;
    }
    return METHOD_MONOGRAMS[account?.method] || "?";
  }

  function onMethodClick(event) {
    const remove = event.target && event.target.closest("[data-wallet-remove-account]");
    if (!remove) {
      return;
    }
    const accountId = readStoredValueFromDataset(remove.dataset.walletRemoveAccount);
    if (!accountId) {
      return;
    }
    const name = readStoredValueFromDataset(remove.dataset.walletAccountName) || "account";
    // In-window confirm sheet (same modal the flows use); the passkey prompt
    // that follows remains the real authority gate.
    openFlowModal(
      `Remove ${name}?`,
      "Built-in accounts require their Wallet recovery key to restore later. Passkey confirmation is required.",
      [],
      [
        modalButton("Cancel", closeModal, true),
        modalButton("Remove", () => removeAccount(remove, accountId, name), false, true),
      ],
    );
  }

  async function removeAccount(remove, accountId, name) {
    closeModal();
    remove.disabled = true;
    try {
      const homeToken = await requestFreshPasskeyHomeToken(
        "wallet.account.delete",
        { account_id: accountId },
      );
      await fetchJson(`/api/apps/wallet/wallet/accounts/${encodeURIComponent(accountId)}`, {
        method: "DELETE",
        headers: shellHeaders({ "content-type": "application/json" }, homeToken),
        body: JSON.stringify({ home_token: homeToken }),
      });
      showStatus(`${name} removed.`, "success");
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      remove.disabled = false;
    }
  }

  function readStoredValueFromDataset(value) {
    return typeof value === "string" ? value.trim() : "";
  }

  function openDrawer(name) {
    activeDrawer = name;
    settingsDrawerNode.hidden = name !== "settings";
    activityDrawerNode.hidden = name !== "activity";
    drawerBackdropNode.hidden = false;
  }

  function closeDrawers() {
    activeDrawer = "";
    settingsDrawerNode.hidden = true;
    activityDrawerNode.hidden = true;
    drawerBackdropNode.hidden = true;
  }

  function onTogglePrivacy() {
    privacyMode = !privacyMode;
    storeValue("wallet.privacy", privacyMode ? "1" : "0");
    applyPrivacyState();
    renderAll();
  }

  function setDisplayCurrency(currency) {
    if (!["usd", "ela", "btc"].includes(currency)) {
      return;
    }
    displayCurrency = currency;
    storeValue(DISPLAY_CURRENCY_STORAGE_KEY, currency);
    applyCurrencySelection();
    renderAll();
  }

  function applyCurrencySelection() {
    document.querySelectorAll("[data-wallet-currency]").forEach((button) => {
      button.classList.toggle("is-active", button.dataset.walletCurrency === displayCurrency);
    });
  }

  function applyPrivacyState() {
    const label = privacyMode ? "Show balances" : "Hide balances";
    const short = privacyMode ? "Show" : "Hide";
    if (privacyButton) {
      privacyButton.textContent = short;
      privacyButton.setAttribute("aria-label", label);
      privacyButton.setAttribute("aria-pressed", privacyMode ? "true" : "false");
      privacyButton.title = label;
      privacyButton.classList.toggle("is-active", privacyMode);
    }
  }

  function openApprovalMethod(target) {
    const activeHomeToken = getHomeToken();
    if (!target || !activeHomeToken || !homeParentOrigin || window.top === window) {
      return;
    }
    closeDrawers();
    // Ask Home for the in-rail ceremony sheet — not a second desktop window.
    // Opaque capsule frames post to window.top (the Home host).
    window.top.postMessage({
      type: "home:open-target",
      target,
      homeToken: activeHomeToken,
      query: { presentation: "sheet" },
    }, homeParentOrigin);
  }

  return {
    applyCurrencySelection,
    applyPrivacyState,
    bindPreferenceEvents,
    closeDrawers,
    getDisplayCurrency: () => displayCurrency,
    getPrivacyMode: () => privacyMode,
    openApprovalMethod,
    openDrawer,
    renderMethods,
  };
}
