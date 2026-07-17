import {
  clockNode,
  fetchJson,
  toolbarIdentity,
  toolbarIdentityButton,
  toolbarIdentityAvatar,
  toolbarIdentityName,
  toolbarIdentityMenu,
  toolbarIdentityMenuName,
  toolbarNetwork,
  toolbarNetworkButton,
  toolbarNetworkMenu,
} from "./shell-core.js?v=home-20260717a";

/* Identity chrome: the signed-in principal's name and avatar initial live in
   the system bar as a disclosure menu (account actions: fullscreen, system,
   sign out). Data comes from the home summary; the name is always rendered as
   textContent — never HTML. */

function summaryDisplayName(summary) {
  const handle = summary?.identity?.handle;
  if (typeof handle === "string" && handle.trim()) {
    return handle.trim();
  }
  return "Operator";
}

export function syncIdentity(summary) {
  if (!toolbarIdentity) {
    return;
  }
  const signedIn = Boolean(summary?.authority?.signed_in);
  if (!signedIn) {
    clearIdentitySurface();
    return;
  }
  const name = summaryDisplayName(summary);
  toolbarIdentityName.textContent = name;
  toolbarIdentityMenuName.textContent = name;
  toolbarIdentityAvatar.textContent = [...name][0].toUpperCase();
  toolbarIdentityButton.setAttribute("aria-label", `Account: ${name}`);
  toolbarIdentity.hidden = false;
}

export function clearIdentitySurface() {
  if (!toolbarIdentity) {
    return;
  }
  closeIdentityMenu({ restoreFocus: false });
  toolbarIdentity.hidden = true;
  toolbarIdentityName.textContent = "";
  toolbarIdentityMenuName.textContent = "";
  toolbarIdentityAvatar.textContent = "";
}

/* Disclosure menu behavior (APG menu-button pattern): click or ArrowDown opens
   and focuses the first item; arrows/Home/End navigate; Escape or Tab closes
   and restores focus to the button; pointer-outside closes. */

function identityMenuItems() {
  return Array.from(
    toolbarIdentityMenu.querySelectorAll('[role="menuitem"]:not([hidden])'),
  );
}

function identityMenuOpen() {
  return !toolbarIdentityMenu.hidden;
}

function openIdentityMenu({ focusLast = false } = {}) {
  toolbarIdentityMenu.hidden = false;
  toolbarIdentityButton.setAttribute("aria-expanded", "true");
  const items = identityMenuItems();
  const target = focusLast ? items[items.length - 1] : items[0];
  target?.focus();
}

function closeIdentityMenu({ restoreFocus = true } = {}) {
  if (!toolbarIdentityMenu || toolbarIdentityMenu.hidden) {
    return;
  }
  toolbarIdentityMenu.hidden = true;
  toolbarIdentityButton.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    toolbarIdentityButton.focus();
  }
}

function moveIdentityMenuFocus(delta) {
  const items = identityMenuItems();
  if (items.length === 0) {
    return;
  }
  const index = items.indexOf(document.activeElement);
  const next = index < 0
    ? (delta > 0 ? 0 : items.length - 1)
    : (index + delta + items.length) % items.length;
  items[next].focus();
}

function setupIdentityMenu() {
  if (!toolbarIdentityButton || !toolbarIdentityMenu) {
    return;
  }
  toolbarIdentityButton.addEventListener("click", () => {
    if (identityMenuOpen()) {
      closeIdentityMenu();
    } else {
      openIdentityMenu();
    }
  });
  toolbarIdentityButton.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openIdentityMenu();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      openIdentityMenu({ focusLast: true });
    }
  });
  toolbarIdentityMenu.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeIdentityMenu();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      moveIdentityMenuFocus(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveIdentityMenuFocus(-1);
    } else if (event.key === "Home") {
      event.preventDefault();
      identityMenuItems()[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      identityMenuItems().at(-1)?.focus();
    } else if (event.key === "Tab") {
      closeIdentityMenu();
    }
  });
  toolbarIdentityMenu.addEventListener("click", (event) => {
    if (event.target.closest('[role="menuitem"]')) {
      closeIdentityMenu({ restoreFocus: false });
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (identityMenuOpen() && !toolbarIdentity.contains(event.target)) {
      closeIdentityMenu({ restoreFocus: false });
    }
  });
}

setupIdentityMenu();

export function updateClock() {
  clockNode.textContent = new Intl.DateTimeFormat([], {
    hour: "numeric",
    minute: "2-digit",
    weekday: "short",
    month: "short",
    day: "2-digit",
  }).format(new Date());
}

/* Network status glyph (the macOS Wi-Fi slot): a glanceable rollup of the
   signals the gateway already tracks — Carrier peers, chain RPC health,
   market-index coverage, availability targets. Glance-only in v1: the popover
   names each subsystem's state, no actions. Poll is 30s while signed in; the
   endpoint itself is a stale-while-revalidate cache, so polling stays cheap. */

const NETWORK_POLL_MS = 30_000;

const networkState = {
  timer: 0,
  latest: null,
  fetching: false,
};

const NETWORK_ROWS = [
  ["carrier", "Carrier"],
  ["chain", "Chain RPC"],
  ["index", "Market index"],
  ["availability", "Availability"],
];

function networkRowState(row) {
  const state = typeof row?.state === "string" ? row.state : "";
  if (state) {
    return state;
  }
  // The index row reports coverage rather than a state keyword.
  if (typeof row?.coverage === "string") {
    return "ok";
  }
  return "unknown";
}

function networkRowDetail(key, row) {
  if (!row || typeof row !== "object") {
    return "no data";
  }
  switch (key) {
    case "carrier":
      return typeof row.peers === "number"
        ? `${row.peers} peer${row.peers === 1 ? "" : "s"}`
        : row.detail || "no data";
    case "chain": {
      if (typeof row.last_ok_secs === "number") {
        const state = networkRowState(row);
        const when = row.last_ok_secs < 5 ? "just now" : `${row.last_ok_secs}s ago`;
        return state === "ok" ? `ok ${when}` : `last ok ${when}`;
      }
      return row.last_error ? String(row.last_error).slice(0, 120) : "no reads yet";
    }
    case "index": {
      if (typeof row.coverage === "string") {
        const pct = typeof row.backfill_pct === "number" ? row.backfill_pct : null;
        return pct !== null && pct < 100 ? `${row.coverage} — ${pct}% history` : row.coverage;
      }
      return "no data";
    }
    case "availability":
      return typeof row.targets === "number"
        ? `${row.targets} target${row.targets === 1 ? "" : "s"}`
        : row.detail || "no data";
    default:
      return "no data";
  }
}

/* Glyph rollup: any degraded subsystem dims the glyph with a badge; Carrier
   off/unknown reads as offline (the glyph is "is this computer on the
   network", so the transport row dominates). */
function networkGlyphState(status) {
  if (!status || typeof status !== "object") {
    return "unknown";
  }
  const carrier = networkRowState(status.carrier);
  if (carrier === "off" || carrier === "unknown") {
    return "off";
  }
  const anyDegraded = NETWORK_ROWS.some(
    ([key]) => networkRowState(status[key]) === "degraded",
  );
  return anyDegraded ? "degraded" : "ok";
}

function renderNetworkMenu(status) {
  toolbarNetworkMenu.replaceChildren();
  const heading = document.createElement("div");
  heading.className = "toolbar-network-heading";
  heading.textContent = "Network";
  toolbarNetworkMenu.appendChild(heading);
  for (const [key, label] of NETWORK_ROWS) {
    const row = status?.[key];
    const state = networkRowState(row);
    const item = document.createElement("div");
    item.className = "toolbar-network-row";
    item.dataset.state = state;
    const dot = document.createElement("span");
    dot.className = "toolbar-network-dot";
    const name = document.createElement("span");
    name.className = "toolbar-network-name";
    name.textContent = label;
    const detail = document.createElement("span");
    detail.className = "toolbar-network-detail";
    detail.textContent = networkRowDetail(key, row);
    item.append(dot, name, detail);
    toolbarNetworkMenu.appendChild(item);
  }
}

function applyNetworkStatus(status) {
  networkState.latest = status;
  toolbarNetworkButton.dataset.state = networkGlyphState(status);
  if (!toolbarNetworkMenu.hidden) {
    renderNetworkMenu(status);
  }
}

async function fetchNetworkStatus() {
  if (networkState.fetching) {
    return;
  }
  networkState.fetching = true;
  try {
    applyNetworkStatus(await fetchJson("/api/apps/home/network-status"));
  } catch (_error) {
    // Poll failure = the gateway itself is unreachable; that IS the offline state.
    applyNetworkStatus(null);
  } finally {
    networkState.fetching = false;
  }
}

export function syncNetworkStatus(summary) {
  if (!toolbarNetwork) {
    return;
  }
  const signedIn = Boolean(summary?.authority?.signed_in);
  if (!signedIn) {
    toolbarNetwork.hidden = true;
    closeNetworkMenu({ restoreFocus: false });
    if (networkState.timer) {
      window.clearInterval(networkState.timer);
      networkState.timer = 0;
    }
    return;
  }
  toolbarNetwork.hidden = false;
  if (!networkState.timer) {
    fetchNetworkStatus();
    networkState.timer = window.setInterval(fetchNetworkStatus, NETWORK_POLL_MS);
  }
}

function networkMenuOpen() {
  return !toolbarNetworkMenu.hidden;
}

function openNetworkMenu() {
  renderNetworkMenu(networkState.latest);
  toolbarNetworkMenu.hidden = false;
  toolbarNetworkButton.setAttribute("aria-expanded", "true");
  toolbarNetworkMenu.focus();
  fetchNetworkStatus();
}

function closeNetworkMenu({ restoreFocus = true } = {}) {
  if (!toolbarNetworkMenu || toolbarNetworkMenu.hidden) {
    return;
  }
  toolbarNetworkMenu.hidden = true;
  toolbarNetworkButton.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    toolbarNetworkButton.focus();
  }
}

function setupNetworkMenu() {
  if (!toolbarNetworkButton || !toolbarNetworkMenu) {
    return;
  }
  toolbarNetworkButton.addEventListener("click", () => {
    if (networkMenuOpen()) {
      closeNetworkMenu();
    } else {
      openNetworkMenu();
    }
  });
  toolbarNetworkMenu.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeNetworkMenu();
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (networkMenuOpen() && !toolbarNetwork.contains(event.target)) {
      closeNetworkMenu({ restoreFocus: false });
    }
  });
}

setupNetworkMenu();
