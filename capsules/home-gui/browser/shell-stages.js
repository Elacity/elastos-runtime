/* Fullscreen Stages + Desktop Spaces — presentation only.
 * Enter/Exit Fullscreen; flick Desktop(s) ↔ fullscreen apps;
 * Mission Control can add Desktops (+) and drag windows into Spaces.
 *
 * Glossary (engineer nouns — UI says Desktop / Space only):
 * - Desktop — windowed Space (primary "desktop" or desk-*); many windows
 * - Space — any ring entry: a Desktop or one fullscreen app Space
 * - Stage — internal id for the active Space (activeStageId); not user copy
 * - Expose / Show Windows — Mission Control overview (shell-expose.js)
 * UI ≠ authority: Space switches never mint Capsule/Carrier grants.
 */

import { shellState } from "./shell-core.js?v=home-20260813a";
import {
  rememberWindowRestoreBounds,
  restoreWindowFromSpecialState,
} from "./shell-window-geometry.js?v=home-20260813a";

const DESKTOP_STAGE = "desktop";
let liveRegion = null;
let stageRecency = [];
let focusWindowFn = null;
let persistSessionFn = null;

/** Generation-safe motion timers — flick must not cancel close-FS teardown. */
let spaceSlideTimer = 0;
let spaceSlideGen = 0;
let closeFullscreenTimer = 0;
let closeFullscreenGen = 0;
let fullscreenZoomTimer = 0;
let fullscreenZoomGen = 0;
let spacePagerBound = false;
let spacePeekBound = false;
let spacePeekTimer = 0;
let spacePeekCloseTimer = 0;
let spacePeekEl = null;
let lastSpacePeekPointerX = 0;
const SPACE_EDGE_REVEAL_PX = 22;
const SPACE_PEEK_CLOSE_MS = 120;

export function bindStageWindowHooks({ focusWindow, persistSession } = {}) {
  if (typeof focusWindow === "function") {
    focusWindowFn = focusWindow;
  }
  if (typeof persistSession === "function") {
    persistSessionFn = persistSession;
  }
}

function reducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

function ensureLiveRegion() {
  if (liveRegion) {
    return liveRegion;
  }
  liveRegion = document.createElement("div");
  liveRegion.id = "stage-live-region";
  liveRegion.className = "visually-hidden";
  liveRegion.setAttribute("role", "status");
  liveRegion.setAttribute("aria-live", "polite");
  liveRegion.setAttribute("aria-atomic", "true");
  document.body.appendChild(liveRegion);
  return liveRegion;
}

export function announceStage(message) {
  const node = ensureLiveRegion();
  node.textContent = "";
  queueMicrotask(() => {
    node.textContent = message;
  });
}

export function desktopStageId() {
  return DESKTOP_STAGE;
}

export function getExtraDesktops() {
  if (!Array.isArray(shellState.extraDesktops)) {
    shellState.extraDesktops = [];
  }
  return shellState.extraDesktops;
}

export function isDesktopSpace(spaceId) {
  return spaceId === DESKTOP_STAGE || getExtraDesktops().includes(spaceId);
}

export function getActiveStageId() {
  return shellState.activeStageId || DESKTOP_STAGE;
}

function touchRecency(windowId) {
  stageRecency = [windowId, ...stageRecency.filter((id) => id !== windowId)].slice(0, 24);
}

function stageEntry(windowId) {
  return shellState.windows.get(windowId) || null;
}

function stagedBrowserEntries() {
  return [...shellState.windows.values()].filter(
    (entry) => entry.kind === "browser" && entry.fullscreenStage === true,
  );
}

function syncMaximizeButton(node, fullscreen) {
  const btn = node?.querySelector?.("[data-action='maximize']");
  if (!btn) {
    return;
  }
  btn.setAttribute("aria-label", fullscreen ? "Exit Fullscreen" : "Enter Fullscreen");
  btn.title = fullscreen ? "Exit Fullscreen" : "Enter Fullscreen";
}

function syncMinimizeButton(node, fullscreen) {
  const btn = node?.querySelector?.("[data-action='minimize']");
  if (!btn) {
    return;
  }
  btn.setAttribute(
    "aria-label",
    fullscreen ? "Exit Fullscreen" : "Minimize",
  );
  btn.title = fullscreen ? "Back to Desktop" : "Minimize";
}

function persist() {
  try {
    persistSessionFn?.();
  } catch (_error) {
    // Session persist is best-effort during teardown.
  }
}

let edgeRevealBound = false;
let menubarRevealTimer = 0;
let dockRevealTimer = 0;

function ensureEdgeSensors() {
  if (document.querySelector(".stage-edge-sensor-top")) {
    return;
  }
  const top = document.createElement("div");
  top.className = "stage-edge-sensor-top";
  top.setAttribute("aria-hidden", "true");
  const bottom = document.createElement("div");
  bottom.className = "stage-edge-sensor-bottom";
  bottom.setAttribute("aria-hidden", "true");
  document.body.append(top, bottom);
  top.addEventListener("pointerenter", () => {
    document.body.classList.add("stage-menubar-reveal");
  });
  bottom.addEventListener("pointerenter", () => {
    document.body.classList.add("stage-dock-reveal");
  });
}

function bindEdgeReveal() {
  if (edgeRevealBound) {
    return;
  }
  edgeRevealBound = true;
  ensureEdgeSensors();
  const toolbar = () => document.querySelector("header.toolbar");
  const dock = () => document.querySelector("footer.taskbar");
  document.addEventListener(
    "pointermove",
    (event) => {
      if (isDesktopSpace(getActiveStageId()) || document.body.classList.contains("expose-active")) {
        document.body.classList.remove("stage-menubar-reveal", "stage-dock-reveal");
        return;
      }
      const y = event.clientY;
      const h = window.innerHeight || 800;
      const overToolbar = Boolean(event.target?.closest?.("header.toolbar"));
      const overDock = Boolean(event.target?.closest?.("footer.taskbar"));
      if (y <= 6 || overToolbar) {
        document.body.classList.add("stage-menubar-reveal");
        window.clearTimeout(menubarRevealTimer);
      } else {
        window.clearTimeout(menubarRevealTimer);
        menubarRevealTimer = window.setTimeout(() => {
          document.body.classList.remove("stage-menubar-reveal");
        }, 450);
      }
      if (y >= h - 8 || overDock) {
        document.body.classList.add("stage-dock-reveal");
        window.clearTimeout(dockRevealTimer);
      } else {
        window.clearTimeout(dockRevealTimer);
        dockRevealTimer = window.setTimeout(() => {
          document.body.classList.remove("stage-dock-reveal");
        }, 450);
      }
    },
    { passive: true },
  );
  toolbar()?.addEventListener("pointerleave", () => {
    window.clearTimeout(menubarRevealTimer);
    menubarRevealTimer = window.setTimeout(() => {
      document.body.classList.remove("stage-menubar-reveal");
    }, 280);
  });
  dock()?.addEventListener("pointerleave", () => {
    window.clearTimeout(dockRevealTimer);
    dockRevealTimer = window.setTimeout(() => {
      document.body.classList.remove("stage-dock-reveal");
    }, 280);
  });
}

export function windowVisibleOnActiveSpace(entry, active = getActiveStageId()) {
  if (!entry) {
    return false;
  }
  if (isDesktopSpace(active)) {
    if (entry.fullscreenStage) {
      return false;
    }
    const home = entry.desktopSpaceId || DESKTOP_STAGE;
    return home === active;
  }
  return entry.fullscreenStage === true && entry.id === active;
}

export function syncStagePresentation() {
  const active = getActiveStageId();
  const desktopLike = isDesktopSpace(active);
  document.body.dataset.activeStage = active;
  document.body.dataset.stageKind = desktopLike ? "desktop" : "fullscreen";
  document.body.classList.toggle("stage-active", !desktopLike);
  document.body.classList.toggle("stage-desktop", desktopLike);
  syncSpacePager();
  if (desktopLike) {
    document.body.classList.remove("stage-menubar-reveal", "stage-dock-reveal");
  } else {
    bindEdgeReveal();
  }

  for (const entry of shellState.windows.values()) {
    const staged = entry.fullscreenStage === true;
    const visible = windowVisibleOnActiveSpace(entry, active);
    entry.node.dataset.fullscreenStage = staged ? "true" : "false";
    entry.node.dataset.desktopSpace = entry.desktopSpaceId || DESKTOP_STAGE;
    entry.node.dataset.spaceVisible = visible ? "true" : "false";
    entry.node.dataset.stageActive = !desktopLike && visible ? "true" : "false";
    syncMaximizeButton(entry.node, staged);
    syncMinimizeButton(entry.node, staged);
    if (!document.body.classList.contains("expose-active")) {
      if (!entry.node.classList.contains("expose-card")) {
        entry.node.style.opacity = "";
      }
    }
  }
}

function nodesForSpace(spaceId) {
  if (isDesktopSpace(spaceId)) {
    return [...shellState.windows.values()]
      .filter(
        (entry) =>
          entry.kind === "browser" &&
          entry.fullscreenStage !== true &&
          (entry.desktopSpaceId || DESKTOP_STAGE) === spaceId &&
          !entry.node.classList.contains("hidden"),
      )
      .map((entry) => entry.node);
  }
  const entry = stageEntry(spaceId);
  return entry?.node ? [entry.node] : [];
}

/**
 * Shared FLIP: firstRect appearance → toTransform after current layout.
 * Used by green fullscreen zoom and Mission Control enter/exit (via export).
 */
/**
 * Shared FLIP helper — Mission Control enter uses toTransform=grid; green FS uses none.
 * Callers pass duration/easing so path feels stay identical.
 */
export function flipRectMotion(
  node,
  firstRect,
  {
    toTransform = "none",
    durationMs = 420,
    easing = "cubic-bezier(0.22, 1, 0.36, 1)",
    transitionExtras = "",
    onComplete = null,
    isStillValid = () => true,
  } = {},
) {
  if (
    !node ||
    !firstRect ||
    firstRect.width < 2 ||
    firstRect.height < 2 ||
    reducedMotion()
  ) {
    if (node) {
      node.style.transform = toTransform === "none" ? "" : toTransform;
      node.style.transition = "";
    }
    onComplete?.();
    return false;
  }
  node.style.transition = "none";
  node.style.transform = "none";
  const last = node.getBoundingClientRect();
  if (last.width < 2 || last.height < 2) {
    node.style.transform = toTransform === "none" ? "" : toTransform;
    onComplete?.();
    return false;
  }
  const dx = firstRect.left + firstRect.width / 2 - (last.left + last.width / 2);
  const dy = firstRect.top + firstRect.height / 2 - (last.top + last.height / 2);
  const sx = firstRect.width / last.width;
  const sy = firstRect.height / last.height;
  node.style.transformOrigin = "center center";
  node.style.transform = `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (!isStillValid()) {
        return;
      }
      const extras = transitionExtras ? `, ${transitionExtras}` : "";
      node.style.transition = `transform ${durationMs}ms ${easing}${extras}`;
      node.style.transform = toTransform;
      window.setTimeout(() => {
        if (!isStillValid()) {
          return;
        }
        node.style.transition = "";
        if (toTransform === "none") {
          node.style.transform = "";
        }
        onComplete?.();
      }, durationMs + 20);
    });
  });
  return true;
}

function prepareSpaceSlideNode(node, startX) {
  node.dataset.spaceSliding = "true";
  node.dataset.spaceVisible = "true";
  node.style.visibility = "visible";
  node.style.opacity = "1";
  node.style.transition = "none";
  node.style.transform = `translateX(${startX}%)`;
}

function finishSpaceSlideNodes(nodes) {
  for (const node of nodes) {
    node.style.transition = "";
    node.style.transform = "";
    node.style.opacity = "";
    delete node.dataset.spaceSliding;
    delete node.dataset.spaceClosing;
  }
}

/** Apple-like horizontal slide between Spaces (Dock, flick, Ctrl+←/→). */
function playSpaceSlide(fromId, toId, { announce = true, focus = true } = {}) {
  const ring = buildStageRing();
  const fromIndex = ring.indexOf(fromId);
  const toIndex = ring.indexOf(toId);
  if (
    fromIndex < 0 ||
    toIndex < 0 ||
    fromIndex === toIndex ||
    reducedMotion() ||
    document.body.classList.contains("expose-active")
  ) {
    shellState.activeStageId = toId;
    syncStagePresentation();
    syncSpacePager();
    return false;
  }
  const dir = toIndex > fromIndex ? 1 : -1;
  const fromNodes = nodesForSpace(fromId);
  const toNodes = nodesForSpace(toId);
  if (fromNodes.length === 0 && toNodes.length === 0) {
    shellState.activeStageId = toId;
    syncStagePresentation();
    syncSpacePager();
    return false;
  }

  // Do not clear closeFullscreenTimer — close teardown must always finish.
  const myGen = ++spaceSlideGen;
  window.clearTimeout(spaceSlideTimer);
  shellState.activeStageId = toId;
  if (!isDesktopSpace(toId)) {
    touchRecency(toId);
  }
  document.body.dataset.activeStage = toId;
  document.body.dataset.stageKind = isDesktopSpace(toId) ? "desktop" : "fullscreen";
  document.body.classList.add("stage-sliding", "stage-flicking");
  document.body.classList.toggle("stage-active", !isDesktopSpace(toId));

  for (const node of fromNodes) {
    prepareSpaceSlideNode(node, 0);
  }
  for (const node of toNodes) {
    prepareSpaceSlideNode(node, dir * 100);
    if (!isDesktopSpace(toId)) {
      node.dataset.stageActive = "true";
      node.dataset.fullscreenStage = "true";
    }
  }

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (myGen !== spaceSlideGen) {
        return;
      }
      const motion = "transform 380ms cubic-bezier(0.22, 1, 0.36, 1)";
      for (const node of fromNodes) {
        node.style.transition = motion;
        node.style.transform = `translateX(${-dir * 100}%)`;
      }
      for (const node of toNodes) {
        node.style.transition = motion;
        node.style.transform = "translateX(0)";
      }
      spaceSlideTimer = window.setTimeout(() => {
        if (myGen !== spaceSlideGen) {
          return;
        }
        finishSpaceSlideNodes([...fromNodes, ...toNodes]);
        document.body.classList.remove("stage-sliding", "stage-flicking");
        syncStagePresentation();
        syncSpacePager();
        if (announce) {
          if (isDesktopSpace(toId)) {
            announceStage(toId === DESKTOP_STAGE ? "Desktop" : spaceLabelForDesktop(toId));
          } else {
            const entry = stageEntry(toId);
            announceStage(`${entry?.title || "App"}, fullscreen`);
          }
        }
        if (focus && !isDesktopSpace(toId)) {
          focusWindowFn?.(toId);
        } else if (focus && isDesktopSpace(toId) && toNodes[0]) {
          const id = [...shellState.windows.entries()].find(([, e]) => e.node === toNodes[0])?.[0];
          if (id) {
            focusWindowFn?.(id);
          }
        }
        persist();
      }, 400);
    });
  });
  return true;
}

export function setActiveStage(stageId, { announce = true, focus = true, animate = true } = {}) {
  let next = DESKTOP_STAGE;
  if (isDesktopSpace(stageId)) {
    next = stageId;
  } else if (stageEntry(stageId)?.fullscreenStage) {
    next = stageId;
  }
  const prev = getActiveStageId();
  if (animate && prev !== next && playSpaceSlide(prev, next, { announce, focus })) {
    return next;
  }
  shellState.activeStageId = next;
  if (!isDesktopSpace(next)) {
    touchRecency(next);
  }
  syncStagePresentation();
  if (announce) {
    if (isDesktopSpace(next)) {
      announceStage(next === DESKTOP_STAGE ? "Desktop" : spaceLabelForDesktop(next));
    } else {
      const entry = stageEntry(next);
      announceStage(`${entry?.title || "App"}, fullscreen`);
    }
  }
  if (focus && !isDesktopSpace(next)) {
    focusWindowFn?.(next);
  }
  persist();
  syncSpacePager();
  return next;
}

function spaceLabelForDesktop(spaceId) {
  if (spaceId === DESKTOP_STAGE) {
    return "Desktop";
  }
  const extras = getExtraDesktops();
  const index = extras.indexOf(spaceId);
  return index >= 0 ? `Desktop ${index + 2}` : "Desktop";
}

export function desktopSpaceLabel(spaceId) {
  return spaceLabelForDesktop(spaceId);
}

function spaceLabelForPager(spaceId) {
  if (isDesktopSpace(spaceId)) {
    return spaceLabelForDesktop(spaceId);
  }
  const entry = stageEntry(spaceId);
  return entry?.title || entry?.targetId || "App";
}

/**
 * Apple grammar: the window you click expands into (or shrinks out of) its
 * fullscreen Space. Horizontal Space-slide is for Dock/trackpad flicks only —
 * using it on green-button fullscreen feels disconnected from the tapped window.
 * No Space-slide — zoom this window into fullscreen (via flipRectMotion).
 */
function playFullscreenZoomFlip(node, firstRect, { onComplete } = {}) {
  const myGen = ++fullscreenZoomGen;
  window.clearTimeout(fullscreenZoomTimer);
  node.dataset.fullscreenZooming = "true";
  document.body.classList.add("stage-fullscreen-zoom");
  const finish = () => {
    if (myGen !== fullscreenZoomGen) {
      return;
    }
    delete node.dataset.fullscreenZooming;
    document.body.classList.remove("stage-fullscreen-zoom");
    node.style.transition = "";
    node.style.transform = "";
    onComplete?.();
  };
  if (
    !node ||
    !firstRect ||
    firstRect.width < 2 ||
    firstRect.height < 2 ||
    reducedMotion() ||
    document.body.classList.contains("expose-active")
  ) {
    finish();
    return false;
  }
  node.style.transition = "none";
  node.style.transform = "none";
  const last = node.getBoundingClientRect();
  if (last.width < 2 || last.height < 2) {
    finish();
    return false;
  }
  const dx = firstRect.left + firstRect.width / 2 - (last.left + last.width / 2);
  const dy = firstRect.top + firstRect.height / 2 - (last.top + last.height / 2);
  const sx = firstRect.width / last.width;
  const sy = firstRect.height / last.height;
  node.style.transformOrigin = "center center";
  node.style.transform = `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (myGen !== fullscreenZoomGen) {
        return;
      }
      node.style.transition =
        "transform 420ms cubic-bezier(0.22, 1, 0.36, 1), border-radius 320ms ease";
      node.style.transform = "none";
      fullscreenZoomTimer = window.setTimeout(finish, 440);
    });
  });
  return true;
}

export function enterFullscreenStage(windowId) {
  const entry = stageEntry(windowId);
  if (!entry?.node) {
    return false;
  }
  const node = entry.node;
  if (
    node.dataset.maximized === "true" ||
    node.dataset.browserMaximized === "true" ||
    node.dataset.snap
  ) {
    restoreWindowFromSpecialState(node);
  }
  // Fullscreen Space is always a live stage — never a minimized ghost.
  node.classList.remove("hidden");
  node.setAttribute("aria-hidden", "false");
  delete node.dataset.exposeMinimized;
  rememberWindowRestoreBounds(node);
  // Capture windowed rect BEFORE fullscreen layout (FLIP first).
  const firstRect = node.getBoundingClientRect();
  entry.fullscreenStage = true;
  // Remember which Desktop to return to when leaving fullscreen.
  if (!entry.desktopSpaceId || !isDesktopSpace(entry.desktopSpaceId)) {
    entry.desktopSpaceId = DESKTOP_STAGE;
  }
  node.dataset.fullscreenStage = "true";
  node.dataset.maximized = "false";
  node.dataset.browserMaximized = "false";
  node.dataset.snap = "";
  touchRecency(windowId);
  // No Space-slide — zoom this window into fullscreen.
  setActiveStage(windowId, { animate: false, focus: true, announce: true });
  playFullscreenZoomFlip(node, firstRect);
  return true;
}

export function exitFullscreenStage(windowId, { desktopSpaceId } = {}) {
  const entry = stageEntry(windowId);
  if (!entry?.node) {
    return false;
  }
  const node = entry.node;
  // Capture fullscreen rect BEFORE restoring windowed geometry.
  const firstRect = node.getBoundingClientRect();
  entry.fullscreenStage = false;
  node.dataset.fullscreenStage = "false";
  node.dataset.stageActive = "false";
  if (desktopSpaceId && isDesktopSpace(desktopSpaceId)) {
    entry.desktopSpaceId = desktopSpaceId;
  } else if (!entry.desktopSpaceId) {
    entry.desktopSpaceId = DESKTOP_STAGE;
  }
  restoreWindowFromSpecialState(node);
  syncMaximizeButton(node, false);
  const targetDesktop = entry.desktopSpaceId || DESKTOP_STAGE;
  const wasActive = getActiveStageId() === windowId;
  if (wasActive) {
    // No Space-slide — shrink this window back onto the Desktop.
    setActiveStage(targetDesktop, { animate: false, focus: false, announce: true });
  } else {
    syncStagePresentation();
  }
  playFullscreenZoomFlip(node, firstRect, {
    onComplete: () => {
      focusWindowFn?.(windowId);
      persist();
    },
  });
  if (reducedMotion() || document.body.classList.contains("expose-active")) {
    focusWindowFn?.(windowId);
    persist();
  }
  return true;
}

export function toggleFullscreenStage(windowId) {
  const entry = stageEntry(windowId);
  if (!entry) {
    return false;
  }
  return entry.fullscreenStage
    ? exitFullscreenStage(windowId)
    : enterFullscreenStage(windowId);
}

export function toggleActiveFullscreenStage() {
  const id = shellState.activeWindowId;
  if (!id) {
    announceStage("Focus a window to enter fullscreen");
    return false;
  }
  return toggleFullscreenStage(id);
}

function defaultStageRing() {
  const staged = stagedBrowserEntries();
  staged.sort((a, b) => {
    const ai = stageRecency.indexOf(a.id);
    const bi = stageRecency.indexOf(b.id);
    const ar = ai === -1 ? 999 : ai;
    const br = bi === -1 ? 999 : bi;
    return ar - br;
  });
  return [DESKTOP_STAGE, ...getExtraDesktops(), ...staged.map((entry) => entry.id)];
}

export function buildStageRing() {
  const fallback = defaultStageRing();
  const valid = new Set(fallback);
  const order = Array.isArray(shellState.spaceOrder) ? shellState.spaceOrder : [];
  if (order.length === 0) {
    return fallback;
  }
  const ring = [];
  for (const id of order) {
    if (typeof id === "string" && valid.has(id)) {
      ring.push(id);
      valid.delete(id);
    }
  }
  for (const id of fallback) {
    if (valid.has(id)) {
      ring.push(id);
      valid.delete(id);
    }
  }
  return ring;
}

/** Reorder Spaces for Mission Control / flick ring. toIndex is insert index without spaceId. */
export function moveSpaceInRing(spaceId, toIndex) {
  if (!spaceId) {
    return false;
  }
  const current = buildStageRing();
  if (!current.includes(spaceId)) {
    return false;
  }
  const ring = current.filter((id) => id !== spaceId);
  const clamped = Math.max(0, Math.min(Number(toIndex) || 0, ring.length));
  ring.splice(clamped, 0, spaceId);
  shellState.spaceOrder = ring;
  persist();
  return true;
}

export function restoreSpaceOrder(order) {
  if (!Array.isArray(order)) {
    shellState.spaceOrder = [];
    return;
  }
  shellState.spaceOrder = order.filter((id) => typeof id === "string" && id.length > 0);
}

export function addDesktopSpace() {
  const id = `desk-${Date.now().toString(36)}`;
  getExtraDesktops().push(id);
  persist();
  announceStage("Desktop added");
  return id;
}

/** Close an extra Desktop Space (primary Desktop is never removed). */
export function removeDesktopSpace(desktopSpaceId) {
  if (!desktopSpaceId || desktopSpaceId === DESKTOP_STAGE || !isDesktopSpace(desktopSpaceId)) {
    return false;
  }
  const extras = getExtraDesktops();
  const index = extras.indexOf(desktopSpaceId);
  if (index < 0) {
    return false;
  }
  for (const entry of shellState.windows.values()) {
    if (entry.fullscreenStage) {
      continue;
    }
    if ((entry.desktopSpaceId || DESKTOP_STAGE) === desktopSpaceId) {
      entry.desktopSpaceId = DESKTOP_STAGE;
      entry.node.dataset.desktopSpace = DESKTOP_STAGE;
    }
  }
  extras.splice(index, 1);
  if (Array.isArray(shellState.spaceOrder)) {
    shellState.spaceOrder = shellState.spaceOrder.filter((id) => id !== desktopSpaceId);
  }
  if (getActiveStageId() === desktopSpaceId) {
    setActiveStage(DESKTOP_STAGE, { announce: false, focus: false });
  }
  persist();
  announceStage("Desktop closed");
  return true;
}

export function canRemoveDesktopSpace(desktopSpaceId) {
  return (
    typeof desktopSpaceId === "string" &&
    desktopSpaceId !== DESKTOP_STAGE &&
    getExtraDesktops().includes(desktopSpaceId)
  );
}

export function assignWindowToDesktop(windowId, desktopSpaceId) {
  const entry = stageEntry(windowId);
  if (!entry || !isDesktopSpace(desktopSpaceId)) {
    return false;
  }
  if (entry.fullscreenStage) {
    return exitFullscreenStage(windowId, { desktopSpaceId });
  }
  entry.desktopSpaceId = desktopSpaceId;
  entry.node.dataset.desktopSpace = desktopSpaceId;
  setActiveStage(desktopSpaceId, { focus: false });
  focusWindowFn?.(windowId);
  persist();
  return true;
}

/** Drag window to Spaces shelf / + → new fullscreen Space (Mac grammar). */
export function promoteWindowToFullscreenSpace(windowId) {
  return enterFullscreenStage(windowId);
}

export function flickStage(delta) {
  const ring = buildStageRing();
  if (ring.length < 2) {
    return false;
  }
  const current = getActiveStageId();
  let index = ring.indexOf(current);
  if (index < 0) {
    index = 0;
  }
  const next = ring[(index + delta + ring.length * 8) % ring.length];
  setActiveStage(next, { animate: true });
  return true;
}

export function ensureDesktopForNewLaunch() {
  if (isDesktopSpace(getActiveStageId())) {
    return false;
  }
  setActiveStage(DESKTOP_STAGE, { announce: true, focus: false, animate: true });
  return true;
}

export function exitActiveFullscreenStage() {
  const active = getActiveStageId();
  if (isDesktopSpace(active)) {
    return false;
  }
  return exitFullscreenStage(active);
}

function spaceHasVisibleContent(spaceId, { ignoreWindowId = null } = {}) {
  if (isDesktopSpace(spaceId)) {
    return [...shellState.windows.values()].some(
      (entry) =>
        entry.id !== ignoreWindowId &&
        entry.kind === "browser" &&
        entry.fullscreenStage !== true &&
        (entry.desktopSpaceId || DESKTOP_STAGE) === spaceId &&
        !entry.node.classList.contains("hidden"),
    );
  }
  if (spaceId === ignoreWindowId) {
    return false;
  }
  const entry = stageEntry(spaceId);
  return Boolean(
    entry?.fullscreenStage && entry.node && !entry.node.classList.contains("hidden"),
  );
}

/**
 * Apple-like close of a fullscreen Space: the dying Space slides away while
 * the neighbor slides in. Keep the closing window in the DOM until onComplete.
 * Returns false when motion cannot run (caller should tear down immediately).
 */
export function playCloseFullscreenSpaceMotion(closedSpaceId, nextSpaceId, { onComplete } = {}) {
  const entry = stageEntry(closedSpaceId);
  const fromNode = entry?.node;
  if (!fromNode || !nextSpaceId || closedSpaceId === nextSpaceId) {
    return false;
  }
  if (reducedMotion() || document.body.classList.contains("expose-active")) {
    return false;
  }
  const ring = buildStageRing();
  const fromIndex = ring.indexOf(closedSpaceId);
  const toIndex = ring.indexOf(nextSpaceId);
  if (fromIndex < 0 || toIndex < 0) {
    return false;
  }
  const dir = toIndex > fromIndex ? 1 : -1;
  const toNodes = nodesForSpace(nextSpaceId).filter((node) => node !== fromNode);

  // Own timer/gen — Dock flick must not cancel this onComplete (zombie window).
  const myGen = ++closeFullscreenGen;
  window.clearTimeout(closeFullscreenTimer);
  shellState.activeStageId = nextSpaceId;
  if (!isDesktopSpace(nextSpaceId)) {
    touchRecency(nextSpaceId);
  }
  document.body.dataset.activeStage = nextSpaceId;
  document.body.dataset.stageKind = isDesktopSpace(nextSpaceId) ? "desktop" : "fullscreen";
  document.body.classList.add("stage-sliding", "stage-flicking", "stage-closing-fullscreen");
  document.body.classList.toggle("stage-active", !isDesktopSpace(nextSpaceId));

  fromNode.dataset.spaceClosing = "true";
  prepareSpaceSlideNode(fromNode, 0);
  for (const node of toNodes) {
    prepareSpaceSlideNode(node, dir * 100);
    if (!isDesktopSpace(nextSpaceId)) {
      node.dataset.stageActive = "true";
      node.dataset.fullscreenStage = "true";
    }
  }

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (myGen !== closeFullscreenGen) {
        return;
      }
      const motion =
        "transform 420ms cubic-bezier(0.22, 1, 0.36, 1), opacity 320ms ease";
      fromNode.style.transition = motion;
      // Slide away + slight shrink so the close reads as intentional, not a cut.
      fromNode.style.transform = `translateX(${-dir * 100}%) scale(0.92)`;
      fromNode.style.opacity = "0.35";
      for (const node of toNodes) {
        node.style.transition = motion;
        node.style.transform = "translateX(0)";
        node.style.opacity = "1";
      }
      closeFullscreenTimer = window.setTimeout(() => {
        if (myGen !== closeFullscreenGen) {
          return;
        }
        finishSpaceSlideNodes([fromNode, ...toNodes]);
        document.body.classList.remove(
          "stage-sliding",
          "stage-flicking",
          "stage-closing-fullscreen",
        );
        try {
          onComplete?.();
        } catch (_error) {
          // Caller teardown is best-effort after the motion.
        }
        syncStagePresentation();
        syncSpacePager();
        if (isDesktopSpace(nextSpaceId)) {
          announceStage(nextSpaceId === DESKTOP_STAGE ? "Desktop" : spaceLabelForDesktop(nextSpaceId));
        } else {
          const nextEntry = stageEntry(nextSpaceId);
          announceStage(`${nextEntry?.title || "App"}, fullscreen`);
        }
        if (!isDesktopSpace(nextSpaceId)) {
          focusWindowFn?.(nextSpaceId);
        } else if (toNodes[0]) {
          const id = [...shellState.windows.entries()].find(([, e]) => e.node === toNodes[0])?.[0];
          if (id) {
            focusWindowFn?.(id);
          }
        }
        persist();
      }, 440);
    });
  });
  return true;
}

/**
 * After closing the only app in a fullscreen Space: drop that Space and pick
 * a neighbor that still has something (right, then left, then any, else Desktop).
 */
export function neighborSpaceAfterClosing(closedSpaceId) {
  const fullRing = buildStageRing();
  const index = fullRing.indexOf(closedSpaceId);
  const ring = fullRing.filter((id) => id !== closedSpaceId);
  const tryOrder = [];
  if (index >= 0) {
    if (index + 1 < fullRing.length) {
      tryOrder.push(fullRing[index + 1]);
    }
    if (index - 1 >= 0) {
      tryOrder.push(fullRing[index - 1]);
    }
  }
  for (const id of ring) {
    if (!tryOrder.includes(id)) {
      tryOrder.push(id);
    }
  }
  for (const id of tryOrder) {
    if (id !== closedSpaceId && spaceHasVisibleContent(id, { ignoreWindowId: closedSpaceId })) {
      return id;
    }
  }
  if (index >= 0) {
    if (index + 1 < fullRing.length && fullRing[index + 1] !== closedSpaceId) {
      return fullRing[index + 1];
    }
    if (index - 1 >= 0) {
      return fullRing[index - 1];
    }
  }
  return DESKTOP_STAGE;
}

/** Retire a fullscreen Space id from the ring after its app window is closed. */
export function forgetClosedFullscreenSpace(spaceId) {
  if (!spaceId) {
    return;
  }
  if (Array.isArray(shellState.spaceOrder)) {
    shellState.spaceOrder = shellState.spaceOrder.filter((id) => id !== spaceId);
  }
  stageRecency = stageRecency.filter((id) => id !== spaceId);
  persist();
}

export function applyFullscreenStageFromPlacement(entry, placement) {
  if (!entry) {
    return;
  }
  if (typeof placement?.desktopSpaceId === "string" && isDesktopSpace(placement.desktopSpaceId)) {
    entry.desktopSpaceId = placement.desktopSpaceId;
  } else if (!entry.desktopSpaceId) {
    entry.desktopSpaceId = DESKTOP_STAGE;
  }
  if (placement?.fullscreenStage !== true) {
    return;
  }
  entry.fullscreenStage = true;
  entry.node.dataset.fullscreenStage = "true";
  entry.node.dataset.maximized = "false";
  entry.node.dataset.browserMaximized = "false";
  entry.node.dataset.snap = "";
  touchRecency(entry.id);
}

export function restoreExtraDesktops(desktops) {
  if (!Array.isArray(desktops)) {
    shellState.extraDesktops = [];
    return;
  }
  shellState.extraDesktops = desktops.filter(
    (id) => typeof id === "string" && id.startsWith("desk-"),
  );
}

/**
 * Space pager dots: hidden on fine pointer (left-edge peek owns switching).
 * Shown on coarse/touch as the Overview affordance when ring.length > 1.
 */
export function syncSpacePager() {
  const host = document.querySelector("#space-pager");
  if (!host) {
    return;
  }
  const fine =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(hover: hover) and (pointer: fine)").matches;
  const ring = buildStageRing();
  const hide =
    fine ||
    ring.length < 2 ||
    document.body.classList.contains("expose-active") ||
    document.body.classList.contains("mission-exiting");
  host.hidden = hide;
  host.setAttribute("aria-hidden", hide ? "true" : "false");
  if (hide) {
    host.replaceChildren();
    return;
  }
  const active = getActiveStageId();
  host.replaceChildren();
  for (const spaceId of ring) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "space-pager-dot";
    btn.dataset.spaceId = spaceId;
    const label = spaceLabelForPager(spaceId);
    btn.title = label;
    btn.setAttribute("aria-label", label);
    if (spaceId === active) {
      btn.classList.add("space-pager-dot-current");
      btn.setAttribute("aria-current", "true");
    }
    btn.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (getActiveStageId() === spaceId) {
        return;
      }
      setActiveStage(spaceId, { animate: true, focus: true });
    });
    host.appendChild(btn);
  }
}

export function bindSpacePager() {
  if (spacePagerBound) {
    return;
  }
  spacePagerBound = true;
  syncSpacePager();
}

function clearSpacePeekCloseTimer() {
  if (spacePeekCloseTimer) {
    window.clearTimeout(spacePeekCloseTimer);
    spacePeekCloseTimer = 0;
  }
}

function spacePeekRelatedStillInside(related) {
  if (!related || !(related instanceof Node)) {
    return false;
  }
  if (spacePeekEl?.contains(related)) {
    return true;
  }
  return Boolean(
    related.closest?.("#space-edge-peek, .stage-edge-sensor-left"),
  );
}

/** Right edge of the keep column (screen X). Full height — Y does not matter. */
function spacePeekKeepRight() {
  if (!spacePeekEl || spacePeekEl.hidden) {
    return SPACE_EDGE_REVEAL_PX;
  }
  return spacePeekEl.getBoundingClientRect().right + 4;
}

/**
 * Keep while the pointer is anywhere in the full-height column from the left
 * edge out to the peek’s far right — so you can travel vertically to the
 * ←/→ buttons without the panel tucking.
 */
function pointerInSpacePeekKeepZone(clientX) {
  if (!Number.isFinite(clientX)) {
    return false;
  }
  return clientX <= spacePeekKeepRight();
}

function scheduleSpacePeekClose(options = {}) {
  if (spacePeekCloseTimer || !spacePeekEl || spacePeekEl.hidden) {
    return;
  }
  const leaveX = Number.isFinite(options.leaveX)
    ? options.leaveX
    : lastSpacePeekPointerX;
  spacePeekCloseTimer = window.setTimeout(() => {
    spacePeekCloseTimer = 0;
    if (!spacePeekEl || spacePeekEl.hidden) {
      return;
    }
    if (pointerInSpacePeekKeepZone(leaveX)) {
      return;
    }
    hideSpacePeek();
  }, SPACE_PEEK_CLOSE_MS);
}

/** Widen the left sensor to the peek’s right edge while open (full-height column). */
function syncSpacePeekKeepColumn() {
  const sensor = document.querySelector(".stage-edge-sensor-left");
  if (!sensor) {
    return;
  }
  if (!spacePeekEl || spacePeekEl.hidden) {
    sensor.style.width = "";
    sensor.classList.remove("space-edge-sensor-left-keep");
    return;
  }
  const width = Math.max(SPACE_EDGE_REVEAL_PX, spacePeekKeepRight());
  sensor.style.width = `${width}px`;
  sensor.classList.add("space-edge-sensor-left-keep");
}

/**
 * Fullscreen left strip above the app iframe (same idea as top/bottom chrome
 * sensors). While the peek is open this widens into a full-height keep column
 * out to the peek’s far right so vertical travel to the buttons stays live.
 */
function ensureSpaceEdgeSensor() {
  if (document.querySelector(".stage-edge-sensor-left")) {
    return;
  }
  const left = document.createElement("div");
  left.className = "stage-edge-sensor-left";
  left.setAttribute("aria-hidden", "true");
  left.addEventListener("pointerenter", () => {
    if (
      document.body.classList.contains("expose-active") ||
      document.body.classList.contains("mission-exiting") ||
      document.body.classList.contains("stage-sliding")
    ) {
      return;
    }
    clearSpacePeekCloseTimer();
    if (spacePeekEl && !spacePeekEl.hidden) {
      return;
    }
    if (spacePeekTimer) {
      return;
    }
    spacePeekTimer = window.setTimeout(() => {
      spacePeekTimer = 0;
      paintSpacePeek();
    }, 160);
  });
  left.addEventListener("pointerleave", (event) => {
    window.clearTimeout(spacePeekTimer);
    spacePeekTimer = 0;
    // Moving onto the peek plate stays inside the column.
    if (spacePeekRelatedStillInside(event.relatedTarget)) {
      return;
    }
    // Left the full-height keep column past the peek’s far right → tuck.
    hideSpacePeek();
  });
  document.body.appendChild(left);
}

function ensureSpacePeekEl() {
  if (spacePeekEl?.isConnected) {
    return spacePeekEl;
  }
  spacePeekEl = document.createElement("div");
  spacePeekEl.id = "space-edge-peek";
  spacePeekEl.className = "space-edge-peek";
  spacePeekEl.hidden = true;
  spacePeekEl.setAttribute("aria-hidden", "true");
  spacePeekEl.setAttribute("role", "group");
  spacePeekEl.setAttribute("aria-label", "Switch Space");
  spacePeekEl.innerHTML =
    '<button type="button" class="space-edge-peek-btn" data-dir="-1">' +
    '<span class="space-edge-peek-chevron space-edge-peek-chevron-prev" aria-hidden="true"></span>' +
    '<span class="space-edge-peek-label" data-role="prev-label"></span>' +
    "</button>" +
    '<button type="button" class="space-edge-peek-btn" data-dir="1">' +
    '<span class="space-edge-peek-chevron space-edge-peek-chevron-next" aria-hidden="true"></span>' +
    '<span class="space-edge-peek-label" data-role="next-label"></span>' +
    "</button>";
  spacePeekEl.addEventListener("click", (event) => {
    const btn = event.target.closest(".space-edge-peek-btn");
    if (!btn || !spacePeekEl.contains(btn)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const target = btn.dataset.targetSpace;
    hideSpacePeek();
    if (target) {
      setActiveStage(target, { animate: true, focus: true });
    }
  });
  spacePeekEl.addEventListener("pointerenter", () => {
    clearSpacePeekCloseTimer();
  });
  spacePeekEl.addEventListener("pointerleave", (event) => {
    // Vertical travel into the keep column (sensor) must not tuck.
    // Do not use leave X against the keep right — coords sit on the plate
    // edge and would look “still inside” when moving off to the right.
    if (spacePeekRelatedStillInside(event.relatedTarget)) {
      return;
    }
    if (Number.isFinite(event.clientX)) {
      lastSpacePeekPointerX = event.clientX;
    }
    hideSpacePeek();
  });
  document.body.appendChild(spacePeekEl);
  return spacePeekEl;
}

function hideSpacePeek() {
  window.clearTimeout(spacePeekTimer);
  spacePeekTimer = 0;
  clearSpacePeekCloseTimer();
  if (!spacePeekEl) {
    return;
  }
  spacePeekEl.hidden = true;
  spacePeekEl.setAttribute("aria-hidden", "true");
  syncSpacePeekKeepColumn();
}

function paintSpacePeek() {
  const liveRing = buildStageRing();
  if (liveRing.length < 2) {
    hideSpacePeek();
    return;
  }
  const active = getActiveStageId();
  const index = Math.max(0, liveRing.indexOf(active));
  const prevId = liveRing[(index - 1 + liveRing.length) % liveRing.length];
  const nextId = liveRing[(index + 1) % liveRing.length];
  const el = ensureSpacePeekEl();
  const prevBtn = el.querySelector('.space-edge-peek-btn[data-dir="-1"]');
  const nextBtn = el.querySelector('.space-edge-peek-btn[data-dir="1"]');
  const prevLabel = el.querySelector('[data-role="prev-label"]');
  const nextLabel = el.querySelector('[data-role="next-label"]');
  if (prevBtn) {
    prevBtn.dataset.targetSpace = prevId;
    prevBtn.title = `Previous Space: ${spaceLabelForPager(prevId)}`;
    prevBtn.setAttribute("aria-label", `Go to ${spaceLabelForPager(prevId)}`);
  }
  if (nextBtn) {
    nextBtn.dataset.targetSpace = nextId;
    nextBtn.title = `Next Space: ${spaceLabelForPager(nextId)}`;
    nextBtn.setAttribute("aria-label", `Go to ${spaceLabelForPager(nextId)}`);
  }
  if (prevLabel) {
    prevLabel.textContent = spaceLabelForPager(prevId);
  }
  if (nextLabel) {
    nextLabel.textContent = spaceLabelForPager(nextId);
  }
  clearSpacePeekCloseTimer();
  el.hidden = false;
  el.setAttribute("aria-hidden", "false");
  // Layout first so the keep column matches the painted peek width.
  requestAnimationFrame(() => {
    syncSpacePeekKeepColumn();
  });
}

/** Left-edge dwell peek with ← and → in one place — never the right edge (Wallet). */
export function bindSpaceEdgePeek() {
  if (spacePeekBound) {
    return;
  }
  spacePeekBound = true;
  const fine =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(hover: hover) and (pointer: fine)").matches;
  if (!fine) {
    return;
  }
  ensureSpacePeekEl();
  ensureSpaceEdgeSensor();
  document.addEventListener(
    "pointermove",
    (event) => {
      if (event.pointerType && event.pointerType !== "mouse") {
        return;
      }
      lastSpacePeekPointerX = event.clientX;
      if (
        document.body.classList.contains("expose-active") ||
        document.body.classList.contains("mission-exiting") ||
        document.body.classList.contains("stage-sliding")
      ) {
        hideSpacePeek();
        return;
      }
      const ring = buildStageRing();
      if (ring.length < 2) {
        hideSpacePeek();
        return;
      }
      // Open only on the thin left reveal. Once open, the keep column is the
      // full height out to the peek’s far right (any Y).
      if (spacePeekEl && !spacePeekEl.hidden) {
        if (pointerInSpacePeekKeepZone(event.clientX)) {
          clearSpacePeekCloseTimer();
        } else {
          scheduleSpacePeekClose({ leaveX: event.clientX });
        }
        return;
      }
      if (event.clientX > SPACE_EDGE_REVEAL_PX) {
        window.clearTimeout(spacePeekTimer);
        spacePeekTimer = 0;
        return;
      }
      clearSpacePeekCloseTimer();
      if (spacePeekTimer) {
        return;
      }
      spacePeekTimer = window.setTimeout(() => {
        spacePeekTimer = 0;
        paintSpacePeek();
      }, 160);
    },
    { passive: true },
  );
}

if (shellState.activeStageId == null) {
  shellState.activeStageId = DESKTOP_STAGE;
}
if (!Array.isArray(shellState.extraDesktops)) {
  shellState.extraDesktops = [];
}
if (!Array.isArray(shellState.spaceOrder)) {
  shellState.spaceOrder = [];
}
