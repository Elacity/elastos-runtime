import { copyIconButton, textNode as defaultTextNode } from "./wallet-render.js?v=wallet-20260720j";

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
  textNode = defaultTextNode,
}) {
  const accountQrCache = new Map();
  const accountQrPending = new Set();
  const heroSurface = { surface: "hero" };
  const receiveAddressSurface = { surface: "hero", headerInline: true };

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

  async function renderReceiveAddress(account) {
    openFlowModal(
      "Receive",
      account.name,
      [textNode("p", "Preparing QR.", "wallet-state")],
      [modalButton("Done", closeModal)],
      receiveAddressSurface,
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
        receiveAddressSurface,
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
