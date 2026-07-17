export function createWalletSendFlow({
  METHOD_LABELS,
  accountDisplayBalance,
  buildViewAccounts,
  closeModal,
  fetchJson,
  flowRow,
  flowStaticRow,
  formatAmount,
  getCurrentPrices,
  getCurrentRequests,
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
  setCurrentRequests,
  shellHeaders,
  shortAddress,
  showStatus,
  textNode,
  validateAddress,
}) {
  function openSendFlow() {
    const accounts = buildViewAccounts();
    const selected = selectedOrDefaultAccount(accounts);
    const sendableAccounts = accounts.filter(canSendFromAccount);
    if (accounts.length === 0) {
      openFlowModal(
        "Send",
        "Create or import a Wallet account before sending funds.",
        [
          textNode(
            "p",
            "Create an EVM or Bitcoin account from Accounts, or import a Wallet key there.",
            "wallet-flow-hint",
          ),
        ],
        [modalButton("Done", closeModal)],
      );
      return;
    }
    if (sendableAccounts.length === 0) {
      const rows = accounts.length
        ? accounts.map((account) =>
            flowStaticRow(account.name, sendAccountStatusMessage(account)),
          )
        : [
            textNode(
              "p",
              "Create a passkey-managed EVM account before sending on an EVM chain.",
              "wallet-flow-hint",
            ),
          ];
      openFlowModal("Send", "No signing account available", rows, [
        modalButton("Done", closeModal),
      ]);
      return;
    }
    const fundedSendableAccounts = sendableAccounts.filter((account) => account.assets.length > 0);
    if (selected && canSendFromAccount(selected) && selected.assets.length > 0) {
      renderSendAssetStep(selected);
      return;
    }
    if ((!selected || !canSendFromAccount(selected) || selected.assets.length === 0)
      && fundedSendableAccounts.length === 1) {
      renderSendAssetStep(fundedSendableAccounts[0]);
      return;
    }
    const rows = [];
    if (selected) {
      rows.push(
        textNode(
          "p",
          `${selected.name}: ${sendAccountStatusMessage(selected)}`,
          "wallet-flow-hint",
        ),
      );
    }
    rows.push(
      ...sendableAccounts.map((account) =>
        flowRow(
          account.name,
          account.assets.length > 0
            ? accountDisplayBalance(account, getCurrentPrices(), getDisplayCurrency())
            : "Unfunded",
          () => renderSendAssetStep(account),
        ),
      ),
    );
    openFlowModal("Send", "Choose a signing account", rows);
  }

  function renderSendAssetStep(account) {
    if (account.assets.length === 0) {
      openFlowModal(
        "Send",
        account.name,
        [
          textNode(
            "p",
            "This account is unfunded. Receive assets before sending.",
            "wallet-flow-hint",
          ),
        ],
        [modalButton("Back", openSendFlow, true), modalButton("Done", closeModal)],
      );
      return;
    }
    openFlowModal(
      "Send",
      "Choose an asset",
      account.assets.map((asset) =>
        flowRow(
          `${formatAmount(asset.amount)} ${asset.symbol}`,
          asset.network || account.name,
          () => renderSendForm(account, asset),
        ),
      ),
    );
  }

  function renderSendForm(account, asset) {
    const sendAccount = accountForAsset(account, asset);
    const form = document.createElement("form");
    form.className = "wallet-flow-form";
    form.innerHTML = `
      <label>Amount <input name="amount" inputmode="decimal" autocomplete="off" placeholder="0.00"></label>
      <label>To address <input name="to" autocomplete="off" placeholder="${sendAccount.chain_namespace.startsWith("eip155:") ? "0x..." : "bc1q..."}"></label>
    `;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const data = new FormData(form);
      const amount = readText(data.get("amount"));
      const to = readText(data.get("to"));
      if (!amount || !to || !validateAddress(to, sendAccount.chain_namespace)) {
        showStatus("Enter a valid amount and address.", "error");
        return;
      }
      renderSendReview(sendAccount, asset, amount, to);
    });
    const next = modalButton("Review", () => form.requestSubmit());
    openFlowModal(
      "Send",
      `${account.name} · ${asset.symbol}`,
      [form],
      [modalButton("Cancel", closeModal, true), next],
    );
  }

  function accountForAsset(account, asset) {
    return {
      ...account,
      account_id: asset.account_id || account.account_id,
      chain_namespace: asset.chain_namespace || account.chain_namespace,
      network: asset.network || account.network,
    };
  }

  function renderSendReview(account, asset, amount, to) {
    const rows = [
      flowStaticRow("From", account.name),
      flowStaticRow("To", shortAddress(to)),
      flowStaticRow("Amount", `${amount} ${asset.symbol}`),
      flowStaticRow("Network", account.network),
      flowStaticRow("Signer", METHOD_LABELS[account.method] || "Unknown"),
    ];
    const unavailableReason = sendUnavailableReason(account);
    if (unavailableReason) {
      rows.push(flowStaticRow("Status", unavailableReason));
    }
    const sign = modalButton("Sign", (button) =>
      sendTransactionFromReview(account, amount, to, button),
    );
    sign.disabled = Boolean(unavailableReason);
    openFlowModal("Review send", "Confirm details", rows, [
      modalButton("Back", () => renderSendForm(account, asset), true),
      sign,
    ]);
  }

  function sendUnavailableReason(account) {
    if (canSendFromAccount(account)) {
      return "";
    }
    if (!account.chain_namespace || !account.chain_namespace.startsWith("eip155:")) {
      return "Sending currently supports built-in EVM accounts protected by your passkey.";
    }
    if (!["eip155:20", "eip155:8453"].includes(account.chain_namespace)) {
      return "This EVM network is not available yet.";
    }
    if (!isPasskeyManagedAccount(account)) {
      return "Open the approval method for this account to send.";
    }
    if (account.signing_status === "managed_key_missing") {
      return "This built-in account is missing its local signing key. Import this account's Wallet recovery key or create a new account.";
    }
    if (account.signing_status === "managed_key_unavailable") {
      return "This account cannot be unlocked on this device. Import its Wallet recovery key or create a new account.";
    }
    if (!managedAccountCanSign(account)) {
      return "This built-in account cannot sign on this device yet.";
    }
    return "";
  }

  function canSendFromAccount(account) {
    return (
      Boolean(account) &&
      ["eip155:20", "eip155:8453"].includes(account.chain_namespace) &&
      isPasskeyManagedAccount(account) &&
      managedAccountCanSign(account)
    );
  }

  function managedAccountCanSign(account) {
    return (
      account.signing_available === true ||
      account.signing_status === "managed_key_available"
    );
  }

  function sendAccountStatusMessage(account) {
    return sendUnavailableReason(account) || "Ready to sign on this device.";
  }

  async function sendTransactionFromReview(account, amount, to, button) {
    setBusy(button, true);
    showStatus("Confirm with your passkey to sign.", "muted");
    try {
      const intent = {
        account_id: account.account_id,
        chain_namespace: account.chain_namespace,
        to,
        amount,
      };
      const homeToken = await requestFreshPasskeyHomeToken("wallet.send", intent);
      const payload = await fetchJson("/api/apps/wallet/wallet/send", {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }, homeToken),
        body: JSON.stringify({
          ...intent,
          home_token: homeToken,
        }),
      });
      const hash = readText(payload.transaction_hash);
      closeModal();
      showStatus(
        hash ? `Transaction sent: ${shortAddress(hash)}` : "Transaction sent.",
        "success",
      );
      notifyHomeSummaryChanged();
      recordCompletedSendActivity(payload, account);
      await refreshWalletState();
      schedulePostSendBalanceRefresh();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  function recordCompletedSendActivity(payload, account) {
    const request = payload && payload.approval_request;
    if (!request || !request.request_id) {
      return;
    }
    const completed = {
      ...request,
      status: "completed",
      capsule_id: "wallet",
      address: request.address || account.address,
      completed_at: Math.floor(Date.now() / 1000),
      transaction_hash: readText(payload.transaction_hash),
    };
    const currentRequests = getCurrentRequests();
    const nextRequests = [
      completed,
      ...currentRequests.filter((item) => item.request_id !== completed.request_id),
    ];
    setCurrentRequests(nextRequests);
    renderActivity(nextRequests);
  }

  function schedulePostSendBalanceRefresh() {
    [2500, 7000, 15000].forEach((delay) => {
      window.setTimeout(() => {
        refreshWalletState().catch((error) =>
          showStatus(String(error.message || error), "error"),
        );
      }, delay);
    });
  }

  return {
    canSendFromAccount,
    openSendFlow,
    sendUnavailableReason,
  };
}
