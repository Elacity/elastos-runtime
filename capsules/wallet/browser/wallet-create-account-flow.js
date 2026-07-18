export function createWalletCreateAccountFlow({
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
}) {
  const accountsSurface = { surface: "accounts" };
  const evmChainNamespaces = MANAGED_CHAIN_NAMESPACES.filter((namespace) =>
    namespace.startsWith("eip155:"),
  );
  const bitcoinChainNamespace = MANAGED_CHAIN_NAMESPACES.find((namespace) =>
    namespace.startsWith("bip122:"),
  );

  async function onCreateManagedWallet(event) {
    event.preventDefault();
    openCreateAccountFlow();
  }

  function primaryActionButton() {
    const host = typeof flowHost === "function" ? flowHost() : null;
    return host?.querySelector(
      ".wallet-modal-actions .wallet-button:not(.wallet-button-secondary)",
    ) || null;
  }

  function openCreateAccountFlow() {
    const rows = [];
    if (evmChainNamespaces.length > 0) {
      rows.push(
        flowRow(
          "EVM",
          "One passkey-controlled account for ESC, Base, and supported EVM networks.",
          () => openCreateAccountTypeForm("evm"),
        ),
      );
    }
    if (bitcoinChainNamespace) {
      rows.push(
        flowRow(
          "Bitcoin",
          "Native Bitcoin account controlled by this passkey.",
          () => openCreateAccountTypeForm("bitcoin"),
        ),
      );
    }
    openFlowModal(
      "Create account",
      "Choose the account type. Chains are provider routes, not separate wallet types.",
      rows,
      [modalButton("Cancel", closeModal, true)],
      accountsSurface,
    );
  }

  function openCreateAccountTypeForm(accountType) {
    const form = document.createElement("form");
    form.className = "wallet-flow-form";
    const defaultLabel = accountType === "bitcoin" ? "Savings" : "EVM Account";
    form.innerHTML = `
      <label>Name <input name="label" autocomplete="off" maxlength="40" placeholder="${defaultLabel}"></label>
    `;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      createManagedAccountFromForm(form, accountType).catch((error) =>
        showStatus(String(error.message || error), "error"),
      );
    });
    openFlowModal(
      "Create account",
      accountType === "bitcoin"
        ? "Create a native Bitcoin account."
        : "Create an EVM account usable through supported EVM chain providers.",
      [form],
      [
        modalButton("Back", openCreateAccountFlow, true),
        modalButton("Create", () => form.requestSubmit()),
      ],
      accountsSurface,
    );
  }

  async function createManagedAccountFromForm(form, accountType) {
    const data = new FormData(form);
    const targetNamespaces =
      accountType === "bitcoin"
        ? [bitcoinChainNamespace].filter(Boolean)
        : evmChainNamespaces.slice(0, 1);
    const firstNamespace = targetNamespaces[0];
    const label =
      readText(data.get("label")) ||
      (accountType === "bitcoin"
        ? nextAccountName(
            firstNamespace,
            buildViewAccounts().map((account) => account.name),
          )
        : "EVM Account");
    if (targetNamespaces.length === 0) {
      showStatus("No supported provider is available for this account type.", "error");
      return;
    }
    const submit = primaryActionButton();
    setBusy(submit, true);
    try {
      for (const [index, chainNamespace] of targetNamespaces.entries()) {
        await fetchJson("/api/apps/wallet/wallet/managed", {
          method: "POST",
          headers: shellHeaders({ "content-type": "application/json" }),
          body: JSON.stringify({
            chain_namespace: chainNamespace,
            label,
            create_new: index === 0,
          }),
        });
      }
      closeModal();
      showStatus(
        accountType === "bitcoin"
          ? `${label} created.`
          : `${label} EVM account created.`,
        "success",
      );
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(submit, false);
    }
  }

  function openImportRecoveryKeyFlow() {
    const form = document.createElement("form");
    form.className = "wallet-flow-form";
    form.innerHTML = `
      <label>Wallet recovery key
        <textarea name="recovery_key" rows="6" spellcheck="false" placeholder='{"schema":"elastos.wallet.recovery-key/v1",...}'></textarea>
      </label>
      <label>Name <input name="label" autocomplete="off" maxlength="40" placeholder="Recovered account"></label>
      <p class="wallet-flow-hint">Paste an individual Wallet recovery key. A full System Recovery Kit can restore Home data and included built-in Wallet accounts from System.</p>
    `;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      importRecoveryKeyFromForm(form).catch((error) =>
        showStatus(String(error.message || error), "error"),
      );
    });
    openFlowModal(
      "Import recovery key",
      "Requires fresh passkey verification.",
      [form],
      [
        modalButton("Cancel", closeModal, true),
        modalButton("Import", () => form.requestSubmit()),
      ],
      accountsSurface,
    );
  }

  async function importRecoveryKeyFromForm(form) {
    const data = new FormData(form);
    const recoveryKeyText = readText(data.get("recovery_key"));
    const label = readText(data.get("label"));
    if (!recoveryKeyText) {
      showStatus("Paste a Wallet recovery key.", "error");
      return;
    }
    let recoveryKey;
    try {
      recoveryKey = JSON.parse(recoveryKeyText);
    } catch {
      showStatus("Recovery key must be valid JSON.", "error");
      return;
    }
    if (!recoveryKey || recoveryKey.schema !== "elastos.wallet.recovery-key/v1") {
      showStatus("Paste an elastos.wallet.recovery-key/v1 account key.", "error");
      return;
    }
    const submit = primaryActionButton();
    setBusy(submit, true);
    try {
      const homeToken = await requestFreshPasskeyHomeToken();
      await fetchJson("/api/apps/wallet/wallet/accounts/import-recovery-key", {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({
          home_token: homeToken,
          recovery_key: recoveryKey,
          label: label || undefined,
        }),
      });
      closeModal();
      showStatus("Wallet recovery key imported.", "success");
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(submit, false);
    }
  }

  return {
    onCreateManagedWallet,
    openCreateAccountFlow,
    openImportRecoveryKeyFlow,
  };
}
