import {
  clockNode,
  toolbarIdentity,
  toolbarIdentityButton,
  toolbarIdentityAvatar,
  toolbarIdentityName,
  toolbarIdentityMenu,
  toolbarIdentityMenuName,
} from "./shell-core.js?v=home-20260717b";

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

/* Bound by the home-gui facade after ensureHomeGuiDom() instantiates the lazy
   GUI template — the identity nodes do not exist in the first host document. */
let identityMenuBound = false;

export function bindIdentityMenu() {
  if (identityMenuBound || !toolbarIdentityButton || !toolbarIdentityMenu) {
    return;
  }
  identityMenuBound = true;
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

export function updateClock() {
  clockNode.textContent = new Intl.DateTimeFormat([], {
    hour: "numeric",
    minute: "2-digit",
    weekday: "short",
    month: "short",
    day: "2-digit",
  }).format(new Date());
}
