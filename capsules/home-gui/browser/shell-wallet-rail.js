import {
  escapeHtml,
  fetchJson,
  shellState,
  targetById,
} from "./shell-core.js?v=home-20260718m";
import {
  iframeAllowForLaunch,
  iframeSandboxForLaunch,
  openTarget,
} from "./shell-windows.js?v=home-20260718m";

/* Wallet rail: a right-hand slide-over that hosts the wallet capsule.
   Chrome only — it launches the wallet through the exact same canonical
   path as a window (POST /api/apps/home/launch) and mounts the returned
   route in an iframe with the same sandbox/allow policy windows compute.
   The shell holds no wallet state and no wallet logic; approvals and
   accounts stay inside the capsule where the authority lives.

   The frame keeps its session across open/close (hide, don't unload) and
   is torn down only when the host retires the GUI surface. */

const EYE_OPEN_SVG =
  '<svg class="wallet-rail-icon-svg" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M1.75 8s2.5-3.75 6.25-3.75S14.25 8 14.25 8s-2.5 3.75-6.25 3.75S1.75 8 1.75 8Z" /><circle cx="8" cy="8" r="1.75" /></svg>';

const EYE_HIDDEN_SVG =
  '<svg class="wallet-rail-icon-svg" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.25 2.25l11.5 11.5" /><path d="M6.6 6.7A1.75 1.75 0 0 0 9.3 9.4" /><path d="M4.2 4.55C2.85 5.55 1.75 8 1.75 8s2.5 3.75 6.25 3.75c1.05 0 2-.3 2.8-.75" /><path d="M7.1 4.35c.3-.05.6-.1.9-.1 3.75 0 6.25 3.75 6.25 3.75a12.4 12.4 0 0 1-1.55 1.7" /></svg>';

let rail = null;
let frame = null;
let barButton = null;
let closeButton = null;
let windowButton = null;
let approvalsButton = null;
let settingsButton = null;
let privacyButton = null;
let approvalsBadge = null;
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
let privacyHidden = false;
let frameReady = false;
let queuedChromeCommand = "";
let openedByEdgeHover = false;
let edgeHoverBound = false;
let edgeOpenTimer = 0;
let edgeCloseTimer = 0;
let edgeOpenedAt = 0;
let lastPointerClientX = 0;
/* Set from summary sync — do not rely only on this module's shellState copy. */
let walletTargetAvailable = false;
let preloadTimer = 0;

const EDGE_REVEAL_PX = 16;
const EDGE_OPEN_MS = 100;
const EDGE_CLOSE_MS = 220;
/* Cover the enter slide so a gutter pointer / transform rect can't auto-close. */
const EDGE_OPEN_GRACE_MS = 320;
const PRELOAD_IDLE_MS = 2000;

export function bindWalletRail() {
  if (rail) {
    return;
  }
  rail = document.querySelector("#wallet-rail");
  frame = document.querySelector("#wallet-rail-frame");
  barButton = document.querySelector("#toolbar-wallet");
  closeButton = document.querySelector("#wallet-rail-close");
  windowButton = document.querySelector("#wallet-rail-open-window");
  approvalsButton = document.querySelector("#wallet-rail-approvals");
  settingsButton = document.querySelector("#wallet-rail-settings");
  privacyButton = document.querySelector("#wallet-rail-privacy");
  approvalsBadge = document.querySelector("#wallet-rail-approvals-badge");
  errorBlock = document.querySelector("#wallet-rail-error");
  errorDetail = document.querySelector("#wallet-rail-error-detail");
  retryButton = document.querySelector("#wallet-rail-retry");
  if (!rail || !frame) {
    return;
  }
  barButton?.addEventListener("click", () => {
    // Toolbar is an intentional pin — not an edge peek session.
    openedByEdgeHover = false;
    clearEdgeTimers();
    toggleWalletRail();
  });
  closeButton?.addEventListener("click", () => {
    openedByEdgeHover = false;
    hideWalletRail();
  });
  windowButton?.addEventListener("click", () => {
    // Single session: tear down the rail frame before the window launch so
    // two home_tokens never race.
    openedByEdgeHover = false;
    retireWalletRail();
    openTarget("wallet");
  });
  approvalsButton?.addEventListener("click", () => {
    postWalletChromeCommand("open-activity");
  });
  settingsButton?.addEventListener("click", () => {
    postWalletChromeCommand("open-settings");
  });
  privacyButton?.addEventListener("click", () => {
    postWalletChromeCommand("toggle-privacy");
  });
  retryButton?.addEventListener("click", () => {
    void mountWalletFrame();
  });
  window.addEventListener("message", onWalletChromeMessage);
  rail.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      // Close in-capsule drawers/modals before hiding — focus often sits on
      // the rail head after Activity/Settings, so iframe Escape never runs.
      openedByEdgeHover = false;
      postWalletChromeCommand("close-overlays");
      hideWalletRail();
    }
  });
  rail.addEventListener("pointerenter", () => {
    clearEdgeCloseTimer();
  });
  rail.addEventListener("pointerleave", (event) => {
    if (!openedByEdgeHover) {
      return;
    }
    // Leaving into the iframe/chrome still counts as inside when relatedTarget
    // is contained; only schedule close when the pointer truly left the rail.
    if (event.relatedTarget && rail.contains(event.relatedTarget)) {
      return;
    }
    // Far-right gutter sits outside the plate (rail is inset 8px). Stay open
    // while the pointer is still in that strip or over the rail column.
    if (pointerInEdgeKeepZone(event.clientX)) {
      clearEdgeCloseTimer();
      return;
    }
    scheduleEdgeClose();
  });
  bindEdgeReveal();
  syncPrivacyButton();
}

function finePointerHoverAvailable() {
  return !window.matchMedia?.("(hover: none), (pointer: coarse)")?.matches;
}

function connectorSheetOpen() {
  const connectorSheet = document.querySelector("#connector-sheet");
  return Boolean(connectorSheet && !connectorSheet.hidden);
}

function clearEdgeOpenTimer() {
  if (edgeOpenTimer) {
    window.clearTimeout(edgeOpenTimer);
    edgeOpenTimer = 0;
  }
}

function clearEdgeCloseTimer() {
  if (edgeCloseTimer) {
    window.clearTimeout(edgeCloseTimer);
    edgeCloseTimer = 0;
  }
}

function clearEdgeTimers() {
  clearEdgeOpenTimer();
  clearEdgeCloseTimer();
}

function scheduleEdgeClose() {
  if (!openedByEdgeHover || edgeCloseTimer) {
    return;
  }
  // Leave during open-grace: still schedule close after grace ends + settle delay
  // so a parked pointer outside the strip cannot leave the rail stuck open.
  const graceLeft = Math.max(0, EDGE_OPEN_GRACE_MS - (Date.now() - edgeOpenedAt));
  const delay = graceLeft + EDGE_CLOSE_MS;
  edgeCloseTimer = window.setTimeout(() => {
    edgeCloseTimer = 0;
    if (!openedByEdgeHover || !walletRailOpen()) {
      return;
    }
    // Last-chance: pointer may still sit in the far-right gutter / rail column.
    if (pointerInEdgeKeepZone(lastPointerClientX)) {
      return;
    }
    openedByEdgeHover = false;
    hideWalletRail({ restoreFocus: false });
  }, delay);
}

function inEdgeOpenGrace() {
  return openedByEdgeHover && Date.now() - edgeOpenedAt < EDGE_OPEN_GRACE_MS;
}

/* Keep open while over the rail, the inset gutter to the viewport edge, or
   the reveal hot-zone — including mid enter-animation when the plate's
   transformed rect is still mostly off-screen to the right. */
function pointerInEdgeKeepZone(clientX) {
  if (!Number.isFinite(clientX)) {
    return false;
  }
  const fromRight = window.innerWidth - clientX;
  if (fromRight <= EDGE_REVEAL_PX) {
    return true;
  }
  if (!rail || !walletRailOpen()) {
    return false;
  }
  if (inEdgeOpenGrace()) {
    return true;
  }
  const rect = rail.getBoundingClientRect();
  // Layout left (ignore transform) so the keep zone is the resting column.
  const layoutLeft = window.innerWidth - 8 - rect.width;
  return clientX >= layoutLeft - 2;
}

function bindEdgeReveal() {
  if (edgeHoverBound) {
    return;
  }
  edgeHoverBound = true;
  document.addEventListener(
    "pointermove",
    (event) => {
      if (!finePointerHoverAvailable()) {
        return;
      }
      if (event.pointerType && event.pointerType !== "mouse") {
        return;
      }
      lastPointerClientX = event.clientX;
      if (!walletRailAvailable() || connectorSheetOpen()) {
        clearEdgeOpenTimer();
        return;
      }
      const fromRight = window.innerWidth - event.clientX;
      const inEdge = fromRight <= EDGE_REVEAL_PX;
      if (inEdge) {
        clearEdgeCloseTimer();
        if (!walletRailOpen() && !edgeOpenTimer) {
          edgeOpenTimer = window.setTimeout(() => {
            edgeOpenTimer = 0;
            if (!walletRailAvailable() || connectorSheetOpen() || walletRailOpen()) {
              return;
            }
            openedByEdgeHover = true;
            edgeOpenedAt = Date.now();
            showWalletRail({ fromEdgeHover: true });
          }, EDGE_OPEN_MS);
        }
        return;
      }
      clearEdgeOpenTimer();
      if (!openedByEdgeHover || !walletRailOpen()) {
        return;
      }
      // Stay out while pointer is on the rail column or right gutter; close
      // only after leaving that whole strip to the left.
      if (pointerInEdgeKeepZone(event.clientX)) {
        clearEdgeCloseTimer();
      } else {
        scheduleEdgeClose();
      }
    },
    { passive: true },
  );
}

function postWalletChromeCommand(cmd) {
  if (!cmd) {
    return;
  }
  const target = frame?.contentWindow;
  if (!target || !frame.dataset.route || !frameReady) {
    queuedChromeCommand = cmd;
    return;
  }
  target.postMessage(
    {
      type: "elastos:wallet-chrome-command",
      cmd,
    },
    window.location.origin,
  );
}

function flushQueuedChromeCommand() {
  if (!queuedChromeCommand || !frameReady) {
    return;
  }
  const cmd = queuedChromeCommand;
  queuedChromeCommand = "";
  postWalletChromeCommand(cmd);
}

function markFrameReady() {
  if (frameReady) {
    flushQueuedChromeCommand();
    return;
  }
  frameReady = true;
  frame?.classList.add("is-ready");
  flushQueuedChromeCommand();
}

function onWalletChromeMessage(event) {
  if (event.origin !== window.location.origin) {
    return;
  }
  // Fail closed: only the mounted Wallet rail frame may drive chrome badges.
  if (!frame?.contentWindow || event.source !== frame.contentWindow) {
    return;
  }
  const message = event.data || {};
  if (message.type === "wallet:privacy-state") {
    markFrameReady();
    privacyHidden = message.privacyMode === true;
    syncPrivacyButton();
    return;
  }
  if (message.type === "wallet:pending-count") {
    markFrameReady();
    syncRailApprovalsBadge(Number(message.count) || 0);
  }
}

function syncPrivacyButton() {
  if (!privacyButton) {
    return;
  }
  const label = privacyHidden ? "Show balances" : "Hide balances";
  privacyButton.setAttribute("aria-label", label);
  privacyButton.title = label;
  privacyButton.setAttribute("aria-pressed", privacyHidden ? "true" : "false");
  privacyButton.classList.toggle("is-active", privacyHidden);
  privacyButton.innerHTML = privacyHidden ? EYE_HIDDEN_SVG : EYE_OPEN_SVG;
}

function syncRailApprovalsBadge(count) {
  if (!approvalsBadge || !approvalsButton) {
    return;
  }
  const n = Math.max(0, Number(count) || 0);
  if (n <= 0) {
    approvalsBadge.hidden = true;
    approvalsBadge.textContent = "";
    approvalsButton.setAttribute("aria-label", "Activity");
    return;
  }
  approvalsBadge.hidden = false;
  approvalsBadge.textContent = n > 9 ? "9+" : String(n);
  approvalsButton.setAttribute(
    "aria-label",
    n === 1 ? "Activity, 1 pending" : `Activity, ${n} pending`,
  );
}

export function walletRailOpen() {
  return Boolean(rail) && !rail.hidden;
}

export function walletRailSessionMounted() {
  return Boolean(frame?.dataset?.route);
}

export function walletRailAvailable() {
  return walletTargetAvailable
    || Boolean(targetById(shellState.currentSummary, "wallet"))
    || Boolean(barButton && !barButton.hidden);
}

export function walletRailFrame() {
  return frame;
}

/* Warm the Wallet iframe while the rail stays hidden — one launch, no remount on peek. */
export function preloadWalletRail() {
  if (!walletRailAvailable() || !frame || frame.dataset.route || launching) {
    return;
  }
  void mountWalletFrame();
}

function clearPreloadTimer() {
  if (preloadTimer) {
    window.clearTimeout(preloadTimer);
    preloadTimer = 0;
  }
}

function scheduleIdlePreload() {
  clearPreloadTimer();
  if (!walletRailAvailable() || frame?.dataset?.route || launching) {
    return;
  }
  preloadTimer = window.setTimeout(() => {
    preloadTimer = 0;
    preloadWalletRail();
  }, PRELOAD_IDLE_MS);
}

function postWalletSoftRefresh() {
  const target = frame?.contentWindow;
  if (!target || !frame.dataset.route || !frameReady) {
    return;
  }
  try {
    target.postMessage(
      {
        type: "elastos:wallet-refresh",
        schema: "elastos.home.wallet-refresh/v1",
      },
      window.location.origin,
    );
  } catch (_error) {
    // Frame may be unloaded or mid-nav.
  }
}

/* Pending wallet approvals from summary facts — badge only, no authority. */
export function syncWalletRailAvailability(summary = shellState.currentSummary) {
  if (!barButton) {
    return;
  }
  const available = Boolean(targetById(summary, "wallet"));
  walletTargetAvailable = available;
  barButton.hidden = !available;
  if (!available) {
    clearPreloadTimer();
    retireWalletRail();
    syncWalletBarBadge(0);
    syncRailApprovalsBadge(0);
    return;
  }
  const pending = walletApprovalPendingCount(summary);
  syncWalletBarBadge(pending);
  // Seed Activity badge from summary until the capsule posts wallet:pending-count.
  syncRailApprovalsBadge(pending);
  scheduleIdlePreload();
}

function walletApprovalPendingCount(summary) {
  const entries = Array.isArray(summary?.notifications?.entries)
    ? summary.notifications.entries
    : [];
  return entries.filter((entry) => {
    const actionId = entry?.action_ref?.action_id;
    return (
      entry?.kind === "wallet_approval_request" &&
      typeof actionId === "string" &&
      actionId.startsWith("wallet-approve-request:")
    );
  }).length;
}

function syncWalletBarBadge(count) {
  if (!barButton) {
    return;
  }
  let badge = barButton.querySelector(".toolbar-wallet-count");
  if (count <= 0) {
    if (badge) {
      badge.hidden = true;
      badge.textContent = "";
    }
    barButton.setAttribute(
      "aria-label",
      walletRailOpen() ? "Wallet. Close" : "Wallet",
    );
    return;
  }
  if (!badge) {
    badge = document.createElement("span");
    badge.className = "toolbar-wallet-count";
    badge.setAttribute("aria-hidden", "true");
    barButton.appendChild(badge);
  }
  badge.hidden = false;
  badge.textContent = count > 99 ? "99+" : String(count);
  barButton.setAttribute(
    "aria-label",
    `Wallet. ${count} pending approval${count === 1 ? "" : "s"}`,
  );
}

export function showWalletRail(options = {}) {
  if (!rail || !walletRailAvailable()) {
    return;
  }
  const fromEdgeHover = options.fromEdgeHover === true;
  if (!fromEdgeHover) {
    openedByEdgeHover = false;
    clearEdgeTimers();
  }
  cancelHideAnimation();
  invoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  rail.hidden = false;
  rail.inert = false;
  rail.setAttribute("aria-hidden", "false");
  barButton?.setAttribute("aria-expanded", "true");
  // Retrigger the enter motion when reopening after a leave (or mid-leave cancel).
  rail.classList.remove("wallet-rail-leaving");
  rail.style.animation = "none";
  void rail.offsetWidth;
  rail.style.animation = "";
  bindOutsideDismiss();
  // Edge peek should not yank keyboard focus off the desktop / an open window.
  if (!fromEdgeHover) {
    rail.focus({ preventScroll: true });
  }
  syncRailApprovalsBadge(walletApprovalPendingCount(shellState.currentSummary));
  clearPreloadTimer();
  if (!frame.dataset.route) {
    void mountWalletFrame();
  } else if (frameReady) {
    flushQueuedChromeCommand();
    // Warm reopen: refresh balances/pending without remounting.
    postWalletSoftRefresh();
  }
}

export function hideWalletRail({ restoreFocus = true, animate = true } = {}) {
  if (!rail || rail.hidden) {
    return;
  }
  clearEdgeTimers();
  if (hideAnimating) {
    pendingRestoreFocus = pendingRestoreFocus && restoreFocus;
    return;
  }
  pendingRestoreFocus = restoreFocus;
  const reduceMotion = Boolean(
    window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches,
  );
  if (!animate || reduceMotion) {
    finishHideWalletRail();
    return;
  }
  hideAnimating = true;
  rail.classList.add("wallet-rail-leaving");
  hideEndHandler = (event) => {
    if (event.target !== rail) {
      return;
    }
    finishHideWalletRail();
  };
  rail.addEventListener("animationend", hideEndHandler);
  hideFinishTimer = window.setTimeout(() => {
    finishHideWalletRail();
  }, 280);
}

function finishHideWalletRail() {
  if (!rail || rail.hidden) {
    cancelHideAnimation();
    return;
  }
  cancelHideAnimation();
  openedByEdgeHover = false;
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

export function toggleWalletRail() {
  if (hideAnimating) {
    // Mid-leave: treat as reopen (cancel the exit motion).
    showWalletRail();
    return;
  }
  if (walletRailOpen()) {
    hideWalletRail();
  } else {
    showWalletRail();
  }
}

/* Full teardown for shell switches: the capsule session must not survive the
   GUI going dormant. */
export function retireWalletRail() {
  clearPreloadTimer();
  hideWalletRail({ restoreFocus: false, animate: false });
  queuedChromeCommand = "";
  frameReady = false;
  privacyHidden = false;
  syncPrivacyButton();
  syncRailApprovalsBadge(0);
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

async function mountWalletFrame() {
  if (launching) {
    return;
  }
  launching = true;
  frameReady = false;
  queuedChromeCommand = queuedChromeCommand || "";
  if (errorBlock) {
    errorBlock.hidden = true;
  }
  frame.hidden = false;
  frame.classList.remove("is-ready");
  try {
    const launched = await fetchJson("/api/apps/home/launch", {
      method: "POST",
      body: JSON.stringify({ target: "wallet", query: {} }),
    });
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
    frame.title = escapeHtml(launched.title || "Wallet");
    frame.addEventListener(
      "load",
      () => {
        markFrameReady();
      },
      { once: true },
    );
    // Mark rail presentation so the capsule hides duplicate Activity/Settings/Hide
    // chrome (those live in this head). Window launches omit the flag.
    const route = new URL(String(launched.route || ""), window.location.origin);
    route.searchParams.set("presentation", "rail");
    frame.src = route.href;
    frame.dataset.route = route.href;
  } catch (error) {
    frame.hidden = true;
    frameReady = false;
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
    if (!walletRailOpen() || rail.contains(event.target)) {
      return;
    }
    // The connector ceremony sheet sits above the rail — ignore outside
    // dismiss while it is open (and never treat sheet clicks as outside).
    if (connectorSheetOpen()) {
      return;
    }
    openedByEdgeHover = false;
    hideWalletRail({ restoreFocus: false });
  });
}
