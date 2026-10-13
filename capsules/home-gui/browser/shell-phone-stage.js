/* Phone stage: the system back gesture and the title-bar swipe.

   Back: the shell keeps at most one history entry of its own (model in
   shell-phone-stage-history.js). It is pushed when the first layer (a window
   on the stage, a sheet, Mission Control, the Assistant room, or the
   long-press menu) opens on a phone and consumed again
   when the last layer closes, so the bare desktop never traps back — the
   host page's own history is left alone. popstate closes the top layer:
   Mission Control, then any open sheet (through the shell's Escape
   contract), then the window that is actually on this Space goes home
   (minimise; the capsule keeps running). A window parked on another Space
   is not that window.

   Swipe: a downward drag on the title bar opens Mission Control as the app
   switcher. It starts on the title bar only — capsule frames own their own
   touches — and cancels on horizontal drift. Presentation only. */

import { shellState } from "./shell-core.js?v=home-20260813a";
import {
  STAGE_HISTORY_STATE,
  createStageHistory,
  isStageHistoryState,
} from "./shell-phone-stage-history.js?v=home-20260813a";
import { isPhone } from "./shell-form-factor.js?v=home-20260813a";
import { minimizeWindow } from "./shell-windows.js?v=home-20260813a";
import { closeExpose, isExposeOpen, openExpose } from "./shell-expose.js?v=home-20260813a";
import { handleShellEscape } from "./shell-popovers.js?v=home-20260813a";
import { hideDesktopContextMenu, hideLauncher } from "./shell-surface.js?v=home-20260813a";
import { hideSpotlight } from "./shell-spotlight.js?v=home-20260813a";
import { hideControlCentre } from "./shell-control-centre.js?v=home-20260813a";
import { hideNotificationCenter } from "./shell-notifications.js?v=home-20260813a";
import { hideInboxRail } from "./shell-inbox-rail.js?v=home-20260813a";
import { hideWalletRail } from "./shell-wallet-rail.js?v=home-20260813a";
import { dismissAssistantForBack, hideAssistantFace } from "./shell-assistant-face.js?v=home-20260813a";

// A title-bar drag this far down opens the switcher…
export const TITLE_SWIPE_DOWN_PX = 48;
// …unless it drifted this far sideways first (a scroll or a slip).
export const TITLE_SWIPE_DRIFT_PX = 32;

// Sheets that count as a layer over the stage, top-most first, each with the
// closer its module already exposes.
const SHEETS = Object.freeze([
  { selector: "#desktop-context-menu", close: () => hideDesktopContextMenu() },
  { selector: "#assistant-space", close: () => dismissAssistantForBack() },
  { selector: "#control-centre", close: () => hideControlCentre() },
  { selector: "#notification-center", close: () => hideNotificationCenter() },
  { selector: "#spotlight", close: () => hideSpotlight() },
  { selector: "#inbox-rail", close: () => hideInboxRail() },
  { selector: "#wallet-rail", close: () => hideWalletRail() },
  { selector: "#launcher", close: () => hideLauncher() },
  { selector: "#assistant-face", close: () => hideAssistantFace() },
]);
export const SHEET_SELECTORS = Object.freeze(SHEETS.map((sheet) => sheet.selector));

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

export function openSheetCount(doc = document) {
  return SHEET_SELECTORS.filter((selector) => {
    const node = doc.querySelector(selector);
    return node && !node.hidden;
  }).length;
}

export function stageLayerCount(doc = document) {
  return visibleWindowCount() + openSheetCount(doc) + (isExposeOpen() ? 1 : 0);
}

// Closes the top layer and reports how many remain. The long-press menu and
// the Assistant room sit above the Escape registry: the Agent room is not a
// desktop Space, so the fullscreen-stage Escape handler would otherwise take
// the back and leave the room up.
export function closeTopStageLayer(doc = document) {
  const overlay = SHEETS.find((sheet) => {
    if (sheet.selector !== "#desktop-context-menu" && sheet.selector !== "#assistant-space") {
      return false;
    }
    const node = doc.querySelector(sheet.selector);
    return node && !node.hidden;
  });
  if (overlay) {
    overlay.close();
    return stageLayerCount(doc);
  }
  // The shell's ordered Escape registry first (modal, menus, Mission Control…).
  if (handleShellEscape()) {
    return stageLayerCount(doc);
  }
  if (isExposeOpen()) {
    closeExpose();
    return stageLayerCount(doc);
  }
  const openSheet = SHEETS.find((sheet) => {
    const node = doc.querySelector(sheet.selector);
    return node && !node.hidden;
  });
  if (openSheet) {
    openSheet.close();
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
      gesture = { startX: event.clientX, startY: event.clientY, pointerId: event.pointerId };
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

export function bindPhoneStage(doc = document, view = window) {
  bindTitleSwipe(doc);
  if (!stageHistorySupported(view)) {
    doc.body.dataset.stageHistory = "buttons";
    return null;
  }
  doc.body.dataset.stageHistory = "history";
  const stageHistory = createStageHistory({
    onOurEntry: () => isStageHistoryState(view.history.state),
    layerCount: () => (isPhone(view) ? stageLayerCount(doc) : 0),
    pushState: () => view.history.pushState(STAGE_HISTORY_STATE, ""),
    back: () => view.history.back(),
    onBack: () => closeTopStageLayer(doc),
  });
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
  new view.MutationObserver(schedule).observe(doc.body, {
    attributes: true,
    attributeFilter: ["class", "hidden"],
    childList: true,
    subtree: true,
  });
  view.addEventListener("popstate", () => {
    stageHistory.handlePopState();
  });
  view.addEventListener("resize", schedule);
  return stageHistory;
}
