export function createWalletReceiveFlow({
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowRow,
  modalButton,
  onQrReady = () => {},
  openFlowModal,
  selectedOrDefaultAccount,
  shellHeaders,
  textNode,
}) {
  const accountQrCache = new Map();
  const accountQrPending = new Set();
  const heroSurface = { surface: "hero" };

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
      openFlowModal(
        "Receive",
        "Create an account before receiving funds.",
        [
          textNode(
            "p",
            "Create or import a Wallet account from Accounts, then come back here for a QR.",
            "wallet-flow-hint",
          ),
        ],
        [modalButton("Done", closeModal)],
        heroSurface,
      );
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
      undefined,
      heroSurface,
    );
  }

  function copyIconButton(address) {
    const copy = document.createElement("button");
    copy.className = "wallet-copy-icon";
    copy.type = "button";
    copy.setAttribute("aria-label", "Copy address");
    copy.title = "Copy address";
    copy.dataset.walletCopyAddress = address;
    copy.innerHTML =
      '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M3.5 10.5V3.5A1 1 0 0 1 4.5 2.5h7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>';
    return copy;
  }

  async function renderReceiveAddress(account) {
    openFlowModal(
      "Receive",
      account.name,
      [textNode("p", "Preparing QR.", "wallet-state")],
      [modalButton("Done", closeModal)],
      heroSurface,
    );
    try {
      await loadAccountQr(account);
      const svg = qrForAccount(account);
      const row = document.createElement("div");
      row.className = "wallet-receive-compact";
      const meta = document.createElement("div");
      meta.className = "wallet-receive-meta";
      const addressRow = document.createElement("div");
      addressRow.className = "wallet-receive-address-row";
      const address = textNode("code", account.address, "wallet-receive-address");
      address.title = account.address;
      addressRow.append(address, copyIconButton(account.address));
      const warningText = account.chain_namespace?.startsWith("eip155:")
        ? "EVM address — send only on a supported network."
        : `Only ${account.network} assets.`;
      meta.append(addressRow, textNode("p", warningText, "wallet-flow-hint"));

      if (svg) {
        const qrBox = document.createElement("div");
        qrBox.className = "wallet-qr";
        qrBox.innerHTML = svg;
        qrBox.setAttribute("aria-label", `QR code for ${account.name}`);
        row.append(qrBox, meta);
      } else {
        const unavailable = textNode("p", "QR unavailable", "wallet-qr-unavailable");
        unavailable.setAttribute("role", "status");
        row.append(unavailable, meta);
      }

      openFlowModal(
        "Receive",
        account.name,
        [row],
        [modalButton("Done", closeModal)],
        heroSurface,
      );
    } catch (error) {
      openFlowModal(
        "Receive",
        String(error.message || error),
        [],
        [modalButton("Done", closeModal)],
        heroSurface,
      );
    }
  }

  return {
    loadAccountQr,
    openReceiveFlow,
    qrForAccount,
    renderReceiveAddress,
  };
}
