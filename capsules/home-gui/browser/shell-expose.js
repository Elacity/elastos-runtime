/* Show Windows (Exposé v1) — scale every open window (including minimized)
 * into a glanceable grid using transform/opacity only. F3 toggles; click or
 * Enter focuses; Esc dismisses. Pure shell presentation — no gateway, no
 * capture pipeline. Alt+Tab remains the fast list switcher.
 */

import { shellState } from "./shell-core.js?v=home-20260719y";
import {
  browserWindowEntries,
  focusWindow,
  sortWindowEntriesByZOrder,
} from "./shell-windows.js?v=home-20260719y";
import {
  closeOtherShellPopovers,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260719y";

function exposeReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

let active = false;
let activeIndex = 0;
let orderedIds = [];
const originals = new Map(); // windowId -> restored inline styles + wasMinimized
let emptyNode = null;
let chromeNodes = null;
let registered = false;

function exposeEntries() {
  return sortWindowEntriesByZOrder(browserWindowEntries());
}

function gridSpec(count) {
  const cols = Math.max(1, Math.ceil(Math.sqrt(count)));
  const rows = Math.max(1, Math.ceil(count / cols));
  return { cols, rows };
}

function ensureEmptyNode() {
  if (emptyNode) {
    return emptyNode;
  }
  emptyNode = document.createElement("div");
  emptyNode.id = "expose-empty";
  emptyNode.className = "expose-empty";
  emptyNode.setAttribute("role", "status");
  emptyNode.hidden = true;
  emptyNode.innerHTML =
    '<p class="expose-empty-title">No open windows</p>' +
    '<p class="expose-empty-hint">Open an app from the Dock or Search, then try Show Windows again.</p>';
  const stage = document.querySelector("#desktop") || document.body;
  stage.appendChild(emptyNode);
  return emptyNode;
}

function showExposeEmpty(visible) {
  const node = ensureEmptyNode();
  node.hidden = !visible;
}

function setChromeInert(inert) {
  if (!chromeNodes) {
    chromeNodes = [
      document.querySelector("header.toolbar"),
      document.querySelector("footer.taskbar"),
    ].filter(Boolean);
  }
  for (const node of chromeNodes) {
    if (inert) {
      node.dataset.exposeInert = node.inert ? "1" : "0";
      node.inert = true;
    } else if (node.dataset.exposeInert != null) {
      node.inert = node.dataset.exposeInert === "1";
      delete node.dataset.exposeInert;
    }
  }
}

function cardSourceRect(node) {
  if (!node.classList.contains("hidden")) {
    const live = node.getBoundingClientRect();
    if (live.width > 1 && live.height > 1) {
      return live;
    }
  }
  const stage = document.querySelector("#desktop") || document.body;
  const stageRect = stage.getBoundingClientRect();
  const width = Number.parseFloat(node.style.width) || node.offsetWidth || 640;
  const height = Number.parseFloat(node.style.height) || node.offsetHeight || 480;
  const left = stageRect.left + (Number.parseFloat(node.style.left) || 48);
  const top = stageRect.top + (Number.parseFloat(node.style.top) || 48);
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
  };
}

function mountExposeCaption(entry, wasMinimized) {
  const existing = entry.node.querySelector(".expose-caption");
  if (existing) {
    existing.remove();
  }
  const caption = document.createElement("div");
  caption.className = "expose-caption";
  caption.setAttribute("aria-hidden", "true");
  const title = entry.title || entry.targetId || entry.id;
  caption.textContent = wasMinimized ? `${title} · Min` : title;
  entry.node.appendChild(caption);
}

function clearExposeCaption(node) {
  node.querySelector(".expose-caption")?.remove();
}

function layoutExposeGrid(entries) {
  const stage = document.querySelector("#desktop") || document.body;
  const stageRect = stage.getBoundingClientRect();
  const pad = 28;
  const captionReserve = 28;
  const { cols, rows } = gridSpec(entries.length);
  const cellW = (stageRect.width - pad * (cols + 1)) / cols;
  const cellH = (stageRect.height - pad * (rows + 1) - captionReserve) / rows;

  entries.forEach((entry, index) => {
    const node = entry.node;
    const wasMinimized = node.classList.contains("hidden");
    const rect = cardSourceRect(node);
    originals.set(entry.id, {
      transform: node.style.transform,
      transition: node.style.transition,
      zIndex: node.style.zIndex,
      boxShadow: node.style.boxShadow,
      wasMinimized,
    });

    if (wasMinimized) {
      node.classList.remove("hidden");
      node.setAttribute("aria-hidden", "false");
      node.dataset.exposeMinimized = "true";
    }

    const col = index % cols;
    const row = Math.floor(index / cols);
    const targetX = stageRect.left + pad + col * (cellW + pad) + cellW / 2;
    const targetY = stageRect.top + pad + row * (cellH + pad) + cellH / 2;
    const scale =
      Math.min(cellW / Math.max(rect.width, 1), cellH / Math.max(rect.height, 1), 1) * 0.9;
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const dx = targetX - centerX;
    const dy = targetY - centerY;

    node.classList.add("expose-card");
    node.dataset.expose = entry.id;
    node.id = node.id || `expose-card-${entry.id}`;
    node.setAttribute("role", "option");
    node.setAttribute("aria-selected", index === activeIndex ? "true" : "false");
    node.style.zIndex = String(200000 + index);
    node.style.boxShadow = "0 18px 50px rgba(0, 0, 0, 0.45)";
    if (!exposeReducedMotion()) {
      node.style.transition = "transform 220ms cubic-bezier(0.2, 0, 0, 1)";
    }
    node.style.transform = `translate(${dx}px, ${dy}px) scale(${scale})`;
    mountExposeCaption(entry, wasMinimized);
  });
  syncActiveCard();
}

function syncActiveCard() {
  orderedIds.forEach((windowId, index) => {
    const entry = shellState.windows.get(windowId);
    if (!entry?.node) {
      return;
    }
    const selected = index === activeIndex;
    entry.node.classList.toggle("expose-card-active", selected);
    entry.node.setAttribute("aria-selected", selected ? "true" : "false");
  });
  const activeId = orderedIds[activeIndex];
  if (activeId) {
    document.body.setAttribute("aria-activedescendant", `expose-card-${activeId}`);
  } else {
    document.body.removeAttribute("aria-activedescendant");
  }
}

export function isExposeOpen() {
  return active;
}

export function closeExpose() {
  if (!active) {
    return;
  }
  active = false;
  document.body.classList.remove("expose-active");
  document.body.removeAttribute("aria-activedescendant");
  document.body.removeAttribute("aria-label");
  showExposeEmpty(false);
  setChromeInert(false);

  for (const [windowId, style] of originals) {
    const entry = shellState.windows.get(windowId);
    if (!entry?.node) {
      continue;
    }
    const node = entry.node;
    node.style.transform = style.transform;
    node.style.transition = style.transition;
    node.style.zIndex = style.zIndex;
    node.style.boxShadow = style.boxShadow;
    node.classList.remove("expose-card", "expose-card-active");
    node.removeAttribute("data-expose");
    node.removeAttribute("role");
    node.removeAttribute("aria-selected");
    clearExposeCaption(node);
    if (style.wasMinimized || node.dataset.exposeMinimized === "true") {
      node.classList.add("hidden");
      node.classList.remove("window-active");
      node.setAttribute("aria-hidden", "true");
      delete node.dataset.exposeMinimized;
    }
  }
  originals.clear();
  orderedIds = [];
  activeIndex = 0;
}

function activateExposeCard(windowId) {
  if (!windowId || !shellState.windows.has(windowId)) {
    closeExpose();
    return;
  }
  // Drop minimized restoration so closeExpose won't re-hide the chosen window.
  const style = originals.get(windowId);
  if (style) {
    style.wasMinimized = false;
  }
  const entry = shellState.windows.get(windowId);
  if (entry?.node) {
    delete entry.node.dataset.exposeMinimized;
  }
  closeExpose();
  focusWindow(windowId);
}

export function openExpose() {
  closeOtherShellPopovers("show-windows");
  closeExpose();
  const entries = exposeEntries();
  active = true;
  document.body.classList.add("expose-active");
  document.body.setAttribute("aria-label", "Show Windows");
  setChromeInert(true);

  if (entries.length === 0) {
    orderedIds = [];
    activeIndex = 0;
    showExposeEmpty(true);
    return true;
  }

  showExposeEmpty(false);
  orderedIds = entries.map((entry) => entry.id);
  activeIndex = 0;
  layoutExposeGrid(entries);
  return true;
}

export function toggleExpose() {
  if (active) {
    closeExpose();
    return true;
  }
  return openExpose();
}

export function moveExposeSelection(delta) {
  if (!active || orderedIds.length === 0) {
    return false;
  }
  const count = orderedIds.length;
  activeIndex = (activeIndex + delta + count) % count;
  syncActiveCard();
  return true;
}

export function moveExposeSelectionGrid(key) {
  if (!active || orderedIds.length === 0) {
    return false;
  }
  if (key === "ArrowRight") {
    return moveExposeSelection(1);
  }
  if (key === "ArrowLeft") {
    return moveExposeSelection(-1);
  }
  const { cols } = gridSpec(orderedIds.length);
  if (key === "ArrowDown") {
    const next = activeIndex + cols;
    if (next < orderedIds.length) {
      activeIndex = next;
      syncActiveCard();
    }
    return true;
  }
  if (key === "ArrowUp") {
    const next = activeIndex - cols;
    if (next >= 0) {
      activeIndex = next;
      syncActiveCard();
    }
    return true;
  }
  return false;
}

export function activateExposeSelection() {
  if (!active) {
    return false;
  }
  if (orderedIds.length === 0) {
    closeExpose();
    return true;
  }
  activateExposeCard(orderedIds[activeIndex]);
  return true;
}

let exposeBound = false;

export function bindExpose() {
  if (exposeBound) {
    return;
  }
  exposeBound = true;
  if (!registered) {
    registerShellPopover("show-windows", () => closeExpose());
    registered = true;
  }
  document.addEventListener(
    "click",
    (event) => {
      if (!active) {
        return;
      }
      const card = event.target.closest(".window[data-expose]");
      if (card?.dataset.expose) {
        event.preventDefault();
        event.stopPropagation();
        activateExposeCard(card.dataset.expose);
        return;
      }
      if (!event.target.closest(".window") && !event.target.closest("#expose-empty")) {
        closeExpose();
      }
    },
    true,
  );
  document.addEventListener(
    "keydown",
    (event) => {
      if (!active) {
        return;
      }
      if (
        event.key === "ArrowRight" ||
        event.key === "ArrowLeft" ||
        event.key === "ArrowDown" ||
        event.key === "ArrowUp"
      ) {
        if (moveExposeSelectionGrid(event.key)) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
      if (event.key === "Enter" || event.key === " ") {
        if (activateExposeSelection()) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
      if (event.key === "Tab") {
        event.preventDefault();
        event.stopPropagation();
        moveExposeSelection(event.shiftKey ? -1 : 1);
      }
    },
    true,
  );
}
