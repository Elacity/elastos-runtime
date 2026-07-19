import {
  DISPLAY_CURRENCY_STORAGE_KEY,
  METHOD_MONOGRAMS,
  shortAddress,
  namesForMethod,
  readStoredBoolean,
  readStoredValue,
  storeValue,
} from "./wallet-format.js?v=wallet-20260720j";
import {
  actionButton,
  methodMark,
  textNode,
} from "./wallet-render.js?v=wallet-20260720j";

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
  const settingsOpenButton = document.querySelector("#wallet-settings-open");
  const methodsNode = document.querySelector("#wallet-methods");

  let privacyMode = readStoredBoolean("wallet.privacy");
  let displayCurrency = readStoredValue(DISPLAY_CURRENCY_STORAGE_KEY, "usd", ["btc", "usd", "ela"]);
  let activeDrawer = "";
  let drawerCloseTimer = 0;
  const DRAWER_MS = 320;

  function bindPreferenceEvents() {
    privacyButton?.addEventListener("click", onTogglePrivacy);
    // Activity click is bound in wallet.js — pending focuses hero; else history.
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

  function prefersReducedMotion() {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  function drawerFor(name) {
    if (name === "settings") {
      return settingsDrawerNode;
    }
    if (name === "activity") {
      return activityDrawerNode;
    }
    return null;
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

    // One shared container per signer family: header + its accounts, then
    // the next connector block (MetaMask / UniSat / WC) below.
    const passkey = methods.find((method) => method.id === "passkey");
    const connectors = methods.filter((method) => method.id !== "passkey");
    if (passkey) {
      methodsNode.append(methodGroup(
        passkey,
        accounts.filter((account) => account.method === "passkey"),
      ));
    }
    for (const method of connectors) {
      methodsNode.append(methodGroup(
        method,
        accounts.filter((account) => account.method === method.id),
      ));
    }
  }

  function methodGroup(method, methodAccounts) {
    const group = document.createElement("section");
    group.className = "wallet-method-group";
    group.setAttribute("aria-label", method.label);
    group.append(methodRow(method));
    for (const account of methodAccounts) {
      group.append(methodAccountRow(account));
    }
    return group;
  }

  function methodRow(method) {
    const row = document.createElement("article");
    row.className = "wallet-method wallet-method-head";
    row.append(methodMark(method.id, METHOD_MONOGRAMS[method.id], true, ""));
    const body = document.createElement("div");
    // Built-in shows "Passkey (N)" so the signer kind is visible; connector
    // rows stay "(N)" because the title already names MetaMask / UniSat / WC.
    const subtitle = method.connected.length > 0
      ? method.id === "passkey"
        ? `Passkey (${method.connected.length})`
        : `(${method.connected.length})`
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
    window.clearTimeout(drawerCloseTimer);
    const next = drawerFor(name);
    if (!next) {
      return;
    }
    // Swap drawers without animating the previous closed mid-flight.
    for (const node of [settingsDrawerNode, activityDrawerNode]) {
      if (node && node !== next) {
        node.classList.remove("is-open");
        node.hidden = true;
      }
    }
    activeDrawer = name;
    next.hidden = false;
    if (drawerBackdropNode) {
      drawerBackdropNode.hidden = false;
    }
    // Force layout so the closed transform is painted before we open —
    // otherwise some engines skip the enter transition.
    void next.offsetWidth;
    void drawerBackdropNode?.offsetWidth;
    next.classList.add("is-open");
    drawerBackdropNode?.classList.add("is-open");
  }

  function closeDrawers() {
    window.clearTimeout(drawerCloseTimer);
    activeDrawer = "";
    settingsDrawerNode?.classList.remove("is-open");
    activityDrawerNode?.classList.remove("is-open");
    drawerBackdropNode?.classList.remove("is-open");
    const finish = () => {
      if (settingsDrawerNode) {
        settingsDrawerNode.hidden = true;
      }
      if (activityDrawerNode) {
        activityDrawerNode.hidden = true;
      }
      if (drawerBackdropNode) {
        drawerBackdropNode.hidden = true;
      }
    };
    if (prefersReducedMotion()) {
      finish();
      return;
    }
    drawerCloseTimer = window.setTimeout(finish, DRAWER_MS);
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
    // Window nav: eye + Show/Hide. Rail stays icon-only in Home chrome.
    const eyeSvg = privacyMode
      ? '<svg class="wallet-nav-icon-svg" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.25 2.25l11.5 11.5" /><path d="M6.6 6.7A1.75 1.75 0 0 0 9.3 9.4" /><path d="M4.2 4.55C2.85 5.55 1.75 8 1.75 8s2.5 3.75 6.25 3.75c1.05 0 2-.3 2.8-.75" /><path d="M7.1 4.35c.3-.05.6-.1.9-.1 3.75 0 6.25 3.75 6.25 3.75a12.4 12.4 0 0 1-1.55 1.7" /></svg>'
      : '<svg class="wallet-nav-icon-svg" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M1.75 8s2.5-3.75 6.25-3.75S14.25 8 14.25 8s-2.5 3.75-6.25 3.75S1.75 8 1.75 8Z" /><circle cx="8" cy="8" r="1.75" /></svg>';
    if (privacyButton) {
      privacyButton.innerHTML = `${eyeSvg}<span>${short}</span>`;
      privacyButton.setAttribute("aria-label", label);
      privacyButton.setAttribute("aria-pressed", privacyMode ? "true" : "false");
      privacyButton.title = label;
      privacyButton.classList.toggle("is-active", privacyMode);
    }
    if (window.parent !== window) {
      // Parent is the opaque-sandboxed GUI frame ("null" origin): post with
      // "*" — a boolean chrome hint, no secrets cross the boundary.
      window.parent.postMessage(
        {
          type: "wallet:privacy-state",
          privacyMode,
        },
        "*",
      );
    }
  }

  function togglePrivacy() {
    onTogglePrivacy();
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
    togglePrivacy,
  };
}
