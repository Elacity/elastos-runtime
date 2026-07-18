import {
  escapeHtml,
  fetchJson,
  shellState,
  targetById,
} from "./shell-core.js?v=home-20260718d";
import {
  iframeAllowForLaunch,
  iframeSandboxForLaunch,
  openTarget,
} from "./shell-windows.js?v=home-20260718d";

/* Wallet rail: a right-hand slide-over that hosts the wallet capsule.
   Chrome only — it launches the wallet through the exact same canonical
   path as a window (POST /api/apps/home/launch) and mounts the returned
   route in an iframe with the same sandbox/allow policy windows compute.
   The shell holds no wallet state and no wallet logic; approvals and
   accounts stay inside the capsule where the authority lives.

   The frame keeps its session across open/close (hide, don't unload) and
   is torn down only when the host retires the GUI surface. */

let rail = null;
let frame = null;
let barButton = null;
let closeButton = null;
let windowButton = null;
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

export function bindWalletRail() {
  if (rail) {
    return;
  }
  rail = document.querySelector("#wallet-rail");
  frame = document.querySelector("#wallet-rail-frame");
  barButton = document.querySelector("#toolbar-wallet");
  closeButton = document.querySelector("#wallet-rail-close");
  windowButton = document.querySelector("#wallet-rail-open-window");
  errorBlock = document.querySelector("#wallet-rail-error");
  errorDetail = document.querySelector("#wallet-rail-error-detail");
  retryButton = document.querySelector("#wallet-rail-retry");
  if (!rail || !frame) {
    return;
  }
  barButton?.addEventListener("click", () => {
    toggleWalletRail();
  });
  closeButton?.addEventListener("click", () => hideWalletRail());
  windowButton?.addEventListener("click", () => {
    // Single session: tear down the rail frame before the window launch so
    // two home_tokens never race.
    retireWalletRail();
    openTarget("wallet");
  });
  retryButton?.addEventListener("click", () => {
    void mountWalletFrame();
  });
  rail.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideWalletRail();
    }
  });
}

export function walletRailOpen() {
  return Boolean(rail) && !rail.hidden;
}

export function walletRailAvailable() {
  return Boolean(targetById(shellState.currentSummary, "wallet"));
}

export function walletRailFrame() {
  return frame;
}

/* Pending wallet approvals from summary facts — badge only, no authority. */
export function syncWalletRailAvailability(summary = shellState.currentSummary) {
  if (!barButton) {
    return;
  }
  const available = Boolean(targetById(summary, "wallet"));
  barButton.hidden = !available;
  if (!available) {
    retireWalletRail();
    syncWalletBarBadge(0);
    return;
  }
  syncWalletBarBadge(walletApprovalPendingCount(summary));
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

export function showWalletRail() {
  if (!rail || !walletRailAvailable()) {
    return;
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
  rail.focus({ preventScroll: true });
  if (!frame.dataset.route) {
    void mountWalletFrame();
  }
}

export function hideWalletRail({ restoreFocus = true, animate = true } = {}) {
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
  hideWalletRail({ restoreFocus: false, animate: false });
  if (frame) {
    frame.removeAttribute("src");
    delete frame.dataset.route;
    frame.hidden = true;
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
        frame.classList.add("is-ready");
      },
      { once: true },
    );
    frame.src = launched.route;
    frame.dataset.route = launched.route;
  } catch (error) {
    frame.hidden = true;
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
    const connectorSheet = document.querySelector("#connector-sheet");
    if (connectorSheet && !connectorSheet.hidden) {
      return;
    }
    hideWalletRail({ restoreFocus: false });
  });
}
