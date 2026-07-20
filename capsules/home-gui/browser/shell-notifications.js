/* Notification Center — the clock's other job (macOS model: the clock is the
 * door to notification history). Date/time + a compact read-only month sit
 * above history. Every entry that flows through the summary poll is remembered
 * in a small client-side ring buffer, so a toast you missed is never gone.
 * Purely a browser concern: localStorage only, no gateway writes, and clearing
 * history here never touches the server-side entries that drive the inbox badge.
 *
 * History only: entry clicks route to the actionable surface by kind
 * (wallet → Wallet rail; everything else → Inbox rail).
 */

import { clockNode } from "./shell-core.js?v=home-20260719x";
import { showInboxRail } from "./shell-inbox-rail.js?v=home-20260719x";
import {
  closeOtherShellPopovers,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260719x";
import {
  dismissWithMotion,
  prepareSurfaceOpen,
} from "./shell-motion.js?v=home-20260719x";
import { showWalletRail } from "./shell-wallet-rail.js?v=home-20260719x";

const STORE_KEY = "elastos.home.notifications";
const MAX_ENTRIES = 50;
const WEEKDAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/* Bound by bindNotificationCenter() once the lazy GUI template is in the DOM. */
let panel = null;
let list = null;
let emptyState = null;
let clearButton = null;
let ncCalDate = null;
let ncCalTime = null;
let ncCalMonth = null;

let outsideDismissBound = false;
let registered = false;

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

function mondayBasedWeekday(date) {
  return (date.getDay() + 6) % 7;
}

function renderNcMonthGrid(now) {
  if (!ncCalMonth) {
    return;
  }
  const year = now.getFullYear();
  const month = now.getMonth();
  const today = now.getDate();
  const first = new Date(year, month, 1);
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const startPad = mondayBasedWeekday(first);

  ncCalMonth.replaceChildren();
  const header = document.createElement("div");
  header.className = "nc-cal-weekdays";
  header.setAttribute("role", "row");
  for (const label of WEEKDAYS) {
    const cell = document.createElement("span");
    cell.className = "nc-cal-weekday";
    cell.setAttribute("role", "columnheader");
    cell.textContent = label;
    header.appendChild(cell);
  }
  ncCalMonth.appendChild(header);

  const grid = document.createElement("div");
  grid.className = "nc-cal-days";
  grid.setAttribute("role", "rowgroup");

  let row = document.createElement("div");
  row.className = "nc-cal-row";
  row.setAttribute("role", "row");

  for (let i = 0; i < startPad; i += 1) {
    const empty = document.createElement("span");
    empty.className = "nc-cal-day is-empty";
    empty.setAttribute("role", "gridcell");
    empty.setAttribute("aria-hidden", "true");
    row.appendChild(empty);
  }

  for (let day = 1; day <= daysInMonth; day += 1) {
    if (row.children.length === 7) {
      grid.appendChild(row);
      row = document.createElement("div");
      row.className = "nc-cal-row";
      row.setAttribute("role", "row");
    }
    const cell = document.createElement("span");
    cell.className = "nc-cal-day";
    cell.setAttribute("role", "gridcell");
    cell.textContent = String(day);
    if (day === today) {
      cell.classList.add("is-today");
      cell.setAttribute("aria-current", "date");
    }
    row.appendChild(cell);
  }
  while (row.children.length > 0 && row.children.length < 7) {
    const empty = document.createElement("span");
    empty.className = "nc-cal-day is-empty";
    empty.setAttribute("role", "gridcell");
    empty.setAttribute("aria-hidden", "true");
    row.appendChild(empty);
  }
  if (row.children.length > 0) {
    grid.appendChild(row);
  }
  ncCalMonth.appendChild(grid);
}

export function renderNcTimeChrome(now = new Date()) {
  if (ncCalDate) {
    ncCalDate.textContent = new Intl.DateTimeFormat(undefined, {
      weekday: "long",
      day: "numeric",
      month: "long",
    }).format(now);
  }
  if (ncCalTime) {
    ncCalTime.textContent = new Intl.DateTimeFormat(undefined, {
      hour: "numeric",
      minute: "2-digit",
    }).format(now);
  }
  renderNcMonthGrid(now);
}

function renderNotificationCenter() {
  if (!panel || !list || !emptyState) {
    return;
  }
  renderNcTimeChrome();
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
      hideNotificationCenter({ restoreFocus: false });
      if (item.kind === "wallet_approval_request") {
        showWalletRail();
      } else {
        showInboxRail();
      }
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
  closeOtherShellPopovers("notification-center");
  renderNotificationCenter();
  prepareSurfaceOpen(panel);
  panel.hidden = false;
  panel.inert = false;
  panel.setAttribute("aria-hidden", "false");
  clockNode?.setAttribute("aria-expanded", "true");
  bindOutsideDismiss();
  panel.focus({ preventScroll: true });
}

export function hideNotificationCenter({ restoreFocus = true } = {}) {
  if (!panel || panel.hidden) {
    return;
  }
  clockNode?.setAttribute("aria-expanded", "false");
  dismissWithMotion(panel, {
    className: "menubar-card-leaving",
    ms: 120,
    hide: false,
    onDone: () => {
      panel.hidden = true;
      panel.inert = true;
      panel.setAttribute("aria-hidden", "true");
      if (restoreFocus) {
        clockNode?.focus?.();
      }
    },
  });
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
      hideNotificationCenter({ restoreFocus: false });
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
  ncCalDate = document.querySelector("#nc-cal-date");
  ncCalTime = document.querySelector("#nc-cal-time");
  ncCalMonth = document.querySelector("#nc-cal-month");
  if (!panel || !clockNode) {
    return;
  }
  if (!registered) {
    registerShellPopover("notification-center", () => hideNotificationCenter({ restoreFocus: false }));
    registered = true;
  }
  clockNode.addEventListener("click", () => {
    toggleNotificationCenter();
  });
  clearButton?.addEventListener("click", () => {
    saveHistory([]);
    renderNotificationCenter();
  });
  renderNcTimeChrome();
}
