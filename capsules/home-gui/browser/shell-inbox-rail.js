import {
  escapeHtml,
  pushUiPreferencesToFrameWindow,
  shellState,
  targetById,
} from "./shell-core.js?v=home-20260804ar";
import {
  closeOtherShellPopovers,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260804ar";
import {
  iframeAllowForLaunch,
  iframeSandboxForLaunch,
  launchHomeTarget,
  openTarget,
} from "./shell-windows.js?v=home-20260804ar";
import { playUiSound } from "./shell-sounds.js?v=home-20260804ar";

/* Inbox rail: right-hand slide-over hosting the Inbox capsule with
   presentation=rail. Chrome only — launches through the same host-mediated
   path as windows; approve/deny authority stays inside the capsule frame.
   Session persists across hide/show; torn down on GUI retire or Open-window. */

let rail = null;
let frame = null;
let barButton = null;
let closeButton = null;
let windowButton = null;
let refreshButton = null;
let errorBlock = null;
let errorDetail = null;
let retryButton = null;
let invoker = null;
let launching = false;
let outsideDismissBound = false;
let hideAnimating = false;
let hideFinishTimer = 0;
let hideEndHandler = null;
let pendingRestoreFocus = true;
let frameReady = false;
let registered = false;

export function bindInboxRail() {
  if (rail) {
    return;
  }
  rail = document.querySelector("#inbox-rail");
  frame = document.querySelector("#inbox-rail-frame");
  barButton = document.querySelector("#toolbar-inbox");
  closeButton = document.querySelector("#inbox-rail-close");
  windowButton = document.querySelector("#inbox-rail-open-window");
  refreshButton = document.querySelector("#inbox-rail-refresh");
  errorBlock = document.querySelector("#inbox-rail-error");
  errorDetail = document.querySelector("#inbox-rail-error-detail");
  retryButton = document.querySelector("#inbox-rail-retry");
  if (!rail || !frame) {
    return;
  }
  if (!registered) {
    registerShellPopover("inbox-rail", () => hideInboxRail({ restoreFocus: false }));
    registered = true;
  }
  refreshButton?.addEventListener("click", () => {
    if (refreshButton.dataset.spinning === "true") {
      return;
    }
    refreshButton.dataset.spinning = "true";
    refreshButton.classList.add("is-refreshing");
    postInboxRefresh();
    window.setTimeout(() => {
      refreshButton.classList.remove("is-refreshing");
      delete refreshButton.dataset.spinning;
    }, 700);
  });
  closeButton?.addEventListener("click", () => {
    hideInboxRail();
  });
  windowButton?.addEventListener("click", () => {
    // Single session: tear down the rail frame before the window launch so
    // two home_tokens never race.
    retireInboxRail();
    openTarget("inbox");
  });
  retryButton?.addEventListener("click", () => {
    void mountInboxFrame();
  });
  window.addEventListener("message", onInboxChromeMessage);
  rail.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideInboxRail();
    }
  });
}

export function inboxRailOpen() {
  return Boolean(rail) && !rail.hidden;
}

export function inboxRailSessionMounted() {
  return Boolean(frame?.dataset?.route);
}

export function inboxRailAvailable() {
  return Boolean(targetById(shellState.currentSummary, "inbox"))
    || Boolean(barButton && !barButton.hidden);
}

export function inboxRailFrame() {
  return frame;
}

export function showInboxRail() {
  if (!rail || !inboxRailAvailable()) {
    return;
  }
  closeOtherShellPopovers("inbox-rail");
  cancelHideAnimation();
  invoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  rail.hidden = false;
  rail.inert = false;
  rail.setAttribute("aria-hidden", "false");
  barButton?.setAttribute("aria-expanded", "true");
  rail.classList.remove("wallet-rail-leaving");
  rail.style.animation = "none";
  void rail.offsetWidth;
  rail.style.animation = "";
  bindOutsideDismiss();
  rail.focus({ preventScroll: true });
  if (!frame.dataset.route) {
    void mountInboxFrame();
  } else if (frameReady) {
    postInboxRefresh();
  }
}

export function hideInboxRail({ restoreFocus = true, animate = true } = {}) {
  if (!rail || rail.hidden) {
    return;
  }
  if (hideAnimating) {
    pendingRestoreFocus = pendingRestoreFocus && restoreFocus;
    return;
  }
  pendingRestoreFocus = restoreFocus;
  const reduceMotion = Boolean(
    window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches,
  );
  if (!animate || reduceMotion) {
    finishHideInboxRail();
    return;
  }
  hideAnimating = true;
  rail.classList.add("wallet-rail-leaving");
  hideEndHandler = (event) => {
    if (event.target !== rail) {
      return;
    }
    finishHideInboxRail();
  };
  rail.addEventListener("animationend", hideEndHandler);
  hideFinishTimer = window.setTimeout(() => {
    finishHideInboxRail();
  }, 280);
}

function finishHideInboxRail() {
  if (!rail || rail.hidden) {
    cancelHideAnimation();
    return;
  }
  cancelHideAnimation();
  rail.hidden = true;
  rail.inert = true;
  rail.setAttribute("aria-hidden", "true");
  barButton?.setAttribute("aria-expanded", "false");
  if (pendingRestoreFocus) {
    invoker?.focus?.();
  }
  invoker = null;
  pendingRestoreFocus = true;
}

function cancelHideAnimation() {
  if (hideFinishTimer) {
    window.clearTimeout(hideFinishTimer);
    hideFinishTimer = 0;
  }
  if (hideEndHandler && rail) {
    rail.removeEventListener("animationend", hideEndHandler);
  }
  hideEndHandler = null;
  hideAnimating = false;
  rail?.classList.remove("wallet-rail-leaving");
}

export function toggleInboxRail() {
  if (hideAnimating) {
    showInboxRail();
    return;
  }
  if (inboxRailOpen()) {
    hideInboxRail();
  } else {
    showInboxRail();
  }
}

export function retireInboxRail() {
  hideInboxRail({ restoreFocus: false, animate: false });
  frameReady = false;
  if (frame) {
    frame.removeAttribute("src");
    delete frame.dataset.route;
    frame.hidden = true;
    frame.classList.remove("is-ready");
  }
  if (errorBlock) {
    errorBlock.hidden = true;
  }
}

function markFrameReady() {
  if (frameReady) {
    return;
  }
  frameReady = true;
  frame?.classList.add("is-ready");
  pushUiPreferencesToFrameWindow(frame?.contentWindow);
}

function postInboxRefresh() {
  const target = frame?.contentWindow;
  if (!target || !frame.dataset.route || !frameReady) {
    return;
  }
  try {
    target.postMessage(
      {
        type: "elastos:inbox-chrome-command",
        cmd: "refresh",
      },
      "*",
    );
  } catch (_error) {
    // Frame may be unloaded or mid-nav.
  }
}

function onInboxChromeMessage(event) {
  if (event.origin !== "null") {
    return;
  }
  if (!frame?.contentWindow || event.source !== frame.contentWindow) {
    return;
  }
  const message = event.data || {};
  if (message.type === "inbox:pending-count") {
    markFrameReady();
  }
}

async function mountInboxFrame() {
  if (launching) {
    return;
  }
  launching = true;
  frameReady = false;
  if (errorBlock) {
    errorBlock.hidden = true;
  }
  frame.hidden = false;
  frame.classList.remove("is-ready");
  try {
    const launched = await launchHomeTarget("inbox", {});
    if (launched.attach_kind !== "iframe") {
      throw new Error(`unsupported attach kind: ${launched.attach_kind || "unknown"}`);
    }
    if (
      typeof launched.launch_status === "string" &&
      launched.launch_status.trim() !== "" &&
      launched.launch_status !== "launched"
    ) {
      throw new Error(
        typeof launched.launch_detail === "string" && launched.launch_detail.trim() !== ""
          ? launched.launch_detail.trim()
          : `launch status: ${launched.launch_status}`,
      );
    }
    frame.setAttribute("sandbox", iframeSandboxForLaunch(launched));
    frame.setAttribute("allow", iframeAllowForLaunch(launched));
    frame.title = escapeHtml(launched.title || "Inbox");
    frame.addEventListener(
      "load",
      () => {
        markFrameReady();
      },
      { once: true },
    );
    const route = new URL(String(launched.route || ""), window.location.origin);
    route.searchParams.set("presentation", "rail");
    frame.src = route.href;
    frame.dataset.route = route.href;
  } catch (error) {
    frame.hidden = true;
    frameReady = false;
    playUiSound("error");
    if (errorBlock) {
      errorBlock.hidden = false;
      if (errorDetail) {
        errorDetail.textContent = String(error?.message || error);
      }
    }
  } finally {
    launching = false;
  }
}

function bindOutsideDismiss() {
  if (outsideDismissBound) {
    return;
  }
  outsideDismissBound = true;
  document.addEventListener("pointerdown", (event) => {
    if (!inboxRailOpen() || rail.contains(event.target)) {
      return;
    }
    if (barButton?.contains(event.target)) {
      return;
    }
    hideInboxRail({ restoreFocus: false });
  });
}
