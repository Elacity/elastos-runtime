import { readText, relativeTime, requestTitle, shortAddress } from "./wallet-format.js?v=wallet-20260523a";

export function createWalletActivity({ activityNode, textNode }) {
  function renderActivity(requests) {
    activityNode.replaceChildren();
    if (requests.length === 0) {
      activityNode.append(textNode("p", "No wallet activity yet.", "wallet-state"));
      return;
    }
    const recent = [...requests].sort((a, b) => activityTime(b) - activityTime(a)).slice(0, 12);
    for (const request of recent) {
      const row = document.createElement("article");
      row.className = "wallet-activity-row";
      row.innerHTML = `<span class="wallet-activity-icon">${activityIcon(request)}</span>`;
      const body = document.createElement("div");
      body.append(
        textNode("strong", activityTitle(request)),
        textNode("small", activitySubtitle(request)),
      );
      row.append(body);
      activityNode.append(row);
    }
  }

  return { renderActivity };
}

export function pendingRequests(requests) {
  return requests.filter((request) => readText(request.status) === "pending");
}

export function activityTime(request) {
  return Number(request.completed_at || request.created_at || 0);
}

export function activityIcon(request) {
  const status = readText(request.status);
  if (status === "completed") return "✓";
  if (status === "rejected" || status === "expired") return "×";
  return "!";
}

export function activityTitle(request) {
  const status = readText(request.status);
  const capsule = readText(request.capsule_id) || "Capsule";
  if (request.transaction_hash) {
    return `${capsule} · Sent transaction`;
  }
  if (status === "completed") {
    return `${capsule} · Approved ${requestTitle(request)}`;
  }
  if (status === "rejected") {
    return `${capsule} · Rejected ${requestTitle(request)}`;
  }
  if (status === "expired") {
    return `${capsule} · Expired ${requestTitle(request)}`;
  }
  return `${capsule} · ${requestTitle(request)}`;
}

export function activitySubtitle(request) {
  const parts = [];
  if (request.address) {
    parts.push(shortAddress(request.address));
  }
  if (request.transaction_hash) {
    parts.push(`tx ${shortAddress(request.transaction_hash)}`);
  } else if (request.reason) {
    parts.push(readText(request.reason));
  }
  const when = relativeTime(activityTime(request));
  if (when) {
    parts.push(when);
  }
  return parts.join(" · ");
}
