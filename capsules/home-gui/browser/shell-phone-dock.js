/* Phone Dock: on a phone the Dock tucks under an open window so the app gets
   the whole stage, and a 24 px handle above the home indicator brings it back
   (tap or swipe up). Mirrors the fullscreen-Space tuck in shell-stages.js:
   body classes drive CSS, the JS only decides when. Presentation only — the
   Dock's contents, pins and running state are untouched. docs/HOME_MOBILE.md. */

import { launcher, shellState, taskbarTargets } from "./shell-core.js?v=home-20260813a";
import { isPhone } from "./shell-form-factor.js?v=home-20260813a";

export const PHONE_DOCK_TUCKED_CLASS = "phone-dock-tucked";
export const PHONE_DOCK_PEEK_CLASS = "phone-dock-peek";
// A swipe this far up from the handle summons the Dock; shorter is a tap.
export const DOCK_HANDLE_SWIPE_PX = 12;
// Edge fade depth while more icons hide past that end of the pinned row.
export const DOCK_EDGE_FADE_PX = 18;

let lastActiveWindowId = null;

// A window that is on screen in the current Space, ignoring the Space's own
// hidden ghosts and minimized entries.
export function hasVisibleWindow(windows = shellState.windows) {
  for (const entry of windows.values()) {
    const node = entry?.node;
    if (!node || node.classList.contains("hidden")) {
      continue;
    }
    if (node.dataset.spaceVisible === "false") {
      continue;
    }
    return true;
  }
  return false;
}

// Called after every window change (via updateTaskbarState) and on resize.
export function syncPhoneDock(doc = document, view = window) {
  const body = doc.body;
  const launcherOpen = launcher ? !launcher.hidden : false;
  const tucked = isPhone(view) && hasVisibleWindow() && !launcherOpen;
  body.classList.toggle(PHONE_DOCK_TUCKED_CLASS, tucked);
  // A peek ends when the tucked state ends, or when the user picks a window
  // from it (focus moved) — not on unrelated summary refreshes.
  const focusChanged = shellState.activeWindowId !== lastActiveWindowId;
  lastActiveWindowId = shellState.activeWindowId;
  if (!tucked || focusChanged) {
    body.classList.remove(PHONE_DOCK_PEEK_CLASS);
  }
  const handle = doc.querySelector("#phone-dock-handle");
  if (handle) {
    handle.hidden = !tucked;
  }
}

export function peekPhoneDock(doc = document) {
  if (!doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS)) {
    return;
  }
  doc.body.classList.add(PHONE_DOCK_PEEK_CLASS);
  doc.querySelector(".taskbar-item[data-target]")?.focus?.({ preventScroll: true });
}

export function hidePhoneDockPeek(doc = document) {
  doc.body.classList.remove(PHONE_DOCK_PEEK_CLASS);
}

// Fade the pinned row's edges only on the side(s) with hidden icons.
export function syncDockEdgeFades(row = taskbarTargets) {
  if (!row) {
    return;
  }
  const hiddenLeft = row.scrollLeft > 1;
  const hiddenRight = row.scrollWidth - row.clientWidth - row.scrollLeft > 1;
  row.style.setProperty("--dock-fade-start", hiddenLeft ? `${DOCK_EDGE_FADE_PX}px` : "0px");
  row.style.setProperty("--dock-fade-end", hiddenRight ? `${DOCK_EDGE_FADE_PX}px` : "0px");
}

function bindHandle(handle, doc) {
  let pressStartY = null;
  handle.addEventListener("click", () => peekPhoneDock(doc));
  handle.addEventListener("pointerdown", (event) => {
    pressStartY = event.clientY;
  });
  handle.addEventListener("pointermove", (event) => {
    if (pressStartY === null) {
      return;
    }
    if (pressStartY - event.clientY >= DOCK_HANDLE_SWIPE_PX) {
      pressStartY = null;
      peekPhoneDock(doc);
    }
  });
  const release = () => {
    pressStartY = null;
  };
  handle.addEventListener("pointerup", release);
  handle.addEventListener("pointercancel", release);
}

export function bindPhoneDock(doc = document, view = window) {
  const handle = doc.querySelector("#phone-dock-handle");
  if (handle) {
    bindHandle(handle, doc);
  }
  doc.querySelector("#phone-dock-scrim")?.addEventListener("click", () => hidePhoneDockPeek(doc));
  doc.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS)) {
      hidePhoneDockPeek(doc);
    }
  });
  view.addEventListener("resize", () => syncPhoneDock(doc, view));
  if (taskbarTargets) {
    taskbarTargets.addEventListener("scroll", () => syncDockEdgeFades(taskbarTargets), { passive: true });
    new view.ResizeObserver(() => syncDockEdgeFades(taskbarTargets)).observe(taskbarTargets);
  }
  syncPhoneDock(doc, view);
  syncDockEdgeFades(taskbarTargets);
}
