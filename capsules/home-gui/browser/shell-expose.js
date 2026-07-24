/* Mission Control / Show Windows — Mac Spaces grammar:
 * Open MC from the current Space → that Space zooms out onto the floor
 * (Desktop windows, or the fullscreen app you were in). Spaces bar above.
 * Click another Space → preview it on the floor (stay in MC).
 * Click the selected Space, a floor window, or the wallpaper → leave MC
 * and zoom into that Space. Next MC open zooms out from wherever you are.
 * + adds a Desktop. Drag a floor window onto the shelf → fullscreen Space.
 */

import {
  desktopBackdrop,
  mountGlyph,
  shellState,
} from "./shell-core.js?v=home-20260724ai";
import {
  browserWindowEntries,
  focusWindow,
  sortWindowEntriesByZOrder,
} from "./shell-windows.js?v=home-20260724ai";
import {
  closeOtherShellPopovers,
  registerEscapeHandler,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260724ai";
import {
  addDesktopSpace,
  assignWindowToDesktop,
  buildStageRing,
  canRemoveDesktopSpace,
  desktopSpaceLabel,
  desktopStageId,
  getActiveStageId,
  isAgentSpace,
  isDesktopSpace,
  moveSpaceInRing,
  promoteWindowToFullscreenSpace,
  removeDesktopSpace,
  setActiveStage,
  syncStagePresentation,
  syncSpacePager,
  flipRectMotion,
} from "./shell-stages.js?v=home-20260724ai";
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
let spacesShelf = null;
let spacesBar = null;
let selectedSpaceId = desktopStageId();
let spaceThumbIndex = 0;
let missionDrag = null;
let spaceThumbGesture = null;
let promoteGhost = null;
let dragProxy = null;
let suppressExposeClick = false;
let missionExitTimer = 0;
let missionExiting = false;
/** When Overview exit zoom is aborted mid-flight, still commit the target Space. */
let pendingMissionFinish = null;
let ensureGridTimer = 0;
let missionHitCache = null;
let lastSpacesBarRingKey = "";
const MISSION_SHELF_FALLBACK_PX = 172;
const MISSION_DRAG_THRESHOLD_PX = 8;
const MISSION_EXIT_MS = 340;

function pointInRect(x, y, rect, pad = 0) {
  return (
    x >= rect.left - pad &&
    x <= rect.right + pad &&
    y >= rect.top - pad &&
    y <= rect.bottom + pad
  );
}

function exposeEntries() {
  return sortWindowEntriesByZOrder(browserWindowEntries());
}

function floorEntriesForSpace(spaceId) {
  const all = exposeEntries();
  if (isAgentSpace(spaceId)) {
    return [];
  }
  if (isDesktopSpace(spaceId)) {
    return all.filter(
      (entry) =>
        entry.fullscreenStage !== true &&
        (entry.desktopSpaceId || desktopStageId()) === spaceId,
    );
  }
  return all.filter((entry) => entry.id === spaceId);
}

function ensureSpacesBar() {
  if (spacesBar) {
    return spacesBar;
  }
  spacesShelf = document.createElement("div");
  spacesShelf.id = "mission-spaces-shelf";
  spacesShelf.className = "mission-spaces-shelf";
  spacesShelf.setAttribute("aria-hidden", "false");
  spacesBar = document.createElement("div");
  spacesBar.id = "mission-spaces-bar";
  spacesBar.className = "mission-spaces-bar";
  spacesBar.setAttribute("role", "tablist");
  spacesBar.setAttribute("aria-label", "Desktops, Agent, and fullscreen apps");
  spacesShelf.appendChild(spacesBar);
  document.body.appendChild(spacesShelf);
  return spacesBar;
}

function missionShelfHeight() {
  const raw = getComputedStyle(document.body).getPropertyValue("--mission-shelf-h");
  const n = Number.parseFloat(raw);
  return Number.isFinite(n) && n > 40 ? n : MISSION_SHELF_FALLBACK_PX;
}

function spaceLabel(spaceId) {
  if (isAgentSpace(spaceId)) {
    return "Agent";
  }
  if (isDesktopSpace(spaceId)) {
    return desktopSpaceLabel(spaceId);
  }
  const entry = shellState.windows.get(spaceId);
  return entry?.title || entry?.targetId || "App";
}

function windowPlacementBounds(entry) {
  const node = entry?.node;
  if (!node) {
    return { x: 48, y: 60, w: 560, h: 404 };
  }
  const x =
    Number.parseFloat(node.style.left) ||
    Number.parseFloat(node.dataset.restoreLeft) ||
    Number.parseFloat(node.dataset.restoreX) ||
    48;
  const y =
    Number.parseFloat(node.style.top) ||
    Number.parseFloat(node.dataset.restoreTop) ||
    Number.parseFloat(node.dataset.restoreY) ||
    60;
  const w =
    Number.parseFloat(node.style.width) ||
    Number.parseFloat(node.dataset.restoreWidth) ||
    node.offsetWidth ||
    560;
  const h =
    Number.parseFloat(node.style.height) ||
    Number.parseFloat(node.dataset.restoreHeight) ||
    node.offsetHeight ||
    404;
  return { x, y, w, h };
}

function paintSpacePreview(previewEl, spaceId) {
  previewEl.replaceChildren();
  previewEl.style.backgroundImage = "";
  previewEl.classList.remove("mission-space-preview-desktop", "mission-space-preview-agent");
  delete previewEl.dataset.liveThumb;
  if (isAgentSpace(spaceId)) {
    previewEl.classList.add("mission-space-preview-agent");
    const mark = document.createElement("span");
    mark.className = "mission-space-agent-mark";
    mark.setAttribute("aria-hidden", "true");
    const caption = document.createElement("span");
    caption.className = "mission-space-agent-caption";
    caption.textContent = "Agent";
    previewEl.append(mark, caption);
    return;
  }
  if (isDesktopSpace(spaceId)) {
    previewEl.classList.add("mission-space-preview-desktop");
    const wallpaper =
      desktopBackdrop?.style?.getPropertyValue("--desktop-wallpaper") ||
      getComputedStyle(desktopBackdrop || document.body).getPropertyValue("--desktop-wallpaper");
    if (wallpaper && wallpaper !== "none") {
      previewEl.style.backgroundImage = wallpaper.trim();
    }
    // Mac-like snapshot: wallpaper + placed window panes for that Desktop.
    const stage = document.querySelector("#desktop") || document.body;
    const stageW = Math.max(stage.clientWidth || window.innerWidth || 1, 1);
    const stageH = Math.max(stage.clientHeight || window.innerHeight || 1, 1);
    const layer = document.createElement("div");
    layer.className = "mission-space-desk-windows";
    layer.setAttribute("aria-hidden", "true");
    // Schematic panes for Desktop Spaces (live windows stay on the floor).
    const entries = floorEntriesForSpace(spaceId).slice(0, 10);
    for (const entry of entries) {
      const bounds = windowPlacementBounds(entry);
      const pane = document.createElement("div");
      pane.className = "mission-space-desk-window";
      pane.style.left = `${(bounds.x / stageW) * 100}%`;
      pane.style.top = `${(bounds.y / stageH) * 100}%`;
      pane.style.width = `${(bounds.w / stageW) * 100}%`;
      pane.style.height = `${(bounds.h / stageH) * 100}%`;
      const head = document.createElement("div");
      head.className = "mission-space-desk-window-head";
      const glyph = document.createElement("span");
      glyph.className = "mission-space-desk-window-glyph";
      mountGlyph(glyph, entry.targetId);
      const title = document.createElement("span");
      title.textContent = entry.title || entry.targetId || "App";
      head.append(glyph, title);
      pane.append(head);
      layer.append(pane);
    }
    previewEl.append(layer);
    return;
  }
  // Fallback under the live window (layoutThumbWindows parks the real node on top).
  const entry = shellState.windows.get(spaceId);
  if (!entry) {
    return;
  }
  previewEl.dataset.liveThumb = "true";
  previewEl.style.background = "color-mix(in srgb, var(--el-surface, #1c1c1e) 88%, #000)";
  const mini = document.createElement("div");
  mini.className = "mission-space-mini-window";
  const head = document.createElement("div");
  head.className = "mission-space-mini-head";
  const glyph = document.createElement("span");
  glyph.className = "mission-space-mini-glyph";
  mountGlyph(glyph, entry.targetId);
  const title = document.createElement("span");
  title.textContent = spaceLabel(spaceId);
  head.append(glyph, title);
  const body = document.createElement("div");
  body.className = "mission-space-mini-body";
  mini.append(head, body);
  previewEl.append(mini);
}

function clearDropTargets() {
  spacesBar
    ?.querySelectorAll(".mission-drop-target")
    .forEach((node) => node.classList.remove("mission-drop-target"));
}

function rebuildMissionHitCache() {
  if (!spacesShelf || !spacesBar) {
    missionHitCache = null;
    return null;
  }
  const thumbs = [];
  for (const thumb of spacesBar.querySelectorAll(
    ".mission-space-thumb:not(.mission-space-ghost)",
  )) {
    const spaceId = thumb.dataset.spaceId;
    if (!spaceId) {
      continue;
    }
    thumbs.push({
      spaceId,
      desktop: isDesktopSpace(spaceId),
      agent: isAgentSpace(spaceId),
      rect: thumb.getBoundingClientRect(),
    });
  }
  const addBtn = spacesBar.querySelector(".mission-space-add");
  missionHitCache = {
    shelfRect: spacesShelf.getBoundingClientRect(),
    addRect: addBtn ? addBtn.getBoundingClientRect() : null,
    thumbs,
  };
  return missionHitCache;
}

function hitMissionDrop(clientX, clientY) {
  // Geometry hit-test — never use elementFromPoint while a floor card follows
  // the cursor (that card sits above the shelf and was swallowing drops).
  // Cache rects for the drag; invalidate when the promote ghost reorders.
  const cache = missionHitCache || rebuildMissionHitCache();
  if (!cache) {
    return null;
  }
  if (!pointInRect(clientX, clientY, cache.shelfRect, 8)) {
    return null;
  }
  if (cache.addRect && pointInRect(clientX, clientY, cache.addRect, 10)) {
    return { type: "add" };
  }
  for (const thumb of cache.thumbs) {
    if (!pointInRect(clientX, clientY, thumb.rect, 6)) {
      continue;
    }
    if (thumb.agent) {
      /* Agent Space hosts no windows — not a fullscreen promote target. */
      return { type: "reject", spaceId: thumb.spaceId };
    }
    if (thumb.desktop) {
      return { type: "desktop", spaceId: thumb.spaceId };
    }
    return { type: "fullscreen", spaceId: thumb.spaceId };
  }
  return { type: "shelf" };
}

function dropWillPromote(windowId, drop) {
  if (!drop || !windowId || drop.type === "reject") {
    return false;
  }
  // Shelf, + , or an existing fullscreen thumb → dedicated fullscreen Space.
  // (Clicking + with no window still adds an empty Desktop.)
  if (drop.type === "shelf" || drop.type === "fullscreen" || drop.type === "add") {
    return true;
  }
  if (drop.type === "desktop") {
    const entry = shellState.windows.get(windowId);
    const home = entry?.desktopSpaceId || desktopStageId();
    return home === drop.spaceId;
  }
  return false;
}

function ensurePromoteGhost(windowId) {
  if (promoteGhost?.dataset.windowId === windowId && promoteGhost.isConnected) {
    return promoteGhost;
  }
  removePromoteGhost();
  const entry = shellState.windows.get(windowId);
  if (!entry || !spacesBar) {
    return null;
  }
  const ghost = document.createElement("div");
  ghost.className = "mission-space-thumb mission-space-ghost";
  ghost.dataset.windowId = windowId;
  ghost.setAttribute("aria-hidden", "true");
  const preview = document.createElement("span");
  preview.className = "mission-space-preview";
  const mini = document.createElement("div");
  mini.className = "mission-space-mini-window";
  const head = document.createElement("div");
  head.className = "mission-space-mini-head";
  const glyph = document.createElement("span");
  glyph.className = "mission-space-mini-glyph";
  mountGlyph(glyph, entry.targetId);
  const title = document.createElement("span");
  title.textContent = entry.title || entry.targetId || "App";
  head.append(glyph, title);
  const body = document.createElement("div");
  body.className = "mission-space-mini-body";
  mini.append(head, body);
  preview.append(mini);
  const label = document.createElement("span");
  label.className = "mission-space-label";
  label.textContent = entry.title || entry.targetId || "App";
  ghost.append(preview, label);
  const addBtn = spacesBar.querySelector(".mission-space-add");
  if (addBtn) {
    spacesBar.insertBefore(ghost, addBtn);
  } else {
    spacesBar.appendChild(ghost);
  }
  promoteGhost = ghost;
  return ghost;
}

/** Live-inject the promote card into the Spaces bar at the cursor slot (Mac-like). */
function placePromoteGhostAt(windowId, clientX) {
  const ghost = ensurePromoteGhost(windowId);
  if (!ghost || !spacesBar || !missionDrag) {
    return ghost;
  }
  const others = spaceThumbNodes().sort(
    (a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left,
  );
  const insertAt = insertIndexAmongOthers(others, clientX, missionDrag.promoteInsertAt ?? -1);
  const slotChanged = insertAt !== missionDrag.promoteInsertAt;
  missionDrag.promoteInsertAt = insertAt;
  const slot = Math.max(0, Math.min(insertAt, others.length));
  let order = 1;
  for (let i = 0; i <= others.length; i += 1) {
    if (i === slot) {
      ghost.style.order = String(order);
      order += 1;
    }
    if (i < others.length) {
      others[i].style.order = String(order);
      order += 1;
    }
  }
  const addBtn = spacesBar.querySelector(".mission-space-add");
  if (addBtn) {
    addBtn.style.order = String(order + 10);
  }
  ghost.classList.add("mission-space-ghost-live", "mission-space-ghost-inserting");
  if (slotChanged) {
    rebuildMissionHitCache();
  }
  return ghost;
}

function clearPromoteInsertLayout() {
  removePromoteGhost();
  if (spacesBar && active) {
    applySpacesBarFlexOrder(buildStageRing());
  }
}

function removePromoteGhost() {
  promoteGhost?.remove();
  promoteGhost = null;
}

function ensureDragProxy(windowId, startRect) {
  if (dragProxy?.dataset.windowId === windowId && dragProxy.isConnected) {
    return dragProxy;
  }
  removeDragProxy();
  const entry = shellState.windows.get(windowId);
  if (!entry || !startRect) {
    return null;
  }
  const proxy = document.createElement("div");
  proxy.className = "mission-drag-proxy";
  proxy.dataset.windowId = windowId;
  proxy.setAttribute("aria-hidden", "true");
  const head = document.createElement("div");
  head.className = "mission-drag-proxy-head";
  const glyph = document.createElement("span");
  glyph.className = "mission-space-mini-glyph";
  mountGlyph(glyph, entry.targetId);
  const title = document.createElement("span");
  title.textContent = entry.title || entry.targetId || "App";
  head.append(glyph, title);
  const body = document.createElement("div");
  body.className = "mission-drag-proxy-body";
  proxy.append(head, body);
  proxy.style.left = `${startRect.left}px`;
  proxy.style.top = `${startRect.top}px`;
  proxy.style.width = `${startRect.width}px`;
  proxy.style.height = `${startRect.height}px`;
  document.body.appendChild(proxy);
  dragProxy = proxy;
  return proxy;
}

function removeDragProxy() {
  dragProxy?.remove();
  dragProxy = null;
}

function syncDragProxy(clientX, clientY, drop) {
  if (!missionDrag?.dragging || !missionDrag.startRect) {
    return;
  }
  const proxy = ensureDragProxy(missionDrag.windowId, missionDrag.startRect);
  const node = shellState.windows.get(missionDrag.windowId)?.node;
  if (!proxy) {
    return;
  }
  if (node) {
    node.style.opacity = "0.12";
  }
  const promoting = dropWillPromote(missionDrag.windowId, drop);
  if (promoting) {
    // Inject the Space card where the cursor sits; siblings slide aside.
    const ghost = placePromoteGhostAt(missionDrag.windowId, clientX);
    const target = ghost?.querySelector(".mission-space-preview")?.getBoundingClientRect();
    if (target && target.width > 8) {
      proxy.classList.add("mission-drag-proxy-promote");
      proxy.style.transition = exposeReducedMotion()
        ? "none"
        : "left 150ms cubic-bezier(0.2, 0, 0, 1), top 150ms cubic-bezier(0.2, 0, 0, 1), width 150ms cubic-bezier(0.2, 0, 0, 1), height 150ms cubic-bezier(0.2, 0, 0, 1), border-radius 150ms ease";
      proxy.style.left = `${target.left}px`;
      proxy.style.top = `${target.top}px`;
      proxy.style.width = `${target.width}px`;
      proxy.style.height = `${target.height}px`;
      return;
    }
  }
  // Dragged off the shelf — card goes away, bar snaps back.
  if (promoteGhost) {
    clearPromoteInsertLayout();
    missionDrag.promoteInsertAt = undefined;
  }
  proxy.classList.remove("mission-drag-proxy-promote");
  proxy.style.transition = "none";
  const dx = clientX - missionDrag.x;
  const dy = clientY - missionDrag.y;
  const start = missionDrag.startRect;
  proxy.style.left = `${start.left + dx}px`;
  proxy.style.top = `${start.top + dy}px`;
  proxy.style.width = `${start.width}px`;
  proxy.style.height = `${start.height}px`;
}

function updateDropHighlight(clientX, clientY) {
  clearDropTargets();
  const drop = hitMissionDrop(clientX, clientY);
  if (!drop) {
    spacesShelf?.classList.remove("mission-drop-target");
    if (promoteGhost) {
      clearPromoteInsertLayout();
      if (missionDrag) {
        missionDrag.promoteInsertAt = undefined;
      }
    }
    return;
  }
  if (missionDrag && dropWillPromote(missionDrag.windowId, drop)) {
    spacesShelf?.classList.add("mission-drop-target");
    placePromoteGhostAt(missionDrag.windowId, clientX)?.classList.add("mission-drop-target");
    return;
  }
  if (promoteGhost) {
    clearPromoteInsertLayout();
    if (missionDrag) {
      missionDrag.promoteInsertAt = undefined;
    }
  }
  if (drop.type === "desktop" || drop.type === "fullscreen") {
    spacesBar
      ?.querySelector(`[data-space-id="${CSS.escape(drop.spaceId)}"]`)
      ?.classList.add("mission-drop-target");
  } else if (drop.type === "shelf") {
    spacesShelf?.classList.add("mission-drop-target");
  } else if (drop.type === "add") {
    spacesBar?.querySelector(".mission-space-add")?.classList.add("mission-drop-target");
  }
}

function endMissionDrag() {
  if (!missionDrag) {
    return;
  }
  const entry = shellState.windows.get(missionDrag.windowId);
  const node = entry?.node;
  if (node) {
    node.classList.remove("mission-dragging", "mission-dragging-promote");
    if (missionDrag.baseTransform != null) {
      node.style.transform = missionDrag.baseTransform;
    }
    if (missionDrag.baseTransition != null) {
      node.style.transition = missionDrag.baseTransition;
    }
    node.style.opacity = "";
    try {
      if (missionDrag.pointerId != null) {
        node.releasePointerCapture?.(missionDrag.pointerId);
      }
    } catch (_error) {
      // Capture may already be released.
    }
  }
  clearPromoteInsertLayout();
  removeDragProxy();
  document.body.classList.remove("mission-drag-space");
  spacesShelf?.classList.remove("mission-drop-target");
  clearDropTargets();
  missionHitCache = null;
  missionDrag = null;
}

function updateMissionDragVisual(clientX, clientY) {
  if (!missionDrag?.dragging) {
    return;
  }
  const node = shellState.windows.get(missionDrag.windowId)?.node;
  if (!node) {
    return;
  }
  const drop = hitMissionDrop(clientX, clientY);
  const base = missionDrag.baseTransform || "none";
  // Keep the real floor card parked; the fixed proxy carries the gesture.
  node.style.transition = "none";
  node.style.transform = base === "none" ? "" : base;
  syncDragProxy(clientX, clientY, drop);
}

/** Create fullscreen Space but stay in Mission Control so the user can keep organizing. */
function promoteAndStayInOverview(windowId, insertAt = null) {
  const entry = shellState.windows.get(windowId);
  const node = entry?.node;
  // Minimized floor cards must become a live fullscreen Space, not a hidden
  // window parked on a new Desktop.
  if (node?.classList.contains("hidden")) {
    node.classList.remove("hidden");
    node.setAttribute("aria-hidden", "false");
  }
  if (originals.has(windowId)) {
    originals.get(windowId).wasMinimized = false;
  }
  if (node) {
    delete node.dataset.exposeMinimized;
  }
  if (!promoteWindowToFullscreenSpace(windowId)) {
    return false;
  }
  // Land in the slot the ghost was previewing along the Spaces bar.
  if (insertAt != null && Number.isFinite(insertAt)) {
    moveSpaceInRing(windowId, insertAt);
  }
  // Promote activates that Space — return the organizing surface to Desktop.
  setActiveStage(desktopStageId(), { announce: false, focus: false, animate: false });
  selectedSpaceId = desktopStageId();
  return true;
}

function applyMissionDrop(windowId, drop, insertAt = null) {
  if (!drop || !windowId || drop.type === "reject") {
    return false;
  }
  // Drag to + / shelf / fullscreen thumb → dedicated fullscreen Space (Mac).
  // Empty Desktop creation stays on the + button click (no window).
  if (drop.type === "add" || drop.type === "shelf" || drop.type === "fullscreen") {
    return promoteAndStayInOverview(windowId, insertAt);
  }
  if (drop.type === "desktop") {
    const entry = shellState.windows.get(windowId);
    const home = entry?.desktopSpaceId || desktopStageId();
    if (home === drop.spaceId) {
      return promoteAndStayInOverview(windowId, insertAt);
    }
    // Moving onto another Desktop: restore if it was only shown minimized in MC.
    if (originals.has(windowId)) {
      originals.get(windowId).wasMinimized = false;
    }
    if (entry?.node) {
      entry.node.classList.remove("hidden");
      entry.node.setAttribute("aria-hidden", "false");
      delete entry.node.dataset.exposeMinimized;
    }
    assignWindowToDesktop(windowId, drop.spaceId);
    selectedSpaceId = drop.spaceId;
    return true;
  }
  return false;
}

function spaceThumbNodes() {
  return [...(spacesBar?.querySelectorAll(".mission-space-thumb:not(.mission-space-ghost)") || [])];
}

/** Other thumbs in left-to-right visual order (flex `order` may differ from DOM). */
function reorderOtherThumbs(draggedId) {
  return spaceThumbNodes()
    .filter((thumb) => thumb.dataset.spaceId !== draggedId)
    .sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left);
}

function insertIndexAmongOthers(others, clientX, lastInsertAt = -1) {
  let index = others.length;
  for (let i = 0; i < others.length; i += 1) {
    const rect = others[i].getBoundingClientRect();
    if (clientX < rect.left + rect.width / 2) {
      index = i;
      break;
    }
  }
  if (lastInsertAt >= 0 && lastInsertAt !== index && others.length > 0) {
    const band = 16;
    if (index > lastInsertAt) {
      const gateRect = others[Math.min(lastInsertAt, others.length - 1)].getBoundingClientRect();
      if (clientX < gateRect.left + gateRect.width / 2 + band) {
        return lastInsertAt;
      }
    } else {
      const gateRect = others[Math.max(index, 0)].getBoundingClientRect();
      if (clientX > gateRect.left + gateRect.width / 2 - band) {
        return lastInsertAt;
      }
    }
  }
  return index;
}

function ensureReorderSpacer(widthPx, heightPx) {
  let spacer = spacesBar?.querySelector(".mission-reorder-spacer");
  if (!spacer) {
    spacer = document.createElement("div");
    spacer.className = "mission-reorder-spacer";
    spacer.setAttribute("aria-hidden", "true");
  }
  spacer.style.flex = `0 0 ${Math.round(widthPx)}px`;
  spacer.style.width = `${Math.round(widthPx)}px`;
  spacer.style.height = `${Math.round(heightPx)}px`;
  if (spacesBar && spacer.parentNode !== spacesBar) {
    spacesBar.appendChild(spacer);
  }
  return spacer;
}

/** Place the gap by flex order so siblings slide between any slots (not only ends). */
function placeReorderSpacer(spacer, insertAt, draggedId) {
  if (!spacesBar || !spacer) {
    return;
  }
  if (spacer.parentNode !== spacesBar) {
    spacesBar.appendChild(spacer);
  }
  const others = reorderOtherThumbs(draggedId);
  const slot = Math.max(0, Math.min(insertAt, others.length));
  let order = 1;
  for (let i = 0; i <= others.length; i += 1) {
    if (i === slot) {
      spacer.style.order = String(order);
      order += 1;
    }
    if (i < others.length) {
      others[i].style.order = String(order);
      order += 1;
    }
  }
  const addBtn = spacesBar.querySelector(".mission-space-add");
  if (addBtn) {
    addBtn.style.order = String(order + 10);
  }
}

function makeReorderGhost(btn, rect) {
  const ghost = document.createElement("div");
  ghost.className = "mission-space-thumb mission-space-reorder-ghost";
  ghost.setAttribute("aria-hidden", "true");
  ghost.style.position = "fixed";
  ghost.style.left = `${rect.left}px`;
  ghost.style.top = `${rect.top}px`;
  ghost.style.width = `${rect.width}px`;
  ghost.style.zIndex = "190140";
  ghost.style.margin = "0";
  ghost.style.pointerEvents = "none";
  const preview = document.createElement("span");
  preview.className = "mission-space-preview";
  const spaceId = btn.dataset.spaceId || "";
  paintSpacePreview(preview, spaceId);
  const label = document.createElement("span");
  label.className = "mission-space-label";
  label.textContent = spaceLabel(spaceId);
  ghost.append(preview, label);
  document.body.appendChild(ghost);
  return ghost;
}

/**
 * Open a spacer gap and drag a lightweight ghost.
 * Live thumbs (with iframes) stay put and hidden — never transformed/fixed.
 */
function beginSpaceReorderLift(btn, gesture) {
  const rect = btn.getBoundingClientRect();
  const spacer = ensureReorderSpacer(rect.width, btn.offsetHeight || rect.height);
  gesture.spacer = spacer;
  gesture.liftLeft = rect.left;
  gesture.liftTop = rect.top;
  gesture.liftWidth = rect.width;
  gesture.hasLiveThumb = Boolean(btn.querySelector(".mission-space-live-layer"));
  btn.classList.add("mission-space-reordering");
  document.body.classList.add("mission-reorder-space");
  // Always park the source thumb out of flow; drag ghost or fixed clone UI.
  btn.style.position = "absolute";
  btn.style.opacity = "0";
  btn.style.pointerEvents = "none";
  btn.style.left = "0";
  btn.style.top = "0";
  btn.style.order = "0";
  if (gesture.hasLiveThumb) {
    gesture.ghost = makeReorderGhost(btn, rect);
  } else {
    // Non-live: fixed lift of the real thumb (no iframe to blank).
    btn.style.position = "fixed";
    btn.style.opacity = "1";
    btn.style.left = `${rect.left}px`;
    btn.style.top = `${rect.top}px`;
    btn.style.width = `${rect.width}px`;
    btn.style.margin = "0";
    btn.style.zIndex = "190140";
    btn.style.transition = "none";
    btn.style.transform = "none";
  }
}

function endSpaceReorderLift(btn, gesture) {
  gesture?.spacer?.remove();
  gesture?.ghost?.remove();
  if (gesture) {
    gesture.spacer = null;
    gesture.ghost = null;
  }
  btn.classList.remove("mission-space-reordering");
  document.body.classList.remove("mission-reorder-space");
  btn.style.opacity = "";
  btn.style.position = "";
  btn.style.left = "";
  btn.style.top = "";
  btn.style.width = "";
  btn.style.margin = "";
  btn.style.zIndex = "";
  btn.style.pointerEvents = "";
  btn.style.transition = "";
  btn.style.transform = "";
}

function bindSpaceThumbGesture(btn, spaceId) {
  btn.addEventListener("pointerdown", (event) => {
    if (!active || event.button !== 0) {
      return;
    }
    if (event.target.closest(".mission-space-close")) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    spaceThumbGesture = {
      spaceId,
      x: event.clientX,
      y: event.clientY,
      dragging: false,
      pointerId: event.pointerId,
      lastInsertAt: -1,
      spacer: null,
      liftLeft: 0,
      liftTop: 0,
      liftWidth: 0,
    };
    try {
      btn.setPointerCapture?.(event.pointerId);
    } catch (_error) {
      // Optional.
    }
  });
  btn.addEventListener("pointermove", (event) => {
    if (!spaceThumbGesture || spaceThumbGesture.spaceId !== spaceId) {
      return;
    }
    if (event.pointerId !== spaceThumbGesture.pointerId) {
      return;
    }
    const dx = event.clientX - spaceThumbGesture.x;
    const dy = event.clientY - spaceThumbGesture.y;
    if (
      !spaceThumbGesture.dragging &&
      dx * dx + dy * dy >= MISSION_DRAG_THRESHOLD_PX * MISSION_DRAG_THRESHOLD_PX
    ) {
      spaceThumbGesture.dragging = true;
      beginSpaceReorderLift(btn, spaceThumbGesture);
      spaceThumbGesture.lastInsertAt = insertIndexAmongOthers(
        reorderOtherThumbs(spaceId),
        event.clientX,
      );
      placeReorderSpacer(spaceThumbGesture.spacer, spaceThumbGesture.lastInsertAt, spaceId);
    }
    if (!spaceThumbGesture.dragging) {
      return;
    }
    const followLeft = spaceThumbGesture.liftLeft + dx;
    const followTop = spaceThumbGesture.liftTop + Math.max(-10, Math.min(10, dy));
    if (spaceThumbGesture.ghost) {
      spaceThumbGesture.ghost.style.left = `${followLeft}px`;
      spaceThumbGesture.ghost.style.top = `${followTop}px`;
    } else {
      btn.style.left = `${followLeft}px`;
      btn.style.top = `${followTop}px`;
    }
    const others = reorderOtherThumbs(spaceId);
    const insertAt = insertIndexAmongOthers(others, event.clientX, spaceThumbGesture.lastInsertAt);
    if (insertAt !== spaceThumbGesture.lastInsertAt) {
      spaceThumbGesture.lastInsertAt = insertAt;
      placeReorderSpacer(spaceThumbGesture.spacer, insertAt, spaceId);
    }
  });
  const endThumbGesture = (event) => {
    if (!spaceThumbGesture || spaceThumbGesture.spaceId !== spaceId) {
      return;
    }
    if (event && event.pointerId !== spaceThumbGesture.pointerId) {
      return;
    }
    const gesture = spaceThumbGesture;
    spaceThumbGesture = null;
    try {
      btn.releasePointerCapture?.(gesture.pointerId);
    } catch (_error) {
      // Optional.
    }
    if (gesture.dragging) {
      suppressExposeClick = true;
      queueMicrotask(() => {
        suppressExposeClick = false;
      });
      const insertAt =
        gesture.lastInsertAt >= 0
          ? gesture.lastInsertAt
          : insertIndexAmongOthers(reorderOtherThumbs(spaceId), event?.clientX ?? gesture.x);
      if (moveSpaceInRing(spaceId, insertAt)) {
        selectedSpaceId = spaceId;
      }
      // Flex order only — keeps live Documents seated without DOM reshuffles.
      applySpacesBarFlexOrder(buildStageRing());
      const wasLive = gesture.hasLiveThumb;
      endSpaceReorderLift(btn, gesture);
      if (wasLive) {
        btn.style.opacity = "0";
        requestAnimationFrame(() => {
          btn.style.opacity = "";
          syncSpacesBarSelectionState();
        });
      } else {
        syncSpacesBarSelectionState();
      }
      return;
    }
    endSpaceReorderLift(btn, gesture);
    // Mac: click another Space = preview; click the current Space = enter it.
    if (spaceId === selectedSpaceId) {
      confirmMissionSpace(spaceId);
    } else {
      selectMissionSpace(spaceId);
    }
  };
  btn.addEventListener("pointerup", endThumbGesture);
  btn.addEventListener("pointercancel", endThumbGesture);
}

/** Visual Space order via flex `order` only — safe for live fullscreen iframes. */
function applySpacesBarFlexOrder(ring = buildStageRing()) {
  const bar = spacesBar || ensureSpacesBar();
  bar.querySelector(".mission-reorder-spacer")?.remove();
  ring.forEach((spaceId, index) => {
    const thumb = bar.querySelector(
      `.mission-space-thumb[data-space-id="${CSS.escape(spaceId)}"]`,
    );
    if (thumb) {
      thumb.style.order = String(index + 1);
    }
  });
  const addBtn = bar.querySelector(".mission-space-add");
  if (addBtn) {
    addBtn.style.order = String(ring.length + 10);
  }
}

function syncSpacesBarSelectionState() {
  if (!ringIncludesSelected()) {
    selectedSpaceId = getActiveStageId();
    if (!ringIncludesSelected()) {
      selectedSpaceId = desktopStageId();
    }
  }
  spaceThumbIndex = Math.max(0, buildStageRing().indexOf(selectedSpaceId));
  spaceThumbNodes().forEach((thumb) => {
    const id = thumb.dataset.spaceId;
    const selected = id === selectedSpaceId;
    thumb.setAttribute("aria-selected", selected ? "true" : "false");
    thumb.tabIndex = selected ? 0 : -1;
    if (selected) {
      thumb.setAttribute("aria-current", "true");
    } else {
      thumb.removeAttribute("aria-current");
    }
  });
}

function ringIncludesSelected() {
  return buildStageRing().includes(selectedSpaceId);
}

/** Persist visual Space order without destroying live thumb hosts / iframes. */
function commitSpacesBarReorder() {
  applySpacesBarFlexOrder(buildStageRing());
  syncSpacesBarSelectionState();
}

function renderSpacesBar() {
  const bar = ensureSpacesBar();
  const ring = buildStageRing();
  syncMissionSpaceMetrics(ring.length);
  if (!ring.includes(selectedSpaceId)) {
    selectedSpaceId = getActiveStageId();
    if (!ring.includes(selectedSpaceId)) {
      selectedSpaceId = desktopStageId();
    }
  }
  spaceThumbIndex = Math.max(0, ring.indexOf(selectedSpaceId));
  const ringKey = ring.join("|");
  // Membership unchanged — flex order + selection only (keeps live iframes seated).
  if (
    ringKey === lastSpacesBarRingKey &&
    bar.querySelectorAll(".mission-space-thumb:not(.mission-space-ghost)").length === ring.length
  ) {
    applySpacesBarFlexOrder(ring);
    syncSpacesBarSelectionState();
    return;
  }
  lastSpacesBarRingKey = ringKey;
  // Reclaim live iframes before wiping the bar (replaceChildren would detach them).
  reclaimLiveThumbWindows();
  bar.replaceChildren();
  ring.forEach((spaceId, index) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mission-space-thumb";
    btn.dataset.spaceId = spaceId;
    btn.style.order = String(index + 1);
    btn.setAttribute("role", "tab");
    btn.setAttribute("aria-selected", spaceId === selectedSpaceId ? "true" : "false");
    if (spaceId === selectedSpaceId) {
      btn.setAttribute("aria-current", "true");
    }
    btn.tabIndex = spaceId === selectedSpaceId ? 0 : -1;
    // aria-label only — native title tooltips stack on the visible label.
    btn.setAttribute("aria-label", spaceLabel(spaceId));
    const preview = document.createElement("span");
    preview.className = "mission-space-preview";
    preview.setAttribute("aria-hidden", "true");
    paintSpacePreview(preview, spaceId);
    if (canRemoveDesktopSpace(spaceId)) {
      const closeBtn = document.createElement("button");
      closeBtn.type = "button";
      closeBtn.className = "mission-space-close";
      closeBtn.title = "Close Desktop";
      closeBtn.setAttribute("aria-label", `Close ${spaceLabel(spaceId)}`);
      closeBtn.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      closeBtn.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!removeDesktopSpace(spaceId)) {
          return;
        }
        if (selectedSpaceId === spaceId) {
          selectedSpaceId = desktopStageId();
        }
        renderSpacesBar();
        layoutMissionFloor();
      });
      preview.appendChild(closeBtn);
    }
    const label = document.createElement("span");
    label.className = "mission-space-label";
    label.textContent = spaceLabel(spaceId);
    btn.append(preview, label);
    bindSpaceThumbGesture(btn, spaceId);
    bar.appendChild(btn);
  });

  const addBtn = document.createElement("button");
  addBtn.type = "button";
  addBtn.className = "mission-space-add";
  addBtn.title = "New Desktop";
  addBtn.setAttribute("aria-label", "New Desktop");
  addBtn.style.order = String(ring.length + 10);
  const addGlyph = document.createElement("span");
  addGlyph.className = "mission-space-add-glyph";
  addGlyph.setAttribute("aria-hidden", "true");
  addBtn.append(addGlyph);
  addBtn.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    const id = addDesktopSpace();
    selectMissionSpace(id);
  });
  bar.appendChild(addBtn);
}

/** Floor cards: transform/opacity only (Mac zoom). Never touch left/top/size. */
function rememberFloorOriginal(windowId, node, wasMinimized) {
  if (originals.has(windowId)) {
    return originals.get(windowId);
  }
  const saved = {
    transform: node.style.transform,
    transition: node.style.transition,
    zIndex: node.style.zIndex,
    boxShadow: node.style.boxShadow,
    opacity: node.style.opacity,
    wasMinimized: Boolean(wasMinimized),
    thumbOnly: false,
  };
  originals.set(windowId, saved);
  return saved;
}

/** Fullscreen Space thumbs: may reparent + temporarily rewrite geometry. */
function rememberThumbOriginal(windowId, node, wasMinimized) {
  if (originals.has(windowId)) {
    return originals.get(windowId);
  }
  const saved = {
    transform: node.style.transform,
    transition: node.style.transition,
    zIndex: node.style.zIndex,
    boxShadow: node.style.boxShadow,
    opacity: node.style.opacity,
    borderRadius: node.style.borderRadius,
    overflow: node.style.overflow,
    left: node.style.left,
    top: node.style.top,
    width: node.style.width,
    height: node.style.height,
    inset: node.style.inset,
    minWidth: node.style.minWidth,
    minHeight: node.style.minHeight,
    position: node.style.position,
    transformOrigin: node.style.transformOrigin,
    thumbSrcW: node.style.getPropertyValue("--mission-thumb-src-w"),
    thumbSrcH: node.style.getPropertyValue("--mission-thumb-src-h"),
    thumbScale: node.style.getPropertyValue("--mission-thumb-scale"),
    parent: node.parentNode,
    nextSibling: node.nextSibling,
    wasMinimized: Boolean(wasMinimized),
    thumbOnly: true,
  };
  originals.set(windowId, saved);
  return saved;
}

function ensureThumbLiveLayer(previewEl) {
  let layer = previewEl.querySelector(":scope > .mission-space-live-layer");
  if (!layer) {
    layer = document.createElement("div");
    layer.className = "mission-space-live-layer";
    layer.setAttribute("aria-hidden", "true");
    previewEl.appendChild(layer);
  }
  return layer;
}

function settleSpacesShelfForMeasure() {
  if (!spacesShelf) {
    return;
  }
  spacesShelf.style.animation = "none";
  spacesShelf.style.transform = "translateY(0)";
  spacesShelf.style.opacity = "1";
}

/** Match Space thumb aspect to the live viewport so scaled fullscreen apps
 *  fill the card (no letterbox bar). Prefer fitting the strip to full width. */
function syncMissionSpaceMetrics(spaceCount = 0) {
  const stageW = Math.max(window.innerWidth || 1, 1);
  const stageH = Math.max(window.innerHeight || 1, 1);
  const aspect = stageW / stageH;
  // Aim ~102px preview height; widen the card so aspect matches the display.
  const previewH = 102;
  let thumbW = Math.round(Math.min(240, Math.max(160, previewH * aspect)));
  // Shrink thumbs so Spaces (+ add) use the full display width before scrolling.
  const slots = Math.max(1, spaceCount);
  const gap = 18;
  const sidePad = 56;
  const addSlot = 44 + gap;
  const fitW = Math.floor((stageW - sidePad - addSlot - gap * (slots - 1)) / slots);
  if (Number.isFinite(fitW) && fitW > 0) {
    thumbW = Math.round(Math.min(thumbW, Math.max(108, fitW)));
  }
  const thumbH = thumbW / aspect;
  // Include hover-lift + label padding so the shelf does not clip thumbs.
  const shelfH = Math.round(thumbH + 92);
  document.body.style.setProperty("--mission-space-aspect", String(aspect));
  document.body.style.setProperty("--mission-thumb-w", `${thumbW}px`);
  document.body.style.setProperty("--mission-shelf-h", `${Math.max(188, shelfH)}px`);
  return { stageW, stageH, aspect, thumbW };
}

/**
 * Fullscreen Space thumbs: keep real window at desktop size and scale into the
 * preview. CSS vars + !important beat the fullscreen inset:0 rule that otherwise
 * reflows the app into the tiny thumb (zoomed/cropped look).
 */
function placeFullscreenWindowInThumb(node, host, stageW, stageH) {
  node.classList.add("mission-thumb-card");
  node.dataset.stageActive = "false";
  node.dataset.spaceVisible = "true";
  node.style.opacity = "1";
  node.style.visibility = "visible";
  node.style.transition = "none";
  node.style.boxShadow = "none";
  if (node.parentNode !== host) {
    host.appendChild(node);
  }
  // Prefer the sized thumb metrics; fall back to measured host.
  const hostRect = host.getBoundingClientRect();
  const hostW = Math.max(hostRect.width || host.clientWidth || 1, 1);
  const hostH = Math.max(hostRect.height || host.clientHeight || 1, 1);
  // Aspect-matched thumbs make these equal; min keeps the whole desktop in-frame.
  const scale = Math.min(hostW / stageW, hostH / stageH);
  node.style.setProperty("--mission-thumb-src-w", `${Math.round(stageW)}px`);
  node.style.setProperty("--mission-thumb-src-h", `${Math.round(stageH)}px`);
  node.style.setProperty("--mission-thumb-scale", `scale(${scale})`);
}

function layoutThumbWindows(ring = buildStageRing()) {
  settleSpacesShelfForMeasure();
  const { stageW, stageH } = syncMissionSpaceMetrics();

  for (const spaceId of ring) {
    if (spaceId === selectedSpaceId) {
      continue;
    }
    // Desktop + Agent: schematic preview only (paintSpacePreview).
    if (isDesktopSpace(spaceId) || isAgentSpace(spaceId)) {
      continue;
    }

    const thumbBtn = spacesBar?.querySelector(`[data-space-id="${CSS.escape(spaceId)}"]`);
    const thumb = thumbBtn?.querySelector(".mission-space-preview");
    if (!thumb) {
      continue;
    }
    const entry = shellState.windows.get(spaceId);
    if (!entry?.node) {
      continue;
    }
    const node = entry.node;
    const existing = originals.get(spaceId);
    if (existing && !existing.thumbOnly) {
      continue;
    }
    const wasMinimized = node.classList.contains("hidden");
    if (!existing) {
      rememberThumbOriginal(spaceId, node, wasMinimized);
      if (wasMinimized) {
        node.classList.remove("hidden");
        node.setAttribute("aria-hidden", "false");
        node.dataset.exposeMinimized = "true";
      } else {
        delete node.dataset.exposeMinimized;
      }
    }
    const host = ensureThumbLiveLayer(thumb);
    node.dataset.expose = spaceId;
    placeFullscreenWindowInThumb(node, host, stageW, stageH);
  }
}

function restoreFloorWindow(node, style) {
  node.style.transform = style.transform || "";
  node.style.transition = style.transition || "";
  node.style.zIndex = style.zIndex || "";
  node.style.boxShadow = style.boxShadow || "";
  node.style.opacity = style.opacity || "";
  node.style.visibility = "";
  node.classList.remove("expose-card", "expose-card-active", "mission-thumb-card", "mission-dragging");
  node.removeAttribute("data-expose");
  node.removeAttribute("data-expose-grid-transform");
  node.removeAttribute("role");
  node.removeAttribute("aria-selected");
  clearExposeCaption(node);
  // Only trust the snapshot from open — stale data-expose-minimized used to
  // re-hide every Desktop window after leaving Mission Control.
  if (style.wasMinimized) {
    node.classList.add("hidden");
    node.classList.remove("window-active");
    node.setAttribute("aria-hidden", "true");
  }
  delete node.dataset.exposeMinimized;
}

function restoreThumbWindow(node, style) {
  if (style.parent && node.parentNode !== style.parent) {
    if (style.nextSibling && style.nextSibling.parentNode === style.parent) {
      style.parent.insertBefore(node, style.nextSibling);
    } else {
      style.parent.appendChild(node);
    }
  }
  node.style.transform = style.transform || "";
  node.style.transition = style.transition || "";
  node.style.zIndex = style.zIndex || "";
  node.style.boxShadow = style.boxShadow || "";
  node.style.opacity = style.opacity || "";
  node.style.borderRadius = style.borderRadius || "";
  node.style.overflow = style.overflow || "";
  node.style.left = style.left || "";
  node.style.top = style.top || "";
  node.style.width = style.width || "";
  node.style.height = style.height || "";
  node.style.inset = style.inset || "";
  node.style.minWidth = style.minWidth || "";
  node.style.minHeight = style.minHeight || "";
  node.style.position = style.position || "";
  node.style.transformOrigin = style.transformOrigin || "";
  if (style.thumbSrcW) {
    node.style.setProperty("--mission-thumb-src-w", style.thumbSrcW);
  } else {
    node.style.removeProperty("--mission-thumb-src-w");
  }
  if (style.thumbSrcH) {
    node.style.setProperty("--mission-thumb-src-h", style.thumbSrcH);
  } else {
    node.style.removeProperty("--mission-thumb-src-h");
  }
  if (style.thumbScale) {
    node.style.setProperty("--mission-thumb-scale", style.thumbScale);
  } else {
    node.style.removeProperty("--mission-thumb-scale");
  }
  node.style.visibility = "";
  node.classList.remove("expose-card", "expose-card-active", "mission-thumb-card", "mission-dragging");
  node.removeAttribute("data-expose");
  node.removeAttribute("role");
  node.removeAttribute("aria-selected");
  clearExposeCaption(node);
  if (style.wasMinimized) {
    node.classList.add("hidden");
    node.classList.remove("window-active");
    node.setAttribute("aria-hidden", "true");
  }
  delete node.dataset.exposeMinimized;
}

/** Pull live Space-thumb windows out before the bar DOM is destroyed. */
function reclaimLiveThumbWindows() {
  const host = document.querySelector("#desktop");
  if (!host || !spacesBar) {
    return;
  }
  for (const layer of spacesBar.querySelectorAll(".mission-space-live-layer")) {
    for (const node of [...layer.querySelectorAll(".window")]) {
      host.appendChild(node);
    }
  }
}

/** If a prior bug left a window parented off #desktop, put it back — no geometry. */
function reparentOrphanWindows() {
  const host = document.querySelector("#desktop");
  if (!host) {
    return;
  }
  for (const entry of shellState.windows.values()) {
    const node = entry?.node;
    if (!node) {
      continue;
    }
    if (!host.contains(node)) {
      host.appendChild(node);
    }
    node.classList.remove("mission-thumb-card", "expose-card", "expose-card-active", "mission-dragging");
    node.removeAttribute("data-expose");
    node.removeAttribute("data-expose-grid-transform");
    delete node.dataset.exposeMinimized;
    node.style.visibility = "";
    node.style.removeProperty("--mission-thumb-src-w");
    node.style.removeProperty("--mission-thumb-src-h");
    node.style.removeProperty("--mission-thumb-scale");
  }
}

function clearFloorCards() {
  reclaimLiveThumbWindows();
  for (const [windowId, style] of originals) {
    const entry = shellState.windows.get(windowId);
    if (!entry?.node) {
      continue;
    }
    if (style.thumbOnly) {
      restoreThumbWindow(entry.node, style);
    } else {
      restoreFloorWindow(entry.node, style);
    }
  }
  originals.clear();
  orderedIds = [];
  activeIndex = 0;
  spacesBar?.querySelectorAll(".mission-space-live-layer").forEach((layer) => layer.remove());
}

function layoutMissionFloor() {
  clearFloorCards();
  const entries = floorEntriesForSpace(selectedSpaceId);
  if (entries.length === 0) {
    showExposeEmpty(true);
    const hint = emptyNode?.querySelector(".expose-empty-hint");
    const title = emptyNode?.querySelector(".expose-empty-title");
    if (isAgentSpace(selectedSpaceId)) {
      if (title) title.textContent = "Agent";
      if (hint) {
        hint.textContent =
          "Your private AI Space. Click Agent above or press Enter to open it — flick here anytime from Desktop.";
      }
    } else if (isDesktopSpace(selectedSpaceId)) {
      if (title) title.textContent = `No windows on ${spaceLabel(selectedSpaceId)}`;
      if (hint) {
        hint.textContent =
          "Drag a window onto the Spaces bar to make a fullscreen Space, or tap + for a new Desktop. Click a Space above to open it.";
      }
    } else {
      if (title) title.textContent = spaceLabel(selectedSpaceId);
      if (hint) hint.textContent = "Click the Space above or press Enter to open it.";
    }
    layoutThumbWindows(buildStageRing());
    return;
  }
  showExposeEmpty(false);
  const focusId = shellState.activeWindowId;
  orderedIds = layoutExposeGrid(entries);
  const focusAt = focusId ? orderedIds.indexOf(focusId) : -1;
  activeIndex = focusAt >= 0 ? focusAt : 0;
  layoutThumbWindows(buildStageRing());
}

function selectMissionSpace(spaceId) {
  selectedSpaceId = spaceId || desktopStageId();
  settleSpacesShelfForMeasure();
  renderSpacesBar();
  layoutMissionFloor();
  requestAnimationFrame(() => {
    if (active) {
      layoutThumbWindows();
    }
  });
}

function gridSpec(count) {
  const cols = Math.max(1, Math.ceil(Math.sqrt(count)));
  const rows = Math.max(1, Math.ceil(count / cols));
  return { cols, rows };
}

/**
 * Assign each desktop window to the nearest free Mission Control cell so
 * left stays left / right stays right (no crossing on enter/exit).
 * Greedy global nearest-pair — works for 2…N windows without a full solver.
 */
function assignEntriesToNearestCells(entries, cells) {
  if (entries.length <= 1 || cells.length === 0) {
    return entries.slice();
  }
  const items = entries.map((entry, wi) => {
    const node = entry?.node;
    const rect =
      entry?.fullscreenStage || !node
        ? null
        : cardSourceRect(node);
    const x = rect ? rect.left + rect.width / 2 : cells[Math.min(wi, cells.length - 1)].x;
    const y = rect ? rect.top + rect.height / 2 : cells[Math.min(wi, cells.length - 1)].y;
    return { entry, wi, x, y };
  });
  const pairs = [];
  for (const item of items) {
    for (let ci = 0; ci < cells.length; ci += 1) {
      const dx = item.x - cells[ci].x;
      const dy = item.y - cells[ci].y;
      pairs.push({ wi: item.wi, ci, d: dx * dx + dy * dy });
    }
  }
  pairs.sort((a, b) => a.d - b.d || a.ci - b.ci || a.wi - b.wi);
  const usedWindows = new Set();
  const usedCells = new Set();
  const byCell = new Array(cells.length);
  for (const pair of pairs) {
    if (usedWindows.has(pair.wi) || usedCells.has(pair.ci)) {
      continue;
    }
    usedWindows.add(pair.wi);
    usedCells.add(pair.ci);
    byCell[pair.ci] = items[pair.wi].entry;
    if (usedWindows.size >= items.length) {
      break;
    }
  }
  // Fill any gaps (shouldn't happen) in stable entry order.
  const ordered = [];
  for (let ci = 0; ci < cells.length; ci += 1) {
    if (byCell[ci]) {
      ordered.push(byCell[ci]);
    }
  }
  for (const item of items) {
    if (!usedWindows.has(item.wi)) {
      ordered.push(item.entry);
    }
  }
  return ordered;
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
    '<p class="expose-empty-hint">Open an app from the Shelf or Search, then try Overview again.</p>';
  const stage = document.querySelector("#desktop") || document.body;
  stage.appendChild(emptyNode);
  return emptyNode;
}

function showExposeEmpty(visible) {
  const node = ensureEmptyNode();
  node.hidden = !visible;
}

function setChromeInert(inert) {
  // Menubar hides in Mission Control; Dock stays interactive for app switching.
  if (!chromeNodes) {
    chromeNodes = [document.querySelector("header.toolbar")].filter(Boolean);
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

function dockReservePx() {
  const dock = document.querySelector("footer.taskbar");
  if (!dock) {
    return 72;
  }
  const rect = dock.getBoundingClientRect();
  return Math.max(56, Math.ceil(rect.height + 16));
}

function captureWindowStartRects() {
  const map = new Map();
  for (const entry of exposeEntries()) {
    if (!entry?.node) {
      continue;
    }
    map.set(entry.id, entry.node.getBoundingClientRect());
  }
  return map;
}

/** FLIP from pre-MC window rects into floor cards (Mac enter motion).
 *  Always ends on the grid transform — never leave cards at transform:none
 *  (that reads as a cluttered desktop pile instead of Mission Control). */
function playMissionEnterMotion(startRects) {
  const floorNodes = floorEntriesForSpace(selectedSpaceId)
    .map((entry) => entry?.node)
    .filter((node) => node?.classList.contains("expose-card"));
  const ensureGrid = () => {
    for (const node of floorNodes) {
      const finalTransform = node.dataset.exposeGridTransform;
      if (finalTransform && active && node.classList.contains("expose-card")) {
        node.style.transform = finalTransform;
      }
    }
  };
  if (exposeReducedMotion() || !startRects?.size) {
    ensureGrid();
    return;
  }
  for (const [windowId, first] of startRects) {
    const entry = shellState.windows.get(windowId);
    const node = entry?.node;
    // Floor cards only — Space thumbs use geometry placement (no transform FLIP).
    if (!node || !node.classList.contains("expose-card")) {
      continue;
    }
    const finalTransform = node.dataset.exposeGridTransform || node.style.transform || "";
    if (!finalTransform || finalTransform === "none") {
      continue;
    }
    flipRectMotion(node, first, {
      toTransform: finalTransform,
      durationMs: 320,
      easing: "cubic-bezier(0.2, 0, 0, 1)",
      isStillValid: () => active && node.classList.contains("expose-card"),
    });
  }
  // Belt-and-braces: if the animation frame was skipped, still show the grid.
  window.clearTimeout(ensureGridTimer);
  ensureGridTimer = window.setTimeout(ensureGrid, 360);
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
  // Use the viewport — #desktop's box can be shorter than the display and
  // produced near-zero translates (windows stayed piled like the normal desk).
  const stageW = Math.max(window.innerWidth || 1, 1);
  const stageH = Math.max(window.innerHeight || 1, 1);
  const stageRect = { left: 0, top: 0, width: stageW, height: stageH };
  const pad = 28;
  const captionReserve = 28;
  // Keep floor cards below the Spaces shelf and above the visible Dock.
  const topPad = missionShelfHeight() + 12;
  const bottomPad = dockReservePx();
  const { cols, rows } = gridSpec(entries.length);
  const usableH = Math.max(160, stageRect.height - topPad - bottomPad - pad - captionReserve);
  const cellW = (stageRect.width - pad * (cols + 1)) / cols;
  const cellH = (usableH - pad * (rows - 1)) / rows;

  // Cell centers in reading order — windows bind to the nearest free cell so
  // a left desk window doesn't fly to the right card (and cross on the way).
  const cells = entries.map((_, index) => {
    const col = index % cols;
    const row = Math.floor(index / cols);
    return {
      index,
      col,
      row,
      x: stageRect.left + pad + col * (cellW + pad) + cellW / 2,
      y: stageRect.top + topPad + row * (cellH + pad) + cellH / 2,
    };
  });
  const ordered = assignEntriesToNearestCells(entries, cells);

  ordered.forEach((entry, index) => {
    const node = entry.node;
    const wasMinimized = node.classList.contains("hidden");
    // Fullscreen Spaces share the Desktop floor zoom: stay viewport-sized, then
    // translate+scale into the cell (same math as a single desktop window).
    if (entry.fullscreenStage) {
      node.dataset.fullscreenStage = "true";
      node.dataset.stageActive = "true";
      node.dataset.spaceVisible = "true";
    }
    const rect = entry.fullscreenStage
      ? {
          left: 0,
          top: 0,
          width: stageW,
          height: stageH,
          right: stageW,
          bottom: stageH,
        }
      : cardSourceRect(node);
    rememberFloorOriginal(entry.id, node, wasMinimized);

    if (wasMinimized) {
      node.classList.remove("hidden");
      node.setAttribute("aria-hidden", "false");
      node.dataset.exposeMinimized = "true";
    } else {
      delete node.dataset.exposeMinimized;
    }

    const cell = cells[index];
    const targetX = cell.x;
    const targetY = cell.y;
    const scale =
      Math.min(cellW / Math.max(rect.width, 1), cellH / Math.max(rect.height, 1), 1) * 0.88;
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const dx = targetX - centerX;
    const dy = targetY - centerY;
    const gridTransform = `translate(${dx}px, ${dy}px) scale(${scale})`;

    node.classList.add("expose-card");
    node.classList.remove("mission-thumb-card");
    node.dataset.expose = entry.id;
    node.dataset.exposeGridTransform = gridTransform;
    node.id = node.id || `expose-card-${entry.id}`;
    node.setAttribute("role", "option");
    node.setAttribute("aria-selected", index === activeIndex ? "true" : "false");
    // Keep frontmost z among overlaps, but don't let z-order rewrite spatial slots.
    node.style.zIndex = String(200000 + index);
    node.style.boxShadow = "0 18px 50px rgba(0, 0, 0, 0.45)";
    node.style.transformOrigin = "center center";
    node.style.opacity = "1";
    node.style.visibility = "visible";
    if (!exposeReducedMotion()) {
      node.style.transition = "transform 220ms cubic-bezier(0.2, 0, 0, 1)";
    }
    node.style.transform = gridTransform;
    mountExposeCaption(entry, wasMinimized);
  });
  syncActiveCard();
  return ordered.map((entry) => entry.id);
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
  return active || missionExiting;
}

function teardownMissionChrome() {
  window.clearTimeout(missionExitTimer);
  missionExitTimer = 0;
  window.clearTimeout(ensureGridTimer);
  ensureGridTimer = 0;
  const finish = pendingMissionFinish;
  pendingMissionFinish = null;
  const wasExiting = missionExiting;
  missionExiting = false;
  missionHitCache = null;
  lastSpacesBarRingKey = "";
  endMissionDrag();
  active = false;
  document.body.classList.remove("expose-active", "mission-exiting");
  document.body.removeAttribute("aria-activedescendant");
  document.body.removeAttribute("aria-label");
  document.body.style.removeProperty("--mission-shelf-h");
  document.body.style.removeProperty("--mission-space-aspect");
  document.body.style.removeProperty("--mission-thumb-w");
  showExposeEmpty(false);
  setChromeInert(false);
  clearFloorCards();
  if (spacesBar) {
    reclaimLiveThumbWindows();
    spacesBar.replaceChildren();
  }
  if (spacesShelf) {
    spacesShelf.setAttribute("aria-hidden", "true");
    spacesShelf.style.animation = "";
    spacesShelf.style.transition = "";
    spacesShelf.style.transform = "";
    spacesShelf.style.opacity = "";
  }
  for (const entry of shellState.windows.values()) {
    entry?.node?.classList.remove("mission-exit-hero");
  }
  reparentOrphanWindows();
  // Abort mid exit-zoom must still land on the chosen Space (no half-commit).
  if (wasExiting && finish) {
    finishStageAfterMission(finish.stageId, finish.heroWindowId, finish.focusHero);
  } else {
    syncStagePresentation();
    syncSpacePager();
  }
}

export function closeExpose() {
  if (!active && !missionExiting) {
    return;
  }
  teardownMissionChrome();
}

function finishStageAfterMission(stageId, heroWindowId, focusHero) {
  if (isAgentSpace(stageId)) {
    setActiveStage(stageId, { animate: false, focus: false, announce: true });
    return;
  }
  const entry = heroWindowId ? shellState.windows.get(heroWindowId) : null;
  if (entry?.fullscreenStage) {
    setActiveStage(heroWindowId, { animate: false, focus: Boolean(focusHero), announce: true });
    return;
  }
  const desk = stageId || entry?.desktopSpaceId || desktopStageId();
  setActiveStage(desk, { animate: false, focus: false, announce: true });
  if (focusHero && heroWindowId) {
    focusWindow(heroWindowId);
  }
}

/**
 * Mac Mission Control exit: the chosen floor card expands out to its real
 * window (inverse of the enter zoom). No horizontal Space slide.
 */
function playMissionExitZoom({ heroWindowId = null, targetStage = null, focusHero = true } = {}) {
  if (!active || missionExiting) {
    if (!active && !missionExiting) {
      finishStageAfterMission(targetStage, heroWindowId, focusHero);
    }
    return;
  }

  const heroEntry = heroWindowId ? shellState.windows.get(heroWindowId) : null;
  const hero = heroEntry?.node || null;
  const stage =
    targetStage ||
    (heroEntry?.fullscreenStage
      ? heroWindowId
      : heroEntry?.desktopSpaceId || selectedSpaceId || desktopStageId());

  if (heroWindowId && originals.has(heroWindowId)) {
    originals.get(heroWindowId).wasMinimized = false;
  }
  if (hero) {
    delete hero.dataset.exposeMinimized;
  }

  const canZoom =
    !exposeReducedMotion() &&
    hero &&
    hero.classList.contains("expose-card") &&
    (hero.style.transform || hero.dataset.exposeGridTransform);

  if (!canZoom) {
    pendingMissionFinish = { stageId: stage, heroWindowId, focusHero };
    missionExiting = true;
    teardownMissionChrome();
    return;
  }

  missionExiting = true;
  pendingMissionFinish = { stageId: stage, heroWindowId, focusHero };
  endMissionDrag();
  // Keep expose-active through the zoom so card CSS stays stable; block input.
  document.body.classList.add("mission-exiting");
  active = false;

  const fromTransform = hero.style.transform || hero.dataset.exposeGridTransform || "";
  const toTransform = originals.get(heroWindowId)?.transform || "";

  for (const [id] of originals) {
    if (id === heroWindowId) {
      continue;
    }
    const node = shellState.windows.get(id)?.node;
    if (!node) {
      continue;
    }
    if (node.classList.contains("expose-card") || node.classList.contains("mission-thumb-card")) {
      node.style.transition = "opacity 150ms ease, transform 220ms cubic-bezier(0.2, 0, 0, 1)";
      node.style.opacity = "0";
      if (node.classList.contains("expose-card") && node.style.transform) {
        node.style.transform = `${node.style.transform} scale(0.96)`;
      }
    }
  }
  if (spacesShelf) {
    spacesShelf.style.transition =
      "opacity 180ms cubic-bezier(0.2, 0, 0, 1), transform 220ms cubic-bezier(0.2, 0, 0, 1)";
    spacesShelf.style.opacity = "0";
    spacesShelf.style.transform = "translateY(-16px)";
  }

  hero.classList.add("mission-exit-hero");
  hero.style.zIndex = "200600";
  hero.style.opacity = "1";
  hero.style.visibility = "visible";
  hero.style.transition = "none";
  hero.style.transform = fromTransform;
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (!missionExiting) {
        return;
      }
      hero.style.transition = `transform ${MISSION_EXIT_MS}ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 280ms ease`;
      hero.style.transform = toTransform || "none";
      hero.style.boxShadow = "0 24px 64px rgba(0, 0, 0, 0.4)";
    });
  });

  window.clearTimeout(missionExitTimer);
  missionExitTimer = window.setTimeout(() => {
    // finish applied inside teardown via pendingMissionFinish
    teardownMissionChrome();
  }, MISSION_EXIT_MS + 20);
}

function activateExposeCard(windowId) {
  if (!windowId || !shellState.windows.has(windowId)) {
    closeExpose();
    return;
  }
  const entry = shellState.windows.get(windowId);
  const stage = entry?.fullscreenStage
    ? windowId
    : entry?.desktopSpaceId || desktopStageId();
  playMissionExitZoom({ heroWindowId: windowId, targetStage: stage, focusHero: true });
}

function confirmMissionSpace(spaceId) {
  const target = spaceId || selectedSpaceId || desktopStageId();
  if (isAgentSpace(target)) {
    playMissionExitZoom({ heroWindowId: null, targetStage: target, focusHero: false });
    return;
  }
  let heroId = null;
  if (!isDesktopSpace(target)) {
    heroId = target;
  } else if (orderedIds.length > 0) {
    heroId = orderedIds[Math.max(0, Math.min(activeIndex, orderedIds.length - 1))];
  } else {
    const floor = floorEntriesForSpace(target);
    heroId = floor[0]?.id || null;
  }
  // Expand the hero card (Apple); skip the Dock-style horizontal Space slide.
  playMissionExitZoom({ heroWindowId: heroId, targetStage: target, focusHero: Boolean(heroId) });
}

export function openExpose() {
  closeOtherShellPopovers("show-windows");
  closeExpose();
  const fromStage = getActiveStageId();
  const startRects = captureWindowStartRects();
  active = true;
  document.body.classList.add("expose-active");
  document.body.setAttribute("aria-label", "Overview");
  syncSpacePager();
  syncMissionSpaceMetrics();
  setChromeInert(true);
  // Stay on the Space you came from — zoom that Space out on the floor (Mac).
  selectedSpaceId = fromStage || desktopStageId();
  ensureSpacesBar();
  if (spacesShelf) {
    spacesShelf.setAttribute("aria-hidden", "false");
  }
  settleSpacesShelfForMeasure();
  renderSpacesBar();
  layoutMissionFloor();
  playMissionEnterMotion(startRects);
  // Re-park thumbs after paint — first measure can race layout/fonts.
  requestAnimationFrame(() => {
    if (!active) {
      return;
    }
    layoutThumbWindows();
    window.setTimeout(() => {
      if (active) {
        layoutThumbWindows();
      }
    }, 48);
  });
  return true;
}

export function toggleExpose() {
  // Never reopen mid exit-zoom — finish teardown instead.
  if (missionExiting) {
    closeExpose();
    return true;
  }
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
    confirmMissionSpace(selectedSpaceId);
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
    registerEscapeHandler("show-windows", {
      priority: 60,
      isActive: () => active || missionExiting,
      dismiss: () => closeExpose(),
    });
    registered = true;
  }
  document.addEventListener(
    "pointerdown",
    (event) => {
      if (!active || event.button !== 0) {
        return;
      }
      if (event.target.closest(".mission-spaces-bar") || event.target.closest(".window-controls")) {
        return;
      }
      const card = event.target.closest(".window.expose-card[data-expose]");
      if (!card?.dataset.expose) {
        return;
      }
      missionDrag = {
        windowId: card.dataset.expose,
        x: event.clientX,
        y: event.clientY,
        dragging: false,
        pointerId: event.pointerId,
        suppressedClick: false,
        baseTransform: card.style.transform || "",
        baseTransition: card.style.transition || "",
      };
      try {
        card.setPointerCapture?.(event.pointerId);
      } catch (_error) {
        // Optional; document listeners still track the gesture.
      }
    },
    true,
  );
  document.addEventListener(
    "pointermove",
    (event) => {
      if (!active || !missionDrag || event.pointerId !== missionDrag.pointerId) {
        return;
      }
      const dx = event.clientX - missionDrag.x;
      const dy = event.clientY - missionDrag.y;
      if (
        !missionDrag.dragging &&
        dx * dx + dy * dy >= MISSION_DRAG_THRESHOLD_PX * MISSION_DRAG_THRESHOLD_PX
      ) {
        missionDrag.dragging = true;
        missionDrag.suppressedClick = true;
        document.body.classList.add("mission-drag-space");
        rebuildMissionHitCache();
        const node = shellState.windows.get(missionDrag.windowId)?.node;
        node?.classList.add("mission-dragging");
        if (node) {
          missionDrag.baseTransform = node.style.transform || "";
          missionDrag.baseTransition = node.style.transition || "";
          missionDrag.startRect = node.getBoundingClientRect();
        }
      }
      if (missionDrag.dragging) {
        updateMissionDragVisual(event.clientX, event.clientY);
        updateDropHighlight(event.clientX, event.clientY);
      }
    },
    true,
  );
  document.addEventListener(
    "pointerup",
    (event) => {
      if (!missionDrag || event.pointerId !== missionDrag.pointerId) {
        return;
      }
      const state = missionDrag;
      if (state.dragging) {
        const drop = hitMissionDrop(event.clientX, event.clientY);
        const windowId = state.windowId;
        const insertAt =
          state.promoteInsertAt != null && Number.isFinite(state.promoteInsertAt)
            ? state.promoteInsertAt
            : null;
        suppressExposeClick = true;
        queueMicrotask(() => {
          suppressExposeClick = false;
        });
        endMissionDrag();
        if (applyMissionDrop(windowId, drop, insertAt)) {
          event.preventDefault();
          event.stopPropagation();
          if (active) {
            renderSpacesBar();
            layoutMissionFloor();
          }
        }
        return;
      }
      endMissionDrag();
    },
    true,
  );
  document.addEventListener(
    "pointercancel",
    () => {
      endMissionDrag();
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      if (!active) {
        return;
      }
      if (event.target.closest(".mission-spaces-bar") || event.target.closest(".mission-spaces-shelf")) {
        return;
      }
      const card = event.target.closest(".window[data-expose]");
      if (card?.dataset.expose) {
        if (suppressExposeClick) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        activateExposeCard(card.dataset.expose);
        return;
      }
      if (
        !event.target.closest(".window") &&
        !event.target.closest("#expose-empty") &&
        !event.target.closest("footer.taskbar")
      ) {
        // Wallpaper click: enter the Space you're previewing (Mac), not the
        // pre-MC stage — otherwise Desktop windows stay space-hidden.
        confirmMissionSpace(selectedSpaceId);
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
      const ring = buildStageRing();
      if (
        (event.key === "ArrowLeft" || event.key === "ArrowRight") &&
        (event.altKey || orderedIds.length === 0)
      ) {
        const delta = event.key === "ArrowRight" ? 1 : -1;
        const index = Math.max(0, ring.indexOf(selectedSpaceId));
        const next = ring[(index + delta + ring.length * 8) % ring.length];
        selectMissionSpace(next);
        event.preventDefault();
        event.stopPropagation();
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
        if (orderedIds.length === 0) {
          confirmMissionSpace(selectedSpaceId);
          event.preventDefault();
          event.stopPropagation();
          return;
        }
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
