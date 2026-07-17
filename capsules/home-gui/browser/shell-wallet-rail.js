import {
  escapeHtml,
  shellState,
  targetById,
} from "./shell-core.js?v=home-20260718n";
import {
  iframeAllowForLaunch,
  iframeSandboxForLaunch,
  launchHomeTarget,
  openTarget,
} from "./shell-windows.js?v=home-20260718n";

/* Wallet rail: a right-hand slide-over that hosts the wallet capsule.
   Chrome only — it launches the wallet through the same host-mediated
   launchTarget path as windows (authority-carrying), then mounts the
   returned route in an iframe with the same sandbox/allow policy.
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
    hideWalletRail({ restoreFocus: false });
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

/* Called on every summary sync: the bar icon exists exactly when the home
   carries a wallet target — never a dead button. */
export function syncWalletRailAvailability() {
  if (!barButton) {
    return;
  }
  const available = walletRailAvailable();
  barButton.hidden = !available;
  if (!available) {
    retireWalletRail();
  }
}

export function showWalletRail() {
  if (!rail || !walletRailAvailable()) {
    return;
  }
  invoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  rail.hidden = false;
  rail.inert = false;
  rail.setAttribute("aria-hidden", "false");
  barButton?.setAttribute("aria-expanded", "true");
  bindOutsideDismiss();
  rail.focus({ preventScroll: true });
  if (!frame.dataset.route) {
    void mountWalletFrame();
  }
}

export function hideWalletRail({ restoreFocus = true } = {}) {
  if (!walletRailOpen()) {
    return;
  }
  rail.hidden = true;
  rail.inert = true;
  rail.setAttribute("aria-hidden", "true");
  barButton?.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    invoker?.focus?.();
  }
  invoker = null;
}

export function toggleWalletRail() {
  if (walletRailOpen()) {
    hideWalletRail();
  } else {
    showWalletRail();
  }
}

/* Full teardown for shell switches: the capsule session must not survive the
   GUI going dormant. */
export function retireWalletRail() {
  hideWalletRail({ restoreFocus: false });
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
    const launched = await launchHomeTarget("wallet", {});
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
    if (walletRailOpen() && !rail.contains(event.target)) {
      hideWalletRail({ restoreFocus: false });
    }
  });
}
