/* App menu bar — the macOS contract: the top bar owns File/Edit/View for the
 * FOCUSED app. Apps declare their menus with a tiny postMessage manifest
 * (`home:menu-manifest`); the shell renders the titles next to the app name
 * and posts each chosen command straight back to that app's iframe as
 * `elastos:menu-command`. Apps that never send a manifest get the default
 * File menu (New Window / Close Window), so the bar always behaves like an
 * operating system, never like a webpage.
 *
 * Trust: manifests are DATA only — sanitized, size-capped, rendered via
 * textContent. A manifest can never inject markup or reach another window;
 * commands are dispatched only to the window that declared them.
 */

import {
  shellState,
  toolbarActiveTitleNode,
  canonicalTargetTitle,
  targetTitle,
} from "./shell-core.js?v=home-20260804aw";
import {
  dismissWithMotion,
  prepareSurfaceOpen,
} from "./shell-motion.js?v=home-20260804aw";

/* Resolved lazily — the menubar lives in the lazy GUI template, which is not
   in the DOM at module-evaluation time. */
let menubarNode = null;

function menubarRoot() {
  if (!menubarNode) {
    menubarNode = document.querySelector("#toolbar-menubar");
  }
  return menubarNode;
}

const MAX_MENUS = 6;
const MAX_ITEMS = 20;
const MAX_LABEL = 48;
const CMD_PATTERN = /^[a-z0-9:._-]{1,64}$/i;

/* Shell-handled commands (never posted to the app). */
const CMD_NEW_WINDOW = "__new-window";
const CMD_CLOSE_WINDOW = "__close-window";

const manifests = new Map(); // windowId -> sanitized menus
let deps = null;
let openMenu = null; // { index, button, popover }
let outsideDismissBound = false;

export function bindMenubar(nextDeps) {
  deps = nextDeps;
}

export function setMenuManifest(windowId, rawMenus) {
  const menus = sanitizeMenus(rawMenus);
  if (menus.length) {
    manifests.set(windowId, menus);
  } else {
    manifests.delete(windowId);
  }
  syncMenubar();
}

function sanitizeMenus(rawMenus) {
  if (!Array.isArray(rawMenus)) {
    return [];
  }
  const menus = [];
  for (const rawMenu of rawMenus.slice(0, MAX_MENUS)) {
    const title = cleanLabel(rawMenu?.title);
    if (!title || !Array.isArray(rawMenu.items)) {
      continue;
    }
    const items = [];
    for (const rawItem of rawMenu.items.slice(0, MAX_ITEMS)) {
      if (rawItem === "-") {
        items.push("-");
        continue;
      }
      const label = cleanLabel(rawItem?.label);
      const cmd = typeof rawItem?.cmd === "string" && CMD_PATTERN.test(rawItem.cmd)
        ? rawItem.cmd
        : "";
      if (!label || !cmd) {
        continue;
      }
      items.push({ label, cmd, disabled: rawItem?.disabled === true });
    }
    if (items.some((item) => item !== "-")) {
      menus.push({ title, items });
    }
  }
  return menus;
}

function cleanLabel(value) {
  return typeof value === "string" ? value.trim().slice(0, MAX_LABEL) : "";
}

function defaultMenus(targetId) {
  // "New Window" only where opening one is what actually happens: single-
  // session apps refocus, and the protected viewers must never get a menu
  // side door into a dKMS open.
  const items = [];
  if (deps?.supportsNewWindow?.(targetId)) {
    items.push({ label: "New Window", cmd: CMD_NEW_WINDOW, disabled: false }, "-");
  }
  items.push({ label: "Close Window", cmd: CMD_CLOSE_WINDOW, disabled: false });
  return [{ title: "File", items }];
}

function activeBrowserWindow() {
  const entry = shellState.activeWindowId
    ? shellState.windows.get(shellState.activeWindowId)
    : null;
  if (!entry || entry.node.classList.contains("hidden")) {
    return null;
  }
  return entry;
}

function syncActiveAppTitle(entry) {
  const node = toolbarActiveTitleNode || document.querySelector("#toolbar-active-title");
  if (!node) {
    return;
  }
  if (!entry) {
    node.textContent = "Home";
    return;
  }
  const summary = shellState.currentSummary;
  const fromSummary = summary ? targetTitle(summary, entry.targetId) : "";
  const title =
    canonicalTargetTitle(entry.targetId, entry.title || fromSummary) ||
    fromSummary ||
    entry.targetId ||
    "Home";
  node.textContent = title;
}

/* Re-render for the focused window. Runs on every window UI refresh, so it
   also prunes manifests whose windows are gone. */
export function syncMenubar() {
  const root = menubarRoot();
  const entry = activeBrowserWindow();
  syncActiveAppTitle(entry);
  if (!root) {
    return;
  }
  for (const windowId of [...manifests.keys()]) {
    if (!shellState.windows.has(windowId)) {
      manifests.delete(windowId);
    }
  }
  closeMenus({ restoreFocus: false, animate: false });
  root.replaceChildren();
  if (!entry) {
    return;
  }
  const menus = manifests.get(entry.id) || defaultMenus(entry.targetId);
  menus.forEach((menu, index) => {
    root.appendChild(buildMenu(menu, index, entry.id));
  });
}

function buildMenu(menu, index, windowId) {
  const container = document.createElement("div");
  container.className = "toolbar-menu";

  const button = document.createElement("button");
  button.type = "button";
  button.className = "toolbar-menu-title";
  button.textContent = menu.title;
  button.setAttribute("aria-haspopup", "true");
  button.setAttribute("aria-expanded", "false");

  const popover = document.createElement("div");
  popover.className = "toolbar-menu-popover";
  popover.setAttribute("role", "menu");
  popover.setAttribute("aria-label", menu.title);
  popover.hidden = true;

  for (const item of menu.items) {
    if (item === "-") {
      const divider = document.createElement("div");
      divider.className = "toolbar-menu-divider";
      divider.setAttribute("role", "separator");
      popover.appendChild(divider);
      continue;
    }
    const row = document.createElement("button");
    row.type = "button";
    row.className = "toolbar-menu-item";
    row.setAttribute("role", "menuitem");
    row.textContent = item.label;
    row.disabled = item.disabled;
    row.tabIndex = -1;
    row.addEventListener("click", () => {
      closeMenus({ restoreFocus: false });
      runMenuCommand(windowId, item.cmd);
    });
    popover.appendChild(row);
  }

  button.addEventListener("click", () => {
    if (openMenu?.index === index) {
      closeMenus();
      return;
    }
    openMenuAt(index);
  });
  // macOS sweep: while one menu is open, pointing at another title opens it.
  button.addEventListener("pointerenter", () => {
    if (openMenu && openMenu.index !== index) {
      openMenuAt(index);
    }
  });
  button.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openMenuAt(index, { focusFirst: true });
    }
  });
  popover.addEventListener("keydown", (event) => onPopoverKeydown(event, index));

  container.append(button, popover);
  return container;
}

function menuContainers() {
  const root = menubarRoot();
  return root ? [...root.querySelectorAll(".toolbar-menu")] : [];
}

function openMenuAt(index, options = {}) {
  // Instant close when sweeping between titles — leave motion only on full dismiss.
  closeMenus({ restoreFocus: false, animate: false });
  const container = menuContainers()[index];
  if (!container) {
    return;
  }
  const button = container.querySelector(".toolbar-menu-title");
  const popover = container.querySelector(".toolbar-menu-popover");
  prepareSurfaceOpen(popover);
  popover.hidden = false;
  button.setAttribute("aria-expanded", "true");
  openMenu = { index, button, popover };
  bindOutsideDismiss();
  if (options.focusFirst) {
    focusMenuItem(popover, 0);
  }
}

export function closeMenus(options = {}) {
  if (!openMenu) {
    return;
  }
  const { button, popover } = openMenu;
  openMenu = null;
  button.setAttribute("aria-expanded", "false");
  const restore =
    options.restoreFocus !== false && popover.contains(document.activeElement);
  dismissWithMotion(popover, {
    className: "bar-menu-leaving",
    ms: 120,
    animate: options.animate !== false,
    onDone: () => {
      if (restore) {
        button.focus();
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
    if (openMenu && !event.target.closest(".toolbar-menu")) {
      closeMenus({ restoreFocus: false });
    }
  });
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && openMenu) {
      event.stopPropagation();
      closeMenus();
    }
  }, true);
}

function focusMenuItem(popover, index) {
  const items = [...popover.querySelectorAll(".toolbar-menu-item:not(:disabled)")];
  if (!items.length) {
    return;
  }
  const clamped = ((index % items.length) + items.length) % items.length;
  items[clamped].focus();
}

function onPopoverKeydown(event, index) {
  const popover = openMenu?.popover;
  if (!popover) {
    return;
  }
  const items = [...popover.querySelectorAll(".toolbar-menu-item:not(:disabled)")];
  const current = items.indexOf(document.activeElement);
  if (event.key === "ArrowDown") {
    event.preventDefault();
    focusMenuItem(popover, current + 1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    focusMenuItem(popover, current - 1);
  } else if (event.key === "ArrowRight") {
    event.preventDefault();
    openSiblingMenu(index + 1);
  } else if (event.key === "ArrowLeft") {
    event.preventDefault();
    openSiblingMenu(index - 1);
  }
}

function openSiblingMenu(index) {
  const count = menuContainers().length;
  if (count < 2) {
    return;
  }
  openMenuAt(((index % count) + count) % count, { focusFirst: true });
}

function runMenuCommand(windowId, cmd) {
  const entry = shellState.windows.get(windowId);
  if (!entry) {
    return;
  }
  if (cmd === CMD_CLOSE_WINDOW) {
    deps?.closeWindow(windowId);
    return;
  }
  if (cmd === CMD_NEW_WINDOW) {
    deps?.openTarget(entry.targetId);
    return;
  }
  const frame = entry.node.querySelector(".window-frame");
  // App frames are opaque-sandboxed (origin "null"): a concrete URL target
  // would never match, so post with "*" — the frame element pins the target.
  frame?.contentWindow?.postMessage(
    { type: "elastos:menu-command", cmd },
    "*",
  );
}
