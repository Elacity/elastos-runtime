/* Phone stage: the system back gesture and the title-bar swipe.

   Back: the shell keeps at most one history entry of its own (model in
   shell-phone-stage-history.js). It is pushed when the first layer (a window
   on the stage or any registered Back layer: sheets, rails, menus, About,
   Mission Control, the Assistant room) opens on a phone; the shell never
   calls back() while anything is open, and a Back that pops a leftover entry
   with nothing open is finished as "leave Home". popstate closes exactly one layer: the
   open layer that paints on top (one stacking order over every registered
   layer), through the close it registered, and that layer stops counting at
   once while its hide animates. With no layer open, the window that is
   actually on this Space goes home (minimise; the capsule keeps running). A
   window parked on another Space is not that window. Layers register next to
   their own close (registerStageLayer in shell-popovers.js); the phone smoke
   fails on a Home host surface that has none.

   Form factor: decided at load. A resize or rotation across the phone
   boundary runs one clean stage reset (resetPhoneStage) before any new input
   is handled: gestures in progress are cancelled, fullscreen Spaces are
   left, every open registered layer closes through its own close, the
   shell's history entry is dropped and re-armed for the new mode, and app
   frames acknowledge dropping their own history before a phone stage arms
   (APP_HISTORY_MESSAGE). Each step is reported by its owner, never inferred
   from a timeout: a timeout starts a recovery. Back therefore only ever runs
   in a stable form factor; app windows stay open.

   Swipe: a downward drag on the title bar opens Mission Control as the app
   switcher. It starts on the title bar only — capsule frames own their own
   touches — and cancels on horizontal drift. Presentation only. */

import { HOME_GUI_HOST_SELECTORS, shellState } from "./shell-core.js?v=home-20260813a";
import {
  STAGE_HISTORY_STATE,
  createStageHistory,
  isStageHistoryState,
} from "./shell-phone-stage-history.js?v=home-20260813a";
import { isPhone } from "./shell-form-factor.js?v=home-20260813a";
import { minimizeWindow } from "./shell-windows.js?v=home-20260813a";
import { exitFullscreenStage } from "./shell-stages.js?v=home-20260813a";
import { isExposeOpen, openExpose } from "./shell-expose.js?v=home-20260813a";
import {
  LAYER_STATE,
  cancelStageGestures,
  registerStageGesture,
  registeredStageLayers,
} from "./shell-popovers.js?v=home-20260813a";
import { isDismissing } from "./shell-motion.js?v=home-20260813a";

// A title-bar drag this far down opens the switcher…
export const TITLE_SWIPE_DOWN_PX = 48;
// …unless it drifted this far sideways first (a scroll or a slip).
export const TITLE_SWIPE_DRIFT_PX = 32;

// Always on screen, so never a layer. The notification toast is a passing
// notice that dismisses itself, not something Back has to close. Every other
// host surface must be a registered layer.
const STAGE_CHROME = Object.freeze([
  ".desktop-backdrop",
  ".toolbar",
  ".desktop-workspace",
  ".taskbar",
  "#home-notification-toast",
]);
export const LAYER_HOST_SELECTORS = Object.freeze(
  HOME_GUI_HOST_SELECTORS.filter((selector) => !STAGE_CHROME.includes(selector)),
);
// How long a reset waits for one history report (the shell's own traversal,
// an app frame's acknowledgement) before it recovers, and how often it asks
// an app frame to drop its entries.
export const RESET_HISTORY_TIMEOUT_MS = 1500;
export const RESET_HISTORY_ATTEMPTS = 2;
// An app frame that holds browser-history entries of its own says so
// ({ type, held: true }) and says { held: false } once it dropped them. The
// shell asks it to drop them ({ type, drop: true }) when it resets to a phone
// stage, and arms its history only after every holder answered (Library,
// capsules/library/browser/src/navigation.js).
export const APP_HISTORY_MESSAGE = "elastos:app-history";
// A close that never lands stops holding its layer after this long.
export const LAYER_CLOSE_TIMEOUT_MS = 2000;

// On the stage: not minimised, and still in the Space the Dock is showing.
export function windowOnStage(entry) {
  const node = entry?.node;
  return Boolean(node && !node.classList.contains("hidden") && node.dataset.spaceVisible !== "false");
}

export function visibleWindowCount(windows = shellState.windows) {
  let count = 0;
  for (const entry of windows.values()) {
    if (windowOnStage(entry)) {
      count += 1;
    }
  }
  return count;
}

function shown(node) {
  return Boolean(node && !node.hidden && node.getClientRects().length > 0);
}

function layerNode(layer, doc) {
  return doc.querySelector(layer.selector);
}

// The layer's own report (registerStageLayer in shell-popovers.js). Without
// one, the registered element is the one that animates: closed while not
// shown, closing while its dismissWithMotion leave plays, open otherwise.
export function stageLayerState(layer, doc = document) {
  if (layer.state) {
    return layer.state();
  }
  const node = layerNode(layer, doc);
  if (!shown(node)) {
    return LAYER_STATE.closed;
  }
  return isDismissing(node) ? LAYER_STATE.closing : LAYER_STATE.open;
}

// Layers whose close is in flight: one Back already took them. Each maps to
// its settle; a layer leaves the set the moment it reports closed, so the
// same surface opened again afterwards is a new layer Back can take.
const closing = new Map();
let onLayerSettled = () => {};

// The close has landed: the layer reports closed, or it reported closing and
// now reports open again, so it was reopened mid-dismiss and is a new layer.
// A layer that never reported closing is still waiting for its close.
function closeLanded(layer, settle, doc) {
  const state = stageLayerState(layer, doc);
  if (state === LAYER_STATE.closing) {
    settle.sawClosing = true;
  }
  return state === LAYER_STATE.closed || (state === LAYER_STATE.open && settle.sawClosing === true);
}

// Called on every shell DOM change: settles each closing layer that is gone.
export function releaseClosedLayers(doc = document) {
  for (const [id, settle] of [...closing]) {
    const layer = registeredStageLayers().find((entry) => entry.id === id);
    if (!layer || closeLanded(layer, settle, doc)) {
      settle();
    }
  }
}

// z-indexed ancestors of a node, outermost first.
function stackingPath(node, view) {
  const path = [];
  for (let el = node; el && el.nodeType === 1; el = el.parentElement) {
    const z = Number.parseInt(view.getComputedStyle(el).zIndex, 10);
    if (Number.isFinite(z)) {
      path.unshift({ el, z });
    }
  }
  return path;
}

function documentOrder(a, b) {
  return a.compareDocumentPosition(b) & 4 ? -1 : 1;
}

// Negative when `a` paints below `b`. Walks both stacking paths to the first
// context they do not share: a higher z wins there, then the later element in
// the document; inside one context the nested surface is on top.
export function compareStacking(a, b, view) {
  if (!a || !b) {
    return a ? 1 : b ? -1 : 0;
  }
  const pa = stackingPath(a, view);
  const pb = stackingPath(b, view);
  for (let index = 0; index < Math.min(pa.length, pb.length); index += 1) {
    if (pa[index].el === pb[index].el) {
      continue;
    }
    return pa[index].z !== pb[index].z ? pa[index].z - pb[index].z : documentOrder(pa[index].el, pb[index].el);
  }
  if (pa.length !== pb.length) {
    return pa.length - pb.length;
  }
  return a === b ? 0 : documentOrder(a, b);
}

// Open layers that Back has not taken yet, top-most last.
export function openStageLayers(doc = document) {
  const view = doc.defaultView;
  return registeredStageLayers()
    .filter((layer) => !closing.has(layer.id) && stageLayerState(layer, doc) === LAYER_STATE.open)
    .map((layer) => ({ layer, node: layerNode(layer, doc) }))
    .sort((a, b) => compareStacking(a.node, b.node, view))
    .map(({ layer }) => layer);
}

export function openSheetCount(doc = document) {
  return openStageLayers(doc).length;
}

export function stageLayerCount(doc = document) {
  return visibleWindowCount() + openSheetCount(doc);
}

// Runs the layer's own close and resolves once it is really gone (its hide
// animation included), or after LAYER_CLOSE_TIMEOUT_MS if it never goes.
export function closeStageLayer(layer, doc = document) {
  const view = doc.defaultView;
  const started = Date.now();
  return new Promise((resolve) => {
    let timer = 0;
    const settle = () => {
      if (closing.get(layer.id) !== settle) {
        return;
      }
      view.clearTimeout(timer);
      closing.delete(layer.id);
      onLayerSettled();
      resolve();
    };
    // DOM changes settle it at once (releaseClosedLayers); this poll covers a
    // close that changes no observed attribute.
    const check = () => {
      if (!closeLanded(layer, settle, doc) && Date.now() - started < LAYER_CLOSE_TIMEOUT_MS) {
        timer = view.setTimeout(check, 32);
        return;
      }
      settle();
    };
    closing.get(layer.id)?.();
    closing.set(layer.id, settle);
    try {
      layer.close();
    } catch (_error) {
      // A close that throws leaves the layer open; the timeout releases it.
    }
    check();
  });
}

// Closes exactly one layer, the top-most, and reports how many remain.
export function closeTopStageLayer(doc = document) {
  const top = openStageLayers(doc).at(-1);
  if (top) {
    void closeStageLayer(top, doc);
    return stageLayerCount(doc);
  }
  const active = shellState.windows.get(shellState.activeWindowId);
  const visible = [...shellState.windows.values()].find(windowOnStage);
  const chosen = windowOnStage(active) ? active : visible;
  if (chosen?.id) {
    minimizeWindow(chosen.id);
  }
  return stageLayerCount(doc);
}

// The swipe ends with a click on the title bar, which is now a Mission
// Control card; letting it through would activate the card and undo the open.
// Window capture runs before Mission Control's document-capture handlers.
function suppressGestureClick(doc) {
  const view = doc.defaultView;
  const swallow = (event) => {
    event.preventDefault();
    event.stopPropagation();
  };
  view.addEventListener("click", swallow, { capture: true, once: true });
  view.setTimeout(() => view.removeEventListener("click", swallow, { capture: true }), 500);
}

function bindTitleSwipe(doc) {
  let gesture = null;
  // A stage reset forgets a press in progress: its release is swallowed, and
  // a later move must not open Mission Control without a new press.
  registerStageGesture("title-swipe", () => {
    if (!gesture) {
      return;
    }
    try {
      gesture.head.releasePointerCapture(gesture.pointerId);
    } catch (_error) {
      // Capture may already be gone.
    }
    gesture = null;
  });
  doc.addEventListener(
    "pointerdown",
    (event) => {
      if (!isPhone() || isExposeOpen()) {
        return;
      }
      const head = event.target.closest?.(".window-head");
      if (!head || event.target.closest("button")) {
        return;
      }
      gesture = { head, startX: event.clientX, startY: event.clientY, pointerId: event.pointerId };
      // The window body is an iframe; without capture the pointer's moves
      // would land in that document once the swipe leaves the title bar.
      try {
        head.setPointerCapture(event.pointerId);
      } catch (_error) {
        // Capture is a nicety; the gesture still works when the moves stay on the head.
      }
    },
    { passive: true },
  );
  doc.addEventListener(
    "pointermove",
    (event) => {
      if (!gesture || event.pointerId !== gesture.pointerId) {
        return;
      }
      const dx = Math.abs(event.clientX - gesture.startX);
      const dy = event.clientY - gesture.startY;
      if (dx >= TITLE_SWIPE_DRIFT_PX) {
        gesture = null;
        return;
      }
      if (dy >= TITLE_SWIPE_DOWN_PX) {
        gesture = null;
        suppressGestureClick(doc);
        openExpose();
      }
    },
    { passive: true },
  );
  const end = () => {
    gesture = null;
  };
  doc.addEventListener("pointerup", end, { passive: true });
  doc.addEventListener("pointercancel", end, { passive: true });
}

// WebKit (Safari, every iOS browser) records nested frame loads as joint
// session steps and, seen from the shell frame, one history.back() after a
// pushState took the shell frame itself to about:blank (phone smoke,
// 2026-09-23). Buttons and the title swipe cover every function, so on
// WebKit the stage history stays off. Chromium and Gecko follow the spec.
export function stageHistorySupported(view = window) {
  const agent = String(view.navigator?.userAgent || "");
  const webkit = /AppleWebKit\//.test(agent) && !/(Chrome|Chromium|Edg)\//.test(agent);
  return !webkit;
}

// The bound stage: its history model (null on WebKit), whether a reset is
// running, and what to call when an app frame reports its history.
const stage = { history: null, resetting: false, onAppHistory: () => {} };
// App frames that hold browser-history entries of their own. The stage never
// arms its own entry while one does (a late traversal in that frame would
// race it); it arms the moment the last one reports it dropped them.
const historyHolders = new Set();

function trackAppHistory(event, doc) {
  if (event.data?.type !== APP_HISTORY_MESSAGE || typeof event.data.held !== "boolean") {
    return;
  }
  const fromFrame = [...doc.querySelectorAll("iframe")].some((frame) => frame.contentWindow === event.source);
  if (!fromFrame) {
    return;
  }
  if (event.data.held) {
    historyHolders.add(event.source);
  } else {
    historyHolders.delete(event.source);
  }
  stage.onAppHistory();
}

function liveHolders(doc) {
  if (historyHolders.size === 0) {
    return 0;
  }
  const frames = new Set([...doc.querySelectorAll("iframe")].map((frame) => frame.contentWindow));
  for (const holder of historyHolders) {
    if (!frames.has(holder)) {
      historyHolders.delete(holder);
    }
  }
  return historyHolders.size;
}

// Resolves true once `predicate` holds, false when `timeoutMs` ran out first.
function settled(predicate, view, timeoutMs = RESET_HISTORY_TIMEOUT_MS) {
  const started = Date.now();
  return new Promise((resolve) => {
    const check = () => {
      if (predicate()) {
        resolve(true);
        return;
      }
      if (Date.now() - started >= timeoutMs) {
        resolve(false);
        return;
      }
      view.setTimeout(check, 32);
    };
    check();
  });
}

// Leaving the phone: the shell's own entry goes. Done only when the browser
// reports the traversal landed (history.state is no longer ours). If it never
// does, recovery makes the entry stop being ours in place, so a late
// traversal finds nothing to close and no leftover to leave Home over.
async function dropOwnEntry(view) {
  view.history.back();
  if (await settled(() => !isStageHistoryState(view.history.state), view)) {
    return true;
  }
  if (isStageHistoryState(view.history.state)) {
    view.history.replaceState(null, "");
  }
  return false;
}

// Entering the phone: each frame that holds entries is asked to drop them
// and acknowledges with { held: false } once its traversal landed (Library,
// capsules/library/browser/src/navigation.js). A frame that has not answered
// in time is asked again. Silence is never success: the stage stays unarmed
// (layerCount) until every holder answered, so Back steps the frame as on a
// desktop and nothing races; the stage arms when the answer arrives.
async function unwindAppHistory(doc, view) {
  for (let attempt = 0; attempt < RESET_HISTORY_ATTEMPTS; attempt += 1) {
    if (liveHolders(doc) === 0) {
      return true;
    }
    for (const holder of historyHolders) {
      holder.postMessage({ type: APP_HISTORY_MESSAGE, drop: true }, "*");
    }
    if (await settled(() => liveHolders(doc) === 0, view)) {
      return true;
    }
  }
  return false;
}

const RESET_BLOCKED_INPUT = Object.freeze([
  "pointerdown", "pointerup", "click", "dblclick", "contextmenu", "touchstart", "touchend",
  "keydown", "keyup", "wheel",
]);

// Nothing new reaches the shell or a capsule frame until the reset is done.
function blockInput(doc, view) {
  const swallow = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  for (const type of RESET_BLOCKED_INPUT) {
    view.addEventListener(type, swallow, { capture: true, passive: false });
  }
  const wasInert = doc.body.inert;
  doc.body.inert = true;
  doc.body.dataset.stageReset = "running";
  return () => {
    for (const type of RESET_BLOCKED_INPUT) {
      view.removeEventListener(type, swallow, { capture: true });
    }
    doc.body.inert = wasInert;
    delete doc.body.dataset.stageReset;
  };
}

// One clean stage reset across the phone boundary; `commit` switches the
// committed form factor (shell-form-factor.js bindFormFactor). Input stays
// blocked until the stage is consistent: every layer closed, the shell's own
// entry gone or recovered, and app frames either acknowledged their drop or
// left the stage unarmed until they do.
export async function resetPhoneStage(commit, doc = document, view = window) {
  const leavingPhone = isPhone(view);
  // Presses from the old form factor end here, before their release is
  // swallowed below.
  cancelStageGestures();
  const unblock = blockInput(doc, view);
  stage.resetting = true;
  try {
    for (const entry of [...shellState.windows.values()]) {
      if (entry?.fullscreenStage) {
        exitFullscreenStage(entry.id);
      }
    }
    // Top-most first, each through its own close; then wait out any close
    // still in flight (one a Back or a Close button started), as each layer
    // reports it.
    for (const layer of openStageLayers(doc).reverse()) {
      await closeStageLayer(layer, doc);
    }
    const layerClosing = (layer) => stageLayerState(layer, doc) === LAYER_STATE.closing;
    await settled(() => closing.size === 0 && !registeredStageLayers().some(layerClosing), view, LAYER_CLOSE_TIMEOUT_MS);
    if (stage.history && leavingPhone && isStageHistoryState(view.history.state)) {
      await dropOwnEntry(view);
    }
    commit();
    if (stage.history && !leavingPhone && isPhone(view)) {
      await unwindAppHistory(doc, view);
    }
  } finally {
    stage.resetting = false;
    stage.history?.sync();
    unblock();
  }
}

export function bindPhoneStage(doc = document, view = window) {
  bindTitleSwipe(doc);
  let onChange = () => {};
  const observer = new view.MutationObserver(() => {
    releaseClosedLayers(doc);
    onChange();
  });
  observer.observe(doc.body, {
    attributes: true,
    attributeFilter: ["class", "hidden"],
    childList: true,
    subtree: true,
  });
  view.addEventListener("message", (event) => trackAppHistory(event, doc));
  if (!stageHistorySupported(view)) {
    doc.body.dataset.stageHistory = "buttons";
    return null;
  }
  doc.body.dataset.stageHistory = "history";
  const stageHistory = createStageHistory({
    onOurEntry: () => isStageHistoryState(view.history.state),
    // Nothing arms while a reset runs or while an app frame still holds
    // entries of its own below ours.
    layerCount: () => (isPhone(view) && !stage.resetting && liveHolders(doc) === 0 ? stageLayerCount(doc) : 0),
    pushState: () => view.history.pushState(STAGE_HISTORY_STATE, ""),
    back: () => view.history.back(),
    onBack: () => closeTopStageLayer(doc),
  });
  stage.history = stageHistory;
  let scheduled = false;
  const sync = () => {
    scheduled = false;
    stageHistory.sync();
  };
  const schedule = () => {
    if (!scheduled) {
      scheduled = true;
      view.queueMicrotask ? view.queueMicrotask(sync) : Promise.resolve().then(sync);
    }
  };
  // A layer whose close timed out counts again; let the history catch up.
  onLayerSettled = schedule;
  onChange = schedule;
  stage.onAppHistory = schedule;
  view.addEventListener("popstate", () => {
    releaseClosedLayers(doc);
    // The reset's own traversal, or a Back pressed while input is blocked:
    // the reset re-arms for the new form factor when it is done.
    if (stage.resetting) {
      return;
    }
    stageHistory.handlePopState();
  });
  return stageHistory;
}
