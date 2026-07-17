import {
  shellState,
  mountGlyph,
  trapTabWithin,
  launcher,
} from "./shell-core.js?v=home-20260718n";
import {
  moveDesktopSelection,
} from "./shell-surface.js?v=home-20260718n";
import { toggleSpotlight } from "./shell-spotlight.js?v=home-20260718n";
import {
  focusWindow,
  closeWindow,
  sortWindowEntriesByZOrder,
} from "./shell-windows.js?v=home-20260718n";
import {
  closeExpose,
  isExposeOpen,
  toggleExpose,
} from "./shell-expose.js?v=home-20260718n";
import {
  hideQuickLook,
  isQuickLookOpen,
  toggleQuickLook,
} from "./shell-quicklook.js?v=home-20260718n";

/* Shell keyboard layer.
 *
 * Scope honesty: key events only reach the shell while focus is in the shell
 * document — an app iframe with focus receives its own keys (that is correct
 * app behavior, same as any OS). macOS reserves Cmd+Space (Spotlight) and
 * Cmd+Tab at the system level, so each binding has an in-browser equivalent:
 *   spotlight  Cmd+Space / Ctrl+Space (the launcher stays on the dock icon)
 *   switcher   Alt+Tab (hold Alt, Tab advances, release commits)
 *   cycle      Cmd+`
 *   close      Cmd+W (honored when the browser lets the page claim it,
 *              e.g. installed/kiosk mode)
 *   overlay    Cmd+/
 *
 * The layer sleeps whenever Home GUI is not the mounted shell: while an
 * alternate root shell (home-cli) owns the surface, the host document must
 * not answer desktop chords.
 */

/* Bound by bindShellKeyboard() once the lazy GUI template is in the DOM. */
let switcherRoot = null;
let switcherList = null;
let shortcutsOverlay = null;
let shortcutsClose = null;

const switcherState = {
  open: false,
  entries: [],
  index: 0,
  invoker: null,
};

function homeGuiOwnsKeys() {
  return shellState.homeGuiMounted === true;
}

function allWindowEntriesByRecency() {
  return sortWindowEntriesByZOrder(Array.from(shellState.windows.values()));
}

function typingInField(event) {
  const target = event.target;
  if (!target || typeof target.closest !== "function") {
    return false;
  }
  return Boolean(target.closest("input, textarea, select, [contenteditable='true']"));
}

/* ---- Alt+Tab window switcher (listbox semantics) ---- */

function openSwitcher() {
  const entries = allWindowEntriesByRecency();
  if (entries.length === 0) {
    return false;
  }
  switcherState.open = true;
  switcherState.entries = entries;
  switcherState.index = entries.length > 1 ? 1 : 0;
  switcherState.invoker = document.activeElement;
  renderSwitcher();
  switcherRoot.hidden = false;
  switcherRoot.setAttribute("aria-hidden", "false");
  return true;
}

function renderSwitcher() {
  switcherList.replaceChildren();
  switcherState.entries.forEach((entry, index) => {
    const item = document.createElement("div");
    item.className = "window-switcher-item";
    item.id = `window-switcher-item-${index}`;
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", index === switcherState.index ? "true" : "false");
    const glyph = document.createElement("span");
    glyph.className = "window-switcher-glyph app-glyph";
    glyph.setAttribute("aria-hidden", "true");
    mountGlyph(glyph, entry.targetId || entry.id);
    const label = document.createElement("span");
    label.className = "window-switcher-title";
    label.textContent = entry.title || entry.id;
    item.append(glyph, label);
    if (entry.node.classList.contains("hidden")) {
      item.dataset.minimized = "true";
    }
    item.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      switcherState.index = index;
      commitSwitcher();
    });
    switcherList.appendChild(item);
  });
  switcherList.setAttribute(
    "aria-activedescendant",
    `window-switcher-item-${switcherState.index}`,
  );
}

function moveSwitcherSelection(delta) {
  const count = switcherState.entries.length;
  if (count === 0) {
    return;
  }
  switcherState.index = (switcherState.index + delta + count) % count;
  for (const [index, item] of Array.from(switcherList.children).entries()) {
    item.setAttribute("aria-selected", index === switcherState.index ? "true" : "false");
  }
  switcherList.setAttribute(
    "aria-activedescendant",
    `window-switcher-item-${switcherState.index}`,
  );
}

function closeSwitcher() {
  switcherState.open = false;
  switcherState.entries = [];
  switcherRoot.hidden = true;
  switcherRoot.setAttribute("aria-hidden", "true");
  switcherList.replaceChildren();
}

function commitSwitcher() {
  const entry = switcherState.entries[switcherState.index];
  closeSwitcher();
  if (entry && shellState.windows.has(entry.id)) {
    focusWindow(entry.id, { moveFocus: true });
  }
}

/* ---- Cmd+` cycle among visible windows ---- */

function cycleVisibleWindows() {
  const visible = allWindowEntriesByRecency().filter(
    (entry) => !entry.node.classList.contains("hidden"),
  );
  if (visible.length < 2) {
    return false;
  }
  // Frontmost is first; send it to the back of the rotation.
  focusWindow(visible[visible.length - 1].id, { moveFocus: true });
  return true;
}

/* ---- Shortcuts overlay ---- */

function shortcutsOverlayOpen() {
  return shortcutsOverlay ? !shortcutsOverlay.hidden : false;
}

let shortcutsInvoker = null;

export function toggleShortcutsOverlay() {
  if (!shortcutsOverlay) {
    return;
  }
  if (shortcutsOverlayOpen()) {
    hideShortcutsOverlay();
    return;
  }
  shortcutsInvoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  shortcutsOverlay.hidden = false;
  shortcutsOverlay.setAttribute("aria-hidden", "false");
  shortcutsClose?.focus();
}

function hideShortcutsOverlay() {
  if (!shortcutsOverlay || shortcutsOverlay.hidden) {
    return;
  }
  shortcutsOverlay.hidden = true;
  shortcutsOverlay.setAttribute("aria-hidden", "true");
  shortcutsInvoker?.focus?.();
  shortcutsInvoker = null;
}

/* Shell-switch retirement: the host may unmount the GUI while any of these
   transient surfaces is open. */
export function retireKeyboardSurfaces() {
  if (switcherState.open) {
    closeSwitcher();
  }
  hideShortcutsOverlay();
}

/* ---- Global bindings ---- */

let keyboardBound = false;

/* Called by the home-gui facade once ensureHomeGuiDom() has instantiated the
   lazy GUI template — these nodes do not exist at module-evaluation time. */
export function bindShellKeyboard() {
  if (keyboardBound) {
    return;
  }
  keyboardBound = true;
  switcherRoot = document.querySelector("#window-switcher");
  switcherList = document.querySelector("#window-switcher-list");
  shortcutsOverlay = document.querySelector("#shortcuts-overlay");
  shortcutsClose = document.querySelector("#shortcuts-close");

  shortcutsClose?.addEventListener("click", hideShortcutsOverlay);
  shortcutsOverlay?.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".shortcuts-card")) {
      hideShortcutsOverlay();
    }
  });
  shortcutsOverlay?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideShortcutsOverlay();
      return;
    }
    trapTabWithin(shortcutsOverlay.querySelector(".shortcuts-card"), event);
  });

  // Capture phase so the switcher wins over surface-level handlers.
  document.addEventListener(
    "keydown",
    (event) => {
      if (!homeGuiOwnsKeys()) {
        return;
      }
      if (event.key === "Escape") {
        if (isExposeOpen()) {
          event.preventDefault();
          event.stopPropagation();
          closeExpose();
          return;
        }
        if (isQuickLookOpen()) {
          event.preventDefault();
          event.stopPropagation();
          hideQuickLook();
          return;
        }
      }
      if (event.key === "F3" && !event.metaKey && !event.ctrlKey && !event.altKey && !typingInField(event)) {
        if (toggleExpose()) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
      // Quick Look: bare Space with a desktop selection, when focus is still in
      // the shell document (not an app iframe) — matches desktop-OS convention.
      if (
        event.code === "Space" &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !typingInField(event) &&
        !isExposeOpen() &&
        launcher.hidden &&
        shellState.selectedDesktopTargetId
      ) {
        const el = document.activeElement;
        const inAppFrame =
          el?.tagName === "IFRAME" || Boolean(el?.closest?.(".window-frame"));
        if (!inAppFrame && toggleQuickLook()) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
      }
      if (switcherState.open) {
        if (event.key === "Tab" || event.key === "ArrowRight" || event.key === "ArrowDown") {
          event.preventDefault();
          event.stopPropagation();
          moveSwitcherSelection(event.shiftKey && event.key === "Tab" ? -1 : 1);
          return;
        }
        if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
          event.preventDefault();
          event.stopPropagation();
          moveSwitcherSelection(-1);
          return;
        }
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          closeSwitcher();
          return;
        }
        if (event.key === "Enter") {
          event.preventDefault();
          event.stopPropagation();
          commitSwitcher();
          return;
        }
        return;
      }

      if (event.altKey && event.key === "Tab" && !event.metaKey && !event.ctrlKey) {
        if (openSwitcher()) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }

      const primaryModifier = event.metaKey || event.ctrlKey;
      if (!primaryModifier) {
        return;
      }
      if (event.code === "Space" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        toggleSpotlight();
        return;
      }
      if (event.key === "`" && !event.shiftKey && !event.altKey) {
        if (cycleVisibleWindows()) {
          event.preventDefault();
        }
        return;
      }
      if ((event.key === "w" || event.key === "W") && !event.shiftKey && !event.altKey) {
        if (shellState.activeWindowId && !typingInField(event) && launcher.hidden) {
          event.preventDefault();
          closeWindow(shellState.activeWindowId);
        }
        return;
      }
      if (event.key === "/" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        toggleShortcutsOverlay();
      }
    },
    { capture: true },
  );

  document.addEventListener("keyup", (event) => {
    if (switcherState.open && event.key === "Alt") {
      commitSwitcher();
    }
  });

  window.addEventListener("blur", () => {
    if (switcherState.open) {
      closeSwitcher();
    }
  });
}

/* ---- Desktop arrow-key navigation (wired from the home-gui facade) ---- */

export function handleDesktopArrowKey(event) {
  const direction = {
    ArrowLeft: "left",
    ArrowRight: "right",
    ArrowUp: "up",
    ArrowDown: "down",
  }[event.key];
  if (!direction) {
    return false;
  }
  if (moveDesktopSelection(direction)) {
    event.preventDefault();
    return true;
  }
  return false;
}
