import {
  DISPLAY_CURRENCY_STORAGE_KEY,
  METHOD_MONOGRAMS,
  shortAddress,
  namesForMethod,
  readStoredBoolean,
  readStoredValue,
  storeValue,
} from "./wallet-format.js?v=wallet-20260718b";
import {
  actionButton,
  methodMark,
  textNode,
} from "./wallet-render.js?v=wallet-20260718b";

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
  const privacyButton = document.querySelector("#wallet-privacy");
  const privacySettingsButton = document.querySelector("#wallet-privacy-settings");
  const settingsDrawerNode = document.querySelector("#wallet-settings-drawer");
  const activityDrawerNode = document.querySelector("#wallet-activity-drawer");
  const drawerBackdropNode = document.querySelector("#wallet-drawer-backdrop");
  const activityOpenButton = document.querySelector("#wallet-activity-open");
  const settingsOpenButton = document.querySelector("#wallet-settings-open");
  const methodsNode = document.querySelector("#wallet-methods");

  let privacyMode = readStoredBoolean("wallet.privacy");
  let displayCurrency = readStoredValue(DISPLAY_CURRENCY_STORAGE_KEY, "btc", ["btc", "usd", "ela"]);
  let activeDrawer = "";

  function bindPreferenceEvents() {
    privacyButton?.addEventListener("click", onTogglePrivacy);
    privacySettingsButton?.addEventListener("click", onTogglePrivacy);
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
      { id: "passkey", label: "Built-in accounts", hint: "Passkey-controlled by this Wallet", connected: namesForMethod(accounts, "passkey") },
      { id: "metamask", label: "MetaMask", hint: "External EVM signer", connected: namesForMethod(accounts, "metamask"), target: "wallet-metamask", addLabel: "Add account", openLabel: "Open" },
      { id: "btc", label: "UniSat", hint: "External Bitcoin signer", connected: namesForMethod(accounts, "btc"), target: "wallet-unisat", addLabel: "Connect", openLabel: "Open" },
      {
        id: "wc",
        label: "WalletConnect",
        hint: walletConnectAvailable ? "External WalletConnect signer" : "Pinned WalletConnect config required",
        connected: walletConnectConnected,
        target: walletConnectAvailable ? "wallet-walletconnect" : "",
        addLabel: "Connect",
        openLabel: "Open",
      },
    ].filter((method) => method.id !== "wc" || walletConnectAvailable || method.connected.length > 0);
    for (const method of methods) {
      const row = document.createElement("article");
      row.className = "wallet-method";
      row.append(methodMark(method.id, METHOD_MONOGRAMS[method.id], true));
      const body = document.createElement("div");
      body.append(
        textNode("strong", method.label),
        textNode("span", method.connected.length > 0 ? method.connected.join(" · ") : method.hint),
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
      methodsNode.append(row);
      for (const account of accounts.filter((item) => item.method === method.id)) {
        methodsNode.append(methodAccountRow(account));
      }
    }
  }

  function methodAccountRow(account) {
    const row = document.createElement("article");
    row.className = "wallet-method wallet-method-account";
    const spacer = document.createElement("span");
    spacer.className = "wallet-method-spacer";
    const body = document.createElement("div");
    body.append(
      textNode("strong", account.name),
      textNode("span", `${account.network} · ${shortAddress(account.address)}`),
    );
    row.append(spacer, body);
    const remove = actionButton("Remove", "walletRemoveAccount", account.account_id, true);
    remove.dataset.walletAccountName = account.name;
    row.append(remove);
    return row;
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
      const homeToken = await requestFreshPasskeyHomeToken();
      await fetchJson(`/api/apps/wallet/wallet/accounts/${encodeURIComponent(accountId)}`, {
        method: "DELETE",
        headers: shellHeaders({ "content-type": "application/json" }),
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
    if (privacyButton) {
      privacyButton.textContent = privacyMode ? "◉" : "◌";
      privacyButton.setAttribute("aria-label", label);
      privacyButton.setAttribute("aria-pressed", privacyMode ? "true" : "false");
    }
    if (privacySettingsButton) {
      privacySettingsButton.textContent = label;
    }
  }

  function openApprovalMethod(target) {
    const activeHomeToken = getHomeToken();
    if (!target || !activeHomeToken || window.parent === window) {
      return;
    }
    closeDrawers();
    // Ask Home for the in-rail ceremony sheet — not a second desktop window.
    window.parent.postMessage({
      type: "home:open-target",
      target,
      homeToken: activeHomeToken,
      query: { presentation: "sheet" },
    }, window.location.origin);
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
