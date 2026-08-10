import {
  clockNode,
  toolbarIdentityMenuName,
} from "./shell-core.js?v=home-20260810ps";

function identityDisplayName(summary) {
  const raw =
    summary?.identity?.profile_card?.display_name ||
    summary?.identity?.handle ||
    summary?.authority?.principal_id ||
    "";
  return String(raw || "").trim();
}

export function syncIdentity(summary) {
  if (!toolbarIdentityMenuName) {
    return;
  }
  const name = identityDisplayName(summary);
  toolbarIdentityMenuName.textContent = name;
  // Refresh empty-state greeting if it's already on screen (name often arrives after first paint).
  const greeting = document.querySelector(".agent-harness-empty-greeting");
  if (greeting) {
    const first = name.split(/\s+/)[0] || "";
    const label = first.includes("@") ? first.split("@")[0] || "" : first;
    greeting.textContent = label
      ? `What's on your mind, ${label}?`
      : "What's on your mind?";
  }
}

export function clearIdentitySurface() {
  if (toolbarIdentityMenuName) {
    toolbarIdentityMenuName.textContent = "";
  }
}

export function updateClock() {
  clockNode.textContent = new Intl.DateTimeFormat([], {
    hour: "numeric",
    minute: "2-digit",
    weekday: "short",
    month: "short",
    day: "2-digit",
  }).format(new Date());
}
