import {
  defaultIntentForAccount,
  defaultIntentLabel,
  isPasskeyManagedAccount,
  readText,
  shortAddress,
} from "./wallet-format.js?v=wallet-20260523a";
import { pulseCopied, setBusy, textNode } from "./wallet-render.js?v=wallet-20260523a";

export function createWalletAccountActions({
  buildViewAccounts,
  clearAccountSelection,
  closeModal,
  copyText,
  fetchJson,
  flowRow,
  flowStaticRow,
  getSelectedAccountId,
  modalButton,
  modalNode,
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
}) {
  async function onAccountClick(event) {
    const menu = event.target && event.target.closest("[data-wallet-account-menu]");
    if (menu) {
      event.stopPropagation();
      openAccountMenu(menu.dataset.walletAccountMenu);
      return;
    }
    const copy = event.target && event.target.closest("[data-wallet-copy-address]");
    if (copy) {
      event.stopPropagation();
      await copyAddress(copy);
      return;
    }
    const detail = event.target && event.target.closest("[data-wallet-account-detail]");
    if (detail) {
      openAccountDetail(detail.dataset.walletAccountDetail);
    }
  }

  function openAccountMenu(accountId) {
    const account = buildViewAccounts().find((item) => item.account_id === accountId);
    if (!account) {
      return;
    }
    const rows = [
      flowRow(
        getSelectedAccountId() === account.account_id ? "Show default wallet" : "Show in hero",
        account.network,
        () => {
          closeModal();
          if (getSelectedAccountId() === account.account_id) {
            clearAccountSelection();
          } else {
            openAccountDetail(account.account_id);
          }
        },
      ),
      flowRow("Rename", "Change display name", () => openRenameAccount(account)),
      flowRow("Receive", "Show QR code", () => renderReceiveAddress(account)),
      flowRow("Copy address", shortAddress(account.address), async () => {
        await copyText(account.address);
        closeModal();
        showStatus("Address copied.", "success");
      }),
      flowRow("Use by default", defaultIntentLabel(account), () => setDefaultAccount(account)),
    ];
    if (isPasskeyManagedAccount(account)) {
      rows.push(flowRow("Show recovery key", "Requires passkey", () => showRecoveryKey(account)));
    }
    rows.push(
      flowRow("Delete account", "Remove from this Wallet", () => confirmDeleteAccount(account)),
    );
    openFlowModal("Account", account.name, rows);
  }

  function openRenameAccount(account) {
    const form = document.createElement("form");
    form.className = "wallet-flow-form";
    const field = document.createElement("label");
    field.append(document.createTextNode("Account name"));
    const input = document.createElement("input");
    input.name = "label";
    input.autocomplete = "off";
    input.maxLength = 80;
    input.value = account.name;
    field.append(input);
    form.append(field);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      renameAccountFromForm(account, form).catch((error) =>
        showStatus(String(error.message || error), "error"),
      );
    });
    openFlowModal("Rename account", account.network, [form], [
      modalButton("Cancel", closeModal, true),
      modalButton("Save", () => form.requestSubmit()),
    ]);
    input.focus();
    input.select();
  }

  async function renameAccountFromForm(account, form) {
    const data = new FormData(form);
    const label = readText(data.get("label"));
    if (!label) {
      showStatus("Enter an account name.", "error");
      return;
    }
    const button = modalNode.querySelector(
      ".wallet-modal-actions .wallet-button:not(.wallet-button-secondary)",
    );
    setBusy(button, true);
    try {
      await Promise.all(accountIds(account).map((accountId) =>
        fetchJson(`/api/apps/wallet/wallet/accounts/${encodeURIComponent(accountId)}`, {
          method: "PUT",
          headers: shellHeaders({ "content-type": "application/json" }),
          body: JSON.stringify({ label }),
        }),
      ));
      closeModal();
      showStatus(`${label} renamed.`, "success");
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  function confirmDeleteAccount(account) {
    const warning = textNode(
      "p",
      `Delete ${account.name} from this Wallet. Export the recovery key first if this is a built-in account you may need later. Passkey confirmation is required.`,
      "wallet-flow-hint",
    );
    openFlowModal("Delete account", account.network, [warning], [
      modalButton("Cancel", closeModal, true),
      modalButton("Delete", () => deleteAccount(account), true, true),
    ]);
  }

  async function deleteAccount(account) {
    const button = modalNode.querySelector(".wallet-modal-actions .wallet-button-danger");
    setBusy(button, true);
    try {
      const homeToken = await requestFreshPasskeyHomeToken();
      await Promise.all(accountIds(account).map((accountId) =>
        fetchJson(`/api/apps/wallet/wallet/accounts/${encodeURIComponent(accountId)}`, {
          method: "DELETE",
          headers: shellHeaders({ "content-type": "application/json" }),
          body: JSON.stringify({ home_token: homeToken }),
        }),
      ));
      closeModal();
      clearAccountSelection();
      showStatus(`${account.name} deleted.`, "success");
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  async function showRecoveryKey(account) {
    if (!isPasskeyManagedAccount(account)) {
      openInfoModal(
        "Recovery",
        "This account is controlled by an external approval method. Recover it from that wallet.",
      );
      return;
    }
    openFlowModal("Passkey required", "Confirm with your passkey to view this account recovery key.", [
      textNode(
        "p",
        "The key is shown only after fresh passkey verification. Store it offline.",
        "wallet-flow-hint",
      ),
    ]);
    try {
      const homeToken = await requestFreshPasskeyHomeToken();
      const payload = await fetchJson(
        `/api/apps/wallet/wallet/accounts/${encodeURIComponent(account.account_id)}/recovery-key`,
        {
          method: "POST",
          headers: shellHeaders({ "content-type": "application/json" }),
          body: JSON.stringify({ home_token: homeToken }),
        },
      );
      renderRecoveryKey(account, payload);
    } catch (error) {
      openInfoModal("Recovery", String(error.message || error));
    }
  }

  function renderRecoveryKey(account, payload) {
    const recoveryKey = {
      schema: "elastos.wallet.recovery-key/v1",
      account_id: readText(payload.account_id),
      chain_namespace: readText(payload.chain_namespace),
      address: readText(payload.address),
      secret_type: readText(payload.secret_type),
      private_key_hex: readText(payload.private_key_hex),
      note: readText(payload.note),
    };
    const recoveryKeyText = JSON.stringify(recoveryKey, null, 2);
    const note = textNode(
      "p",
      "Use the full Wallet recovery key JSON below to restore this built-in account.",
      "wallet-flow-hint",
    );
    const key = textNode("code", recoveryKeyText, "wallet-secret");
    const details = [
      flowStaticRow("Account", account.name),
      flowStaticRow("Network", account.network),
      flowStaticRow("Address", shortAddress(account.address)),
    ];
    const copy = modalButton("Copy key", async (button) => {
      await copyText(recoveryKeyText);
      pulseCopied(button);
    });
    openFlowModal("Recovery key", "Keep this private.", [note, key, ...details], [
      modalButton("Done", closeModal, true),
      copy,
    ]);
  }

  async function setDefaultAccount(account) {
    await fetchJson("/api/apps/wallet/wallet/default", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({
        account_id: account.account_id,
        chain_namespace: account.chain_namespace,
        intent: defaultIntentForAccount(account),
      }),
    });
    closeModal();
    showStatus(
      `${account.name} is now the default for ${defaultIntentLabel(account).toLowerCase()}.`,
      "success",
    );
    await refreshWalletState();
  }

  function accountIds(account) {
    return Array.isArray(account.account_ids) && account.account_ids.length > 0
      ? account.account_ids
      : [account.account_id];
  }

  function onDocumentClick(event) {
    const openMethod = event.target && event.target.closest("[data-wallet-open-method]");
    if (openMethod) {
      openApprovalMethod(readText(openMethod.dataset.walletOpenMethod));
      return;
    }
    const copy = event.target && event.target.closest("[data-wallet-copy-address]");
    if (copy) {
      event.stopPropagation();
      copyAddress(copy);
    }
  }

  async function copyAddress(button) {
    const address = readText(button.dataset.walletCopyAddress);
    if (!address) {
      return;
    }
    setBusy(button, true);
    try {
      await copyText(address);
      pulseCopied(button);
      showStatus("Address copied.", "success");
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  return {
    copyAddress,
    onAccountClick,
    onDocumentClick,
    openAccountMenu,
  };
}
