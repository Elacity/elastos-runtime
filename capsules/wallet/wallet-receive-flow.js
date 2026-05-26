export function createWalletReceiveFlow({
  buildViewAccounts,
  closeModal,
  copyButton,
  fetchJson,
  flowRow,
  modalButton,
  onQrReady = () => {},
  openFlowModal,
  openInfoModal,
  selectedOrDefaultAccount,
  shellHeaders,
  textNode,
}) {
  const accountQrCache = new Map();
  const accountQrPending = new Set();

  function qrForAccount(account) {
    return accountQrCache.get(account.address) || "";
  }

  async function loadAccountQr(account) {
    if (accountQrCache.has(account.address) || accountQrPending.has(account.address)) {
      return;
    }
    accountQrPending.add(account.address);
    try {
      const qr = await fetchJson("/api/wallet/qr", {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ address: account.address }),
      });
      const svg = qr.svg || "";
      accountQrCache.set(account.address, svg);
      onQrReady(account, svg);
    } catch (_) {
      accountQrCache.set(account.address, "");
      onQrReady(account, "");
    } finally {
      accountQrPending.delete(account.address);
    }
  }

  function selectedOrAllAccounts() {
    const accounts = buildViewAccounts();
    const selected = selectedOrDefaultAccount(accounts);
    return selected ? [selected] : accounts;
  }

  function openReceiveFlow() {
    const accounts = selectedOrAllAccounts();
    if (accounts.length === 0) {
      openInfoModal("Receive", "Create an account before receiving funds.");
      return;
    }
    if (accounts.length === 1) {
      renderReceiveAddress(accounts[0]);
      return;
    }
    openFlowModal(
      "Receive",
      "Choose an account",
      accounts.map((account) =>
        flowRow(account.name, account.network, () => renderReceiveAddress(account)),
      ),
    );
  }

  async function renderReceiveAddress(account) {
    openFlowModal("Receive", account.name, [
      textNode("p", "Preparing QR.", "wallet-state"),
    ]);
    try {
      await loadAccountQr(account);
      const qrBox = document.createElement("div");
      qrBox.className = "wallet-qr";
      qrBox.innerHTML = qrForAccount(account);
      const address = textNode("div", account.address, "wallet-qr-address");
      const warningText = account.chain_namespace?.startsWith("eip155:")
        ? "This EVM address can receive assets on supported EVM networks. Always choose the correct chain in the sending wallet."
        : `Send only ${account.network} assets to this address. Other assets may be lost.`;
      const warning = textNode(
        "p",
        warningText,
        "wallet-flow-hint",
      );
      const copy = copyButton(account.address);
      openFlowModal("Receive", account.name, [qrBox, address, copy, warning], [
        modalButton("Done", closeModal),
      ]);
    } catch (error) {
      openInfoModal("Receive", String(error.message || error));
    }
  }

  return {
    loadAccountQr,
    openReceiveFlow,
    qrForAccount,
    renderReceiveAddress,
  };
}
