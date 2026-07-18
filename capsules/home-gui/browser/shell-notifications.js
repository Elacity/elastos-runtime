/* Notification Center — the clock's other job (macOS model: the clock is the
 * door to notification history). Every entry that flows through the summary
 * poll is remembered in a small client-side ring buffer, so a toast you missed
 * is never gone. Purely a browser concern: localStorage only, no gateway
 * writes, and clearing history here never touches the server-side entries that
 * drive the inbox badge.
 */

import { clockNode } from "./shell-core.js?v=home-20260718m";
import { openTarget } from "./shell-windows.js?v=home-20260718m";

const STORE_KEY = "elastos.home.notifications";
const MAX_ENTRIES = 50;

/* Bound by bindNotificationCenter() once the lazy GUI template is in the DOM. */
let panel = null;
let list = null;
let emptyState = null;
let clearButton = null;

let outsideDismissBound = false;

function loadHistory() {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed : [];
  } catch (_error) {
    return [];
  }
}

function saveHistory(entries) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(entries.slice(0, MAX_ENTRIES)));
  } catch (_error) {
    // History is a convenience; losing it is acceptable.
  }
}

/* Called on every summary poll: remember entries we have not seen before,
   newest first. The summary is the source; this buffer is only the memory. */
export function recordNotifications(summary) {
  const entries = Array.isArray(summary?.notifications?.entries)
    ? summary.notifications.entries
    : [];
  if (entries.length === 0) {
    return;
  }
  const history = loadHistory();
  const known = new Set(history.map((item) => item.id));
  let changed = false;
  for (const entry of entries) {
    const id = String(entry?.id || entry?.action_ref?.action_id || "");
    if (!id || known.has(id)) {
      continue;
    }
    history.unshift({
      id,
      kind: String(entry?.kind || ""),
      title: String(entry?.title || "Notification"),
      body: String(entry?.body || ""),
      seenAt: Date.now(),
    });
    known.add(id);
    changed = true;
  }
  if (changed) {
    saveHistory(history);
    if (panel && !panel.hidden) {
      renderNotificationCenter();
    }
  }
}

function timeLabel(seenAt) {
  const seen = new Date(seenAt);
  const now = new Date();
  const sameDay = seen.toDateString() === now.toDateString();
  if (sameDay) {
    return new Intl.DateTimeFormat([], { hour: "numeric", minute: "2-digit" }).format(seen);
  }
  return new Intl.DateTimeFormat([], { weekday: "short", hour: "numeric", minute: "2-digit" }).format(seen);
}

function isToday(seenAt) {
  return new Date(seenAt).toDateString() === new Date().toDateString();
}

function renderNotificationCenter() {
  if (!panel || !list || !emptyState) {
    return;
  }
  const history = loadHistory();
  list.textContent = "";
  emptyState.hidden = history.length > 0;
  if (clearButton) {
    clearButton.hidden = history.length === 0;
  }
  let renderedSection = "";
  for (const item of history) {
    const section = isToday(item.seenAt) ? "Today" : "Earlier";
    if (section !== renderedSection) {
      const heading = document.createElement("h3");
      heading.className = "notification-center-section";
      heading.textContent = section;
      list.appendChild(heading);
      renderedSection = section;
    }
    const card = document.createElement("button");
    card.type = "button";
    card.className = "notification-center-item";
    const head = document.createElement("div");
    head.className = "notification-center-item-head";
    const title = document.createElement("span");
    title.className = "notification-center-item-title";
    title.textContent = item.title;
    const when = document.createElement("span");
    when.className = "notification-center-item-time";
    when.textContent = timeLabel(item.seenAt);
    head.append(title, when);
    card.appendChild(head);
    if (item.body) {
      const body = document.createElement("p");
      body.className = "notification-center-item-body";
      body.textContent = item.body;
      card.appendChild(body);
    }
    card.addEventListener("click", () => {
      hideNotificationCenter();
      openTarget("inbox");
    });
    list.appendChild(card);
  }
}

export function toggleNotificationCenter() {
  if (!panel) {
    return;
  }
  if (panel.hidden) {
    showNotificationCenter();
  } else {
    hideNotificationCenter();
  }
}

function showNotificationCenter() {
  renderNotificationCenter();
  panel.hidden = false;
  clockNode?.setAttribute("aria-expanded", "true");
  bindOutsideDismiss();
  panel.focus({ preventScroll: true });
}

export function hideNotificationCenter() {
  if (!panel || panel.hidden) {
    return;
  }
  panel.hidden = true;
  clockNode?.setAttribute("aria-expanded", "false");
}

function bindOutsideDismiss() {
  if (outsideDismissBound) {
    return;
  }
  outsideDismissBound = true;
  document.addEventListener("pointerdown", (event) => {
    if (panel.hidden) {
      return;
    }
    if (!panel.contains(event.target) && event.target !== clockNode && !clockNode?.contains(event.target)) {
      hideNotificationCenter();
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !panel.hidden) {
      hideNotificationCenter();
    }
  });
}

/* Called by the home-gui facade once ensureHomeGuiDom() has instantiated the
   lazy GUI template — these nodes do not exist at module-evaluation time. */
export function bindNotificationCenter() {
  if (panel) {
    return;
  }
  panel = document.querySelector("#notification-center");
  list = document.querySelector("#notification-center-list");
  emptyState = document.querySelector("#notification-center-empty");
  clearButton = document.querySelector("#notification-center-clear");
  if (!panel || !clockNode) {
    return;
  }
  clockNode.addEventListener("click", () => {
    toggleNotificationCenter();
  });
  clearButton?.addEventListener("click", () => {
    saveHistory([]);
    renderNotificationCenter();
  });
}
