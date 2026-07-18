import { pendingRequests } from "./wallet-activity.js?v=wallet-20260719j";
import {
  isBitcoinProofRequest,
  isManagedRequest,
  readText,
  requestTiming,
  requestTitle,
  shortAddress,
} from "./wallet-format.js?v=wallet-20260719j";
import { actionButton, setBusy, textNode } from "./wallet-render.js?v=wallet-20260719j";

export function createWalletRequests({
  fetchJson,
  notifyHomeSummaryChanged,
  openApprovalMethod,
  requestFreshPasskeyHomeToken,
  refreshWalletState,
  requestsNode,
  shellHeaders,
  showStatus,
}) {
  function renderRequests(requests, focusRequestId = "") {
    if (!requestsNode) {
      return false;
    }
    requestsNode.replaceChildren();
    const pending = pendingRequests(requests);
    requestsNode.hidden = pending.length === 0;
    if (pending.length === 0) {
      return false;
    }
    let focused = null;
    for (const request of pending) {
      const card = requestCard(request, readText(request.request_id) === focusRequestId);
      if (readText(request.request_id) === focusRequestId) {
        focused = card;
      }
      requestsNode.append(card);
    }
    if (focused) {
      window.setTimeout(() => focused.scrollIntoView({ behavior: "smooth", block: "nearest" }), 0);
    }
    return Boolean(focused);
  }

  function requestCard(request, focused = false) {
    const requestId = readText(request.request_id);
    const card = document.createElement("article");
    card.className = "wallet-request wallet-hero-request";
    card.classList.toggle("wallet-request-focused", focused);
    card.dataset.walletRequestId = requestId;
    const main = document.createElement("div");
    main.className = "wallet-request-main";
    main.append(
      textNode("strong", requestTitle(request)),
      textNode("span", `${readText(request.capsule_id) || "Capsule"} · ${shortAddress(request.address)}`),
      textNode("small", readText(request.reason) || "Approve only what you recognize."),
      textNode("small", requestTiming(request), "wallet-request-time"),
    );
    card.append(main);

    const actions = document.createElement("div");
    actions.className = "wallet-request-actions";
    const connectorId = readText(request.connector_id);
    if (isManagedRequest(request)) {
      actions.append(actionButton("Approve", "walletRequestManagedApprove", requestId));
    } else if (isBitcoinProofRequest(request) || connectorId === "wallet-unisat") {
      actions.append(actionButton("Open UniSat", "walletOpenMethod", "wallet-unisat"));
    } else if (connectorId === "wallet-metamask") {
      actions.append(actionButton("Open MetaMask", "walletOpenMethod", connectorId));
    } else if (connectorId === "wallet-walletconnect") {
      actions.append(actionButton("Open WalletConnect", "walletOpenMethod", connectorId));
    } else if (connectorId) {
      actions.append(actionButton("Open signer", "walletOpenMethod", connectorId));
    }
    actions.append(actionButton("Reject", "walletRequestReject", requestId, true));
    card.append(actions);
    return card;
  }

  function openPendingReview() {
    requestsNode?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  async function onRequestClick(event) {
    const managedApprove = event.target && event.target.closest("[data-wallet-request-managed-approve]");
    if (managedApprove) {
      await approveManagedRequest(managedApprove);
      return;
    }
    const reject = event.target && event.target.closest("[data-wallet-request-reject]");
    if (reject) {
      await rejectRequest(reject);
      return;
    }
    const openMethod = event.target && event.target.closest("[data-wallet-open-method]");
    if (openMethod) {
      openApprovalMethod(readText(openMethod.dataset.walletOpenMethod));
    }
  }

  async function approveManagedRequest(button) {
    const requestId = readText(button.dataset.walletRequestManagedApprove);
    if (!requestId) {
      return;
    }
    setBusy(button, true);
    showStatus("Confirm with your passkey to sign.", "muted");
    try {
      const homeToken = await requestFreshPasskeyHomeToken();
      await fetchJson(`/api/apps/wallet/wallet/managed-approvals/${encodeURIComponent(requestId)}/approve`, {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ reason: "Approved in Wallet", home_token: homeToken }),
      });
      showStatus("Request signed.", "success");
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  async function rejectRequest(button) {
    const requestId = readText(button.dataset.walletRequestReject);
    if (!requestId) {
      return;
    }
    setBusy(button, true);
    showStatus("Rejecting request.", "muted");
    try {
      await fetchJson(`/api/apps/wallet/wallet/approvals/${encodeURIComponent(requestId)}/reject`, {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ reason: "Rejected in Wallet" }),
      });
      showStatus("Request rejected.", "success");
      notifyHomeSummaryChanged();
      await refreshWalletState();
    } catch (error) {
      showStatus(String(error.message || error), "error");
    } finally {
      setBusy(button, false);
    }
  }

  return {
    onRequestClick,
    openPendingReview,
    pendingWalletRequests: pendingRequests,
    renderRequests,
  };
}
