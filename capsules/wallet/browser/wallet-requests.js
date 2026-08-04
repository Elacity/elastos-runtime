import { pendingRequests } from "./wallet-activity.js?v=wallet-20260523a";
import {
  isBitcoinProofRequest,
  isManagedRequest,
  readText,
  requestTiming,
  requestTitle,
  shortAddress,
} from "./wallet-format.js?v=wallet-20260523a";
import { actionButton, setBusy, textNode } from "./wallet-render.js?v=wallet-20260523a";

export function createWalletRequests({
  fetchJson,
  notifyHomeSummaryChanged,
  openApprovalMethod,
  requestPasskeyStepUp,
  refreshWalletState,
  requestsNode,
  requestsPanelNode,
  shellHeaders,
  showStatus,
}) {
  function renderRequests(requests, focusRequestId = "") {
    requestsNode.replaceChildren();
    requestsPanelNode.hidden = requests.length === 0;
    let focused = null;
    for (const request of requests) {
      const card = requestCard(request, readText(request.request_id) === focusRequestId);
      if (readText(request.request_id) === focusRequestId) {
        focused = card;
      }
      requestsNode.append(card);
    }
    if (focused) {
      window.setTimeout(() => focused.scrollIntoView({ behavior: "smooth", block: "center" }), 0);
    }
    return Boolean(focused);
  }

  function requestCard(request, focused = false) {
    const requestId = readText(request.request_id);
    const accountAccess = readText(request.intent) === "browser_account_access";
    const accountAccessReview = accountAccess ? requestAccountAccessReview(request.review) : null;
    const card = document.createElement("article");
    card.className = "wallet-request";
    card.classList.toggle("wallet-request-focused", focused);
    card.dataset.walletRequestId = requestId;
    const main = document.createElement("div");
    main.className = "wallet-request-main";
    main.append(
      textNode("strong", accountAccess ? "Browser account access" : requestTitle(request)),
      textNode("span", `${readText(request.capsule_id) || "Capsule"} · ${shortAddress(request.address)}`),
      textNode("small", readText(request.reason) || "Approval requested."),
      textNode("small", requestTiming(request), "wallet-request-time"),
    );
    if (accountAccessReview) {
      main.append(accountAccessReview);
    }
    card.append(main);

    const actions = document.createElement("div");
    actions.className = "wallet-request-actions";
    const connectorId = readText(request.connector_id);
    if (isManagedRequest(request) && (!accountAccess || accountAccessReview)) {
      const approve = actionButton(
        accountAccess ? "Allow" : "Approve",
        "walletRequestManagedApprove",
        requestId,
      );
      approve.dataset.walletRequestIntent = readText(request.intent);
      actions.append(approve);
    } else if (isBitcoinProofRequest(request)) {
      actions.append(actionButton("Open UniSat", "walletOpenMethod", "wallet-unisat"));
    } else if (connectorId === "wallet-metamask") {
      actions.append(actionButton("Open MetaMask", "walletOpenMethod", connectorId));
    }
    actions.append(actionButton("Reject", "walletRequestReject", requestId, true));
    card.append(actions);
    return card;
  }

  function requestAccountAccessReview(review) {
    if (!review || typeof review !== "object" || readText(review.kind) !== "account_access") {
      return null;
    }
    const details = document.createElement("details");
    details.className = "wallet-request-review";
    details.open = true;
    details.append(textNode("summary", "Review exact account access"));
    const fields = document.createElement("dl");
    const appendField = (label, value) => {
      const text = Array.isArray(value)
        ? value.join(", ")
        : typeof value === "number"
        ? String(value)
        : readText(value);
      if (!text) return;
      fields.append(textNode("dt", label), textNode("dd", text));
    };
    appendField("Origin", review.origin);
    appendField("Page", review.page_url);
    appendField("Permission", review.permission);
    appendField("Account", review.account_id);
    appendField("Address", review.address);
    appendField("Requested network", review.requested_chain_namespace);
    appendField("Allowed networks", review.chain_namespaces);
    appendField("Access expires", review.grant_expires_at);
    details.append(fields);
    return details;
  }

  function openPendingReview() {
    requestsPanelNode?.scrollIntoView({ behavior: "smooth", block: "start" });
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
    const accountAccess = readText(button.dataset.walletRequestIntent) === "browser_account_access";
    setBusy(button, true);
    showStatus(
      accountAccess
        ? "Confirm with your passkey to allow account access."
        : "Confirm with your passkey to sign.",
      "muted",
    );
    try {
      const intent = { request_id: requestId, reason: "Approved in Wallet" };
      const stepUpToken = await requestPasskeyStepUp("wallet.approve", intent);
      await fetchJson(`/api/apps/wallet/wallet/managed-approvals/${encodeURIComponent(requestId)}/approve`, {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ reason: intent.reason, step_up_token: stepUpToken }),
      });
      showStatus(accountAccess ? "Account access allowed." : "Request signed.", "success");
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
