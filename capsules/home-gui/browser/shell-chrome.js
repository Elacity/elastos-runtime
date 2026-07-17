import {
  clockNode,
  toolbarHomeButton,
  toolbarSystem,
  toolbarIdentityMenu,
  toolbarIdentityMenuName,
} from "./shell-core.js?v=home-20260717b";

/* System chrome: the ElastOS brand at the far left of the bar is the system
   menu (the macOS Apple-menu position) — show desktop, fullscreen, System,
   sign out, headed by the signed-in principal from the home summary. The name
   is always rendered as textContent — never HTML. */

function summaryDisplayName(summary) {
  const handle = summary?.identity?.handle;
  if (typeof handle === "string" && handle.trim()) {
    return handle.trim();
  }
  return "Operator";
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
  toolbarIdentityMenuName.textContent = summaryDisplayName(summary);
}

export function clearIdentitySurface() {
  if (!toolbarIdentityMenuName) {
    return;
  }
  closeIdentityMenu({ restoreFocus: false });
  toolbarIdentityMenuName.textContent = "";
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
  toolbarHomeButton.setAttribute("aria-expanded", "true");
  const items = identityMenuItems();
  const target = focusLast ? items[items.length - 1] : items[0];
  target?.focus();
}

function closeIdentityMenu({ restoreFocus = true } = {}) {
  if (!toolbarIdentityMenu || toolbarIdentityMenu.hidden) {
    return;
  }
  toolbarIdentityMenu.hidden = true;
  toolbarHomeButton.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    toolbarHomeButton.focus();
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
  if (identityMenuBound || !toolbarHomeButton || !toolbarIdentityMenu) {
    return;
  }
  identityMenuBound = true;
  toolbarHomeButton.addEventListener("click", () => {
    if (identityMenuOpen()) {
      closeIdentityMenu();
    } else {
      openIdentityMenu();
    }
  });
  toolbarHomeButton.addEventListener("keydown", (event) => {
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
    if (identityMenuOpen() && !toolbarSystem.contains(event.target)) {
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
