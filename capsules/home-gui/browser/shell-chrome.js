import {
  clockNode,
  fetchJson,
  toolbarHomeButton,
  toolbarIdentityAvatar,
  toolbarIdentityAvatarImage,
  toolbarIdentityMonogram,
  toolbarSystem,
  toolbarIdentityMenu,
  toolbarIdentityMenuName,
} from "./shell-core.js?v=home-20260724ck";
import { renderNcTimeChrome } from "./shell-notifications.js?v=home-20260724ck";
import {
  dismissWithMotion,
  prepareSurfaceOpen,
} from "./shell-motion.js?v=home-20260724ck";

/* System chrome: the ElastOS brand at the far left of the bar is the system
   menu (the macOS Apple-menu position) — show desktop, fullscreen, System,
   sign out, headed by the signed-in principal from the home summary. The name
   is always rendered as textContent — never HTML. */

let passkeyAccountsCache = null;
let passkeyAccountsCachedAt = 0;
let avatarResolveSeq = 0;

function summaryDisplayName(summary) {
  const handle = summary?.identity?.handle;
  if (typeof handle === "string" && handle.trim()) {
    return handle.trim();
  }
  return "Operator";
}

function monogramForName(name) {
  const parts = String(name || "")
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  if (parts.length === 0) {
    return "·";
  }
  const first = [...parts[0]][0] || "·";
  if (parts.length === 1) {
    return first.toUpperCase();
  }
  const second = [...parts[parts.length - 1]][0] || "";
  return `${first}${second}`.toUpperCase().replace(/[^A-Z0-9]/g, "") || "·";
}

function avatarColorForId(principalId) {
  let hash = 0;
  const text = String(principalId || "");
  for (let i = 0; i < text.length; i += 1) {
    hash = ((hash << 5) - hash) + text.charCodeAt(i);
    hash |= 0;
  }
  const hue = Math.abs(hash) % 360;
  return `linear-gradient(145deg, hsl(${hue} 52% 46%), hsl(${(hue + 28) % 360} 58% 36%))`;
}

async function loadPasskeyAccounts() {
  if (passkeyAccountsCache && Date.now() - passkeyAccountsCachedAt < 60_000) {
    return passkeyAccountsCache;
  }
  const status = await fetchJson("/api/auth/passkey/status");
  passkeyAccountsCache = Array.isArray(status?.accounts) ? status.accounts : [];
  passkeyAccountsCachedAt = Date.now();
  return passkeyAccountsCache;
}

function clearAvatarPhoto() {
  if (toolbarIdentityAvatarImage) {
    toolbarIdentityAvatarImage.hidden = true;
    toolbarIdentityAvatarImage.removeAttribute("src");
    toolbarIdentityAvatarImage.onload = null;
    toolbarIdentityAvatarImage.onerror = null;
  }
  toolbarIdentityAvatar?.classList.remove("has-photo");
}

function showMonogramAvatar(name, seed) {
  clearAvatarPhoto();
  if (toolbarIdentityMonogram) {
    toolbarIdentityMonogram.textContent = monogramForName(name);
    toolbarIdentityMonogram.hidden = false;
  }
  if (toolbarIdentityAvatar) {
    toolbarIdentityAvatar.hidden = false;
    toolbarIdentityAvatar.style.background = avatarColorForId(seed || name);
  }
}

async function resolveProfileAvatarUrl(summary) {
  const avatarCid =
    typeof summary?.identity?.profile_card?.avatar_cid === "string"
      ? summary.identity.profile_card.avatar_cid.trim()
      : "";
  if (!avatarCid) {
    return "";
  }
  try {
    const accounts = await loadPasskeyAccounts();
    const name = summaryDisplayName(summary).toLowerCase();
    const match =
      accounts.find((entry) => String(entry?.avatar_cid || "").trim() === avatarCid) ||
      accounts.find(
        (entry) => String(entry?.display_name || "").trim().toLowerCase() === name,
      );
    const credentialId = String(match?.credential_id || "").trim();
    if (!credentialId) {
      return "";
    }
    return `/api/auth/passkey/account-avatar?credential_id=${encodeURIComponent(credentialId)}&v=${encodeURIComponent(avatarCid)}`;
  } catch (_error) {
    return "";
  }
}

function applyAvatarPhoto(url, name, seed) {
  if (!toolbarIdentityAvatarImage || !url) {
    showMonogramAvatar(name, seed);
    return;
  }
  const seq = ++avatarResolveSeq;
  toolbarIdentityAvatarImage.onload = () => {
    if (seq !== avatarResolveSeq) {
      return;
    }
    toolbarIdentityAvatarImage.hidden = false;
    if (toolbarIdentityMonogram) {
      toolbarIdentityMonogram.hidden = true;
    }
    if (toolbarIdentityAvatar) {
      toolbarIdentityAvatar.hidden = false;
      toolbarIdentityAvatar.style.background = "transparent";
      toolbarIdentityAvatar.classList.add("has-photo");
    }
  };
  toolbarIdentityAvatarImage.onerror = () => {
    if (seq !== avatarResolveSeq) {
      return;
    }
    showMonogramAvatar(name, seed);
  };
  if (toolbarIdentityMonogram) {
    toolbarIdentityMonogram.textContent = monogramForName(name);
    toolbarIdentityMonogram.hidden = false;
  }
  if (toolbarIdentityAvatar) {
    toolbarIdentityAvatar.hidden = false;
    toolbarIdentityAvatar.style.background = avatarColorForId(seed || name);
  }
  toolbarIdentityAvatarImage.hidden = true;
  toolbarIdentityAvatarImage.src = url;
}

export function syncIdentity(summary) {
  if (!toolbarIdentityMenuName) {
    return;
  }
  const signedIn = Boolean(summary?.authority?.signed_in);
  if (!signedIn) {
    clearIdentitySurface();
    return;
  }
  const name = summaryDisplayName(summary);
  toolbarIdentityMenuName.textContent = name;
  const seed =
    typeof summary?.identity?.principal_id === "string"
      ? summary.identity.principal_id
      : name;
  showMonogramAvatar(name, seed);
  resolveProfileAvatarUrl(summary).then((url) => {
    if (!url || !toolbarIdentityMenuName?.textContent) {
      return;
    }
    applyAvatarPhoto(url, name, seed);
  });
}

export function clearIdentitySurface() {
  if (!toolbarIdentityMenuName) {
    return;
  }
  closeIdentityMenu({ restoreFocus: false });
  toolbarIdentityMenuName.textContent = "";
  avatarResolveSeq += 1;
  passkeyAccountsCache = null;
  passkeyAccountsCachedAt = 0;
  clearAvatarPhoto();
  if (toolbarIdentityMonogram) {
    toolbarIdentityMonogram.textContent = "";
    toolbarIdentityMonogram.hidden = true;
  }
  if (toolbarIdentityAvatar) {
    toolbarIdentityAvatar.hidden = true;
    toolbarIdentityAvatar.style.background = "";
  }
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

function setIdentityMenuExpanded(expanded) {
  toolbarHomeButton?.setAttribute("aria-expanded", expanded ? "true" : "false");
}

function openIdentityMenu({ focusLast = false } = {}) {
  prepareSurfaceOpen(toolbarIdentityMenu);
  toolbarIdentityMenu.hidden = false;
  setIdentityMenuExpanded(true);
  const items = identityMenuItems();
  const target = focusLast ? items[items.length - 1] : items[0];
  target?.focus();
}

function closeIdentityMenu({ restoreFocus = true } = {}) {
  if (!toolbarIdentityMenu || toolbarIdentityMenu.hidden) {
    return;
  }
  setIdentityMenuExpanded(false);
  dismissWithMotion(toolbarIdentityMenu, {
    className: "bar-menu-leaving",
    ms: 120,
    onDone: () => {
      if (restoreFocus) {
        toolbarHomeButton?.focus();
      }
    },
  });
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

/* Bound by the home-gui facade after ensureHomeGuiDom() instantiates the lazy
   GUI template — the identity nodes do not exist in the first host document. */
let identityMenuBound = false;

function bindIdentityMenuInvoker(invoker) {
  if (!invoker) {
    return;
  }
  invoker.addEventListener("click", () => {
    if (identityMenuOpen()) {
      closeIdentityMenu();
    } else {
      openIdentityMenu();
    }
  });
  invoker.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openIdentityMenu();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      openIdentityMenu({ focusLast: true });
    }
  });
}

export function bindIdentityMenu() {
  if (identityMenuBound || !toolbarHomeButton || !toolbarIdentityMenu) {
    return;
  }
  identityMenuBound = true;
  bindIdentityMenuInvoker(toolbarHomeButton);
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
    if (identityMenuOpen() && !toolbarSystem.contains(event.target)) {
      closeIdentityMenu({ restoreFocus: false });
    }
  });
}

/* Apple menubar chip: "Mon 20 Jul 12:51" — no commas, day before month. */
export function formatMenubarClock(now = new Date()) {
  const parts = new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "numeric",
    minute: "2-digit",
    hour12: true,
  }).formatToParts(now);
  const pick = (type) => parts.find((part) => part.type === type)?.value || "";
  const weekday = pick("weekday").replace(/\.$/, "");
  const day = pick("day");
  const month = pick("month").replace(/\.$/, "");
  const hour = pick("hour");
  const minute = pick("minute");
  // Apple menubar omits AM/PM on the chip; hour is still 12-hour cycle.
  return `${weekday} ${day} ${month} ${hour}:${minute}`.replace(/\s+/g, " ").trim();
}

export function updateClock() {
  const now = new Date();
  if (clockNode) {
    clockNode.textContent = formatMenubarClock(now);
  }
  renderNcTimeChrome(now);
}
