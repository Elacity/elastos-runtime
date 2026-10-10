/* Phone Home edit mode, as on a phone. Hold an app and move it (its menu
   gives way), or choose Edit Home Screen from a menu: the icons jiggle, a
   tap no longer opens, and any app on the grid or in the Dock can be
   dragged: along a page, to a screen edge to turn the page (past the last
   page opens a new one), or between the grid and the Dock, which holds
   PHONE_DOCK_SLOTS apps beside the Assistant's fixed tile. Done, Escape or a
   tap on an empty spot ends it; every drop is saved as it lands (the caller
   owns the layout). Pointer events only, so a mouse drags in edit mode too.
   docs/HOME_MOBILE.md. */

import { LONG_PRESS_DRIFT_PX, LONG_PRESS_MS } from "./shell-touch.js?v=home-20260813a";
import {
  appendPhoneHomePage,
  currentPhoneHomePage,
  phoneHomePageCount,
  phoneHomePageNode,
  showPhoneHomePage,
} from "./shell-phone-home-pager.js?v=home-20260813a";

// Held against a screen edge this long, the drag turns the page.
const EDGE_TURN_MS = 450;
const EDGE_PX = 28;
// After a page turn, the next turn waits for the slide to finish.
const PAGE_SLIDE_MS = 380;
// In edit mode a press becomes a drag after this much travel.
const EDIT_DRAG_START_PX = 6;
// The click a drop's release produces arrives well inside this; a deliberate
// tap on Done right after a drop does not.
const DROP_CLICK_SWALLOW_MS = 300;
// A drag reaches for the Dock from just above it.
const DOCK_REACH_PX = 16;

const GRID_TILE = ".phone-home-app[data-target]";
const DOCK_TILE = '.taskbar-entry[data-phone-dock="slot"] > .taskbar-item[data-target]';

let root = null;
let taskbar = null;
let deps = null;
let editing = false;
let press = null;
let drag = null;
let swallowClickUntil = 0;

function now() {
  return window.performance ? window.performance.now() : Date.now();
}

function reducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

export function isPhoneHomeEditing() {
  return editing;
}

export function isPhoneHomeDragging() {
  return drag !== null;
}

export function enterPhoneHomeEdit() {
  if (editing || !deps?.active()) {
    return;
  }
  editing = true;
  document.body.classList.add("phone-home-editing");
  deps.announce("Editing Home Screen. Drag apps to move them, then choose Done.");
}

export function exitPhoneHomeEdit() {
  if (!editing) {
    return;
  }
  cancelDrag();
  editing = false;
  document.body.classList.remove("phone-home-editing");
  deps.announce("Home Screen saved");
}

function tileAt(target) {
  return target?.closest?.(GRID_TILE) || target?.closest?.(DOCK_TILE) || null;
}

function clearPress() {
  if (press) {
    window.clearTimeout(press.timer);
  }
  press = null;
}

function onPointerDown(event) {
  if (drag || event.button > 0 || !deps.active()) {
    return;
  }
  const tile = tileAt(event.target);
  if (!tile || (!editing && event.pointerType === "mouse")) {
    return;
  }
  clearPress();
  press = {
    pointerId: event.pointerId,
    targetId: tile.dataset.target,
    source: tile.matches(GRID_TILE) ? "grid" : "dock",
    tile,
    x: event.clientX,
    y: event.clientY,
    held: editing,
    timer: 0,
  };
  if (!editing) {
    // The same hold that opens the app's menu (shell-touch.js) arms a drag.
    press.timer = window.setTimeout(() => {
      if (press) {
        press.held = true;
      }
    }, LONG_PRESS_MS);
  }
}

function onPointerMove(event) {
  if (drag) {
    if (event.pointerId === drag.pointerId) {
      moveDrag(event.clientX, event.clientY);
    }
    return;
  }
  if (!press || event.pointerId !== press.pointerId) {
    return;
  }
  const distance = Math.hypot(event.clientX - press.x, event.clientY - press.y);
  if (!press.held) {
    if (distance >= LONG_PRESS_DRIFT_PX) {
      clearPress();
    }
    return;
  }
  if (distance >= (editing ? EDIT_DRAG_START_PX : LONG_PRESS_DRIFT_PX)) {
    startDrag(event);
  }
}

function onPointerUp(event) {
  if (drag && event.pointerId === drag.pointerId) {
    finishDrag();
    return;
  }
  if (press && event.pointerId === press.pointerId) {
    clearPress();
  }
}

function onPointerCancel(event) {
  if (drag && event.pointerId === drag.pointerId) {
    cancelDrag();
  }
  if (press && event.pointerId === press.pointerId) {
    clearPress();
  }
}

function startDrag(event) {
  const from = press;
  clearPress();
  deps.hideMenu();
  enterPhoneHomeEdit();
  const icon = from.tile.querySelector(".phone-home-icon, .taskbar-item-icon") || from.tile;
  const box = icon.getBoundingClientRect();
  const ghost = document.createElement("div");
  ghost.className = "phone-home-ghost";
  ghost.setAttribute("aria-hidden", "true");
  ghost.style.width = `${box.width}px`;
  ghost.style.height = `${box.height}px`;
  ghost.appendChild(icon.cloneNode(true));
  document.body.appendChild(ghost);
  const lifted = from.source === "grid" ? from.tile : from.tile.closest(".taskbar-entry");
  const spacer = document.createElement("div");
  spacer.className = "phone-home-spacer";
  spacer.setAttribute("aria-hidden", "true");
  if (from.source === "grid") {
    from.tile.after(spacer);
  }
  lifted.classList.add("is-lifted");
  from.tile.dataset.suppressClick = "true";
  drag = {
    ...from,
    ghost,
    spacer,
    lifted,
    offsetX: from.x - box.left,
    offsetY: from.y - box.top,
    x: event.clientX,
    y: event.clientY,
    overDock: false,
    dockIndex: 0,
    edgeSide: 0,
    edgeTimer: 0,
  };
  moveDrag(event.clientX, event.clientY);
}

function moveDrag(x, y) {
  drag.x = x;
  drag.y = y;
  drag.ghost.style.transform = `translate(${x - drag.offsetX}px, ${y - drag.offsetY}px)`;
  const dock = taskbar.getBoundingClientRect();
  if (y >= dock.top - DOCK_REACH_PX) {
    setEdge(0);
    const accepts = drag.source === "dock" || deps.dockHasRoom();
    drag.overDock = accepts;
    taskbar.classList.toggle("phone-home-drop", accepts);
    if (accepts) {
      drag.spacer.remove();
      drag.dockIndex = dockIndexAt(x);
    }
    return;
  }
  drag.overDock = false;
  taskbar.classList.remove("phone-home-drop");
  const side = x < EDGE_PX ? -1 : x > window.innerWidth - EDGE_PX ? 1 : 0;
  setEdge(side);
  placeSpacer(x, y);
}

function dockIndexAt(x) {
  const tiles = [...taskbar.querySelectorAll(DOCK_TILE)].filter(
    (tile) => tile.dataset.target !== drag.targetId,
  );
  return tiles.filter((tile) => {
    const box = tile.getBoundingClientRect();
    return box.left + box.width / 2 < x;
  }).length;
}

// Where the spacer lands on the page in view: before the first tile in a
// later row, or in this row to the right of the finger. Layout offsets, not
// painted boxes, so tiles mid-slide do not move the target.
function placeSpacer(x, y) {
  const page = phoneHomePageNode(currentPhoneHomePage());
  if (!page) {
    return;
  }
  const origin = page.getBoundingClientRect();
  const tiles = [...page.children].filter(
    (node) => node !== drag.spacer && node !== drag.lifted && node.matches(GRID_TILE),
  );
  let before = null;
  for (const tile of tiles) {
    const top = origin.top + tile.offsetTop;
    const left = origin.left + tile.offsetLeft;
    if (y < top || (y <= top + tile.offsetHeight && x < left + tile.offsetWidth / 2)) {
      before = tile;
      break;
    }
  }
  if (drag.spacer.parentElement === page && drag.spacer.nextElementSibling === before) {
    return;
  }
  if (before && before.previousElementSibling === drag.spacer) {
    return;
  }
  glide(page, () => {
    if (before) {
      page.insertBefore(drag.spacer, before);
    } else {
      page.appendChild(drag.spacer);
    }
  });
}

// FLIP: the other icons slide to their new slots instead of jumping.
function glide(page, change) {
  if (reducedMotion()) {
    change();
    return;
  }
  const tiles = [...page.querySelectorAll(GRID_TILE)];
  const before = new Map(tiles.map((tile) => [tile, [tile.offsetLeft, tile.offsetTop]]));
  change();
  for (const tile of tiles) {
    const [left, top] = before.get(tile);
    const dx = left - tile.offsetLeft;
    const dy = top - tile.offsetTop;
    if (dx === 0 && dy === 0) {
      continue;
    }
    tile.style.transition = "none";
    tile.style.transform = `translate(${dx}px, ${dy}px)`;
    void tile.offsetWidth;
    tile.style.transition = "";
    tile.style.transform = "";
  }
}

function setEdge(side) {
  if (side === drag.edgeSide) {
    return;
  }
  window.clearTimeout(drag.edgeTimer);
  drag.edgeSide = side;
  drag.edgeTimer = side === 0 ? 0 : window.setTimeout(() => turnPage(side), EDGE_TURN_MS);
}

function turnPage(side) {
  if (!drag) {
    return;
  }
  const next = currentPhoneHomePage() + side;
  if (next < 0) {
    return;
  }
  if (next >= phoneHomePageCount()) {
    // A new page only when the dragged app would not leave its old one empty.
    const last = phoneHomePageNode(phoneHomePageCount() - 1);
    const others = last
      ? [...last.querySelectorAll(GRID_TILE)].filter((tile) => tile !== drag.lifted).length
      : 0;
    if (others === 0) {
      return;
    }
    appendPhoneHomePage();
  }
  showPhoneHomePage(next);
  drag.edgeTimer = window.setTimeout(() => {
    if (!drag) {
      return;
    }
    placeSpacer(drag.x, drag.y);
    drag.edgeTimer = window.setTimeout(() => turnPage(side), EDGE_TURN_MS);
  }, PAGE_SLIDE_MS);
}

function endDrag() {
  const ended = drag;
  drag = null;
  window.clearTimeout(ended.edgeTimer);
  ended.ghost.remove();
  ended.lifted.classList.remove("is-lifted");
  taskbar.classList.remove("phone-home-drop");
  swallowClickUntil = now() + DROP_CLICK_SWALLOW_MS;
  return ended;
}

function finishDrag() {
  const ended = endDrag();
  let to = null;
  if (ended.overDock) {
    to = { kind: "dock", index: ended.dockIndex };
  } else if (ended.spacer.isConnected) {
    const page = ended.spacer.parentElement;
    const index = [...page.children]
      .filter((node) => node !== ended.lifted && (node === ended.spacer || node.matches(GRID_TILE)))
      .indexOf(ended.spacer);
    to = { kind: "grid", page: Number(page.dataset.page), index };
  }
  ended.spacer.remove();
  if (to) {
    deps.drop({ targetId: ended.targetId, source: ended.source, to });
  } else {
    deps.restore();
  }
}

function cancelDrag() {
  if (!drag) {
    return;
  }
  const ended = endDrag();
  ended.spacer.remove();
  deps.restore();
}

function onClick(event) {
  if (swallowClickUntil !== 0) {
    const swallow = now() < swallowClickUntil;
    swallowClickUntil = 0;
    if (swallow) {
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }
  }
  if (!editing || event.target.closest?.("#phone-home-done, #phone-home-dots")) {
    return;
  }
  // A tap in edit mode never opens; one on an empty spot ends the mode.
  event.preventDefault();
  event.stopImmediatePropagation();
  if (!tileAt(event.target) && !event.target.closest?.(".taskbar")) {
    exitPhoneHomeEdit();
  }
}

/**
 * @param {{
 *   active: () => boolean,
 *   dockHasRoom: () => boolean,
 *   drop: (move: { targetId: string, source: "grid" | "dock", to: object }) => void,
 *   restore: () => void,
 *   hideMenu: () => void,
 *   announce: (message: string) => void,
 * }} dependencies
 */
export function bindPhoneHomeEdit(dependencies) {
  deps = dependencies;
  root = document.querySelector("#phone-home");
  taskbar = document.querySelector(".taskbar");
  if (!root || !taskbar) {
    return;
  }
  for (const host of [root, taskbar]) {
    host.addEventListener("pointerdown", onPointerDown);
    // Once a hold has armed a drag the thumb moves the icon, not the pages.
    host.addEventListener("touchmove", (event) => {
      if (drag || press?.held) {
        event.preventDefault();
      }
    }, { passive: false });
  }
  document.addEventListener("pointermove", onPointerMove);
  document.addEventListener("pointerup", onPointerUp);
  document.addEventListener("pointercancel", onPointerCancel);
  document.addEventListener("click", onClick, true);
  document.addEventListener("contextmenu", (event) => {
    if (editing) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);
  document.addEventListener("keydown", (event) => {
    if (editing && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      exitPhoneHomeEdit();
    }
  }, true);
  document.querySelector("#phone-home-done")?.addEventListener("click", () => exitPhoneHomeEdit());
}
