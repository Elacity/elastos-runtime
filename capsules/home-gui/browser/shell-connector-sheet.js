import {
  escapeHtml,
  pushUiPreferencesToFrameWindow,
} from "./shell-core.js?v=home-20260719y";
import {
  iframeAllowForLaunch,
  iframeSandboxForLaunch,
  launchHomeTarget,
} from "./shell-windows.js?v=home-20260719y";
import { showWalletRail, walletRailOpen } from "./shell-wallet-rail.js?v=home-20260719y";

/* Connector sheet: thin ceremony surface for wallet-metamask / unisat /
   walletconnect. Same launch path as a window, mounted in a rail-aligned
   overlay instead of a second product window. Authority stays in the
   connector capsule; this module is chrome only. */

const CONNECTOR_SHEET_TARGETS = new Set([
  "wallet-metamask",
  "wallet-unisat",
  "wallet-walletconnect",
]);

const SHEET_TITLES = {
  "wallet-metamask": "Connect MetaMask",
  "wallet-unisat": "Connect UniSat",
  "wallet-walletconnect": "Connect WalletConnect",
};

let sheet = null;
let frame = null;
let titleNode = null;
let closeButton = null;
let launching = false;
let activeTarget = "";
let bound = false;

export function bindConnectorSheet() {
  if (bound) {
    return;
  }
  sheet = document.querySelector("#connector-sheet");
  frame = document.querySelector("#connector-sheet-frame");
  titleNode = document.querySelector("#connector-sheet-title");
  closeButton = document.querySelector("#connector-sheet-close");
  if (!sheet || !frame) {
    return;
  }
  bound = true;
  closeButton?.addEventListener("click", () => hideConnectorSheet());
  sheet.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideConnectorSheet();
    }
  });
}

export function isConnectorSheetTarget(targetId) {
  return CONNECTOR_SHEET_TARGETS.has(targetId);
}

export function connectorSheetOpen() {
  return Boolean(sheet) && !sheet.hidden;
}

export function connectorSheetFrame() {
  return frame;
}

export function connectorSheetTarget() {
  return activeTarget;
}

export async function showConnectorSheet(targetId, options = {}) {
  if (!sheet || !frame || !isConnectorSheetTarget(targetId)) {
    return false;
  }
  // Connectors are often hidden from the launcher; launch fails closed via API.
  if (!walletRailOpen()) {
    showWalletRail();
  }
  activeTarget = targetId;
  if (titleNode) {
    titleNode.textContent = SHEET_TITLES[targetId] || "Connect wallet";
  }
  sheet.hidden = false;
  sheet.inert = false;
  sheet.setAttribute("aria-hidden", "false");
  sheet.focus({ preventScroll: true });
  await mountConnectorFrame(targetId, options.query || {});
  return true;
}

export function hideConnectorSheet() {
  if (!sheet || sheet.hidden) {
    return;
  }
  sheet.hidden = true;
  sheet.inert = true;
  sheet.setAttribute("aria-hidden", "true");
  if (frame) {
    frame.removeAttribute("src");
    delete frame.dataset.route;
    frame.hidden = true;
    frame.classList.remove("is-ready");
  }
  activeTarget = "";
}

export function retireConnectorSheet() {
  hideConnectorSheet();
}

/* After a successful link the connector posts home:refresh-summary (to the
   host, which relays it here token-bound). Tell every Wallet surface to
   reload accounts (summary refresh alone does not), then close the ceremony
   so the user lands back on the rail. */
export function noteConnectorSheetSummaryRefresh(homeToken) {
  if (!connectorSheetOpen() || !homeToken) {
    return;
  }
  const mountedToken = connectorLaunchTokenFromRoute(frame?.dataset?.route || "");
  if (!mountedToken || mountedToken !== homeToken) {
    return;
  }
  broadcastWalletAccountsRefresh();
  window.setTimeout(() => {
    hideConnectorSheet();
    if (walletRailOpen()) {
      showWalletRail();
    }
    // Second nudge after the sheet is gone — covers slow provider write.
    broadcastWalletAccountsRefresh();
  }, 450);
}

function connectorLaunchTokenFromRoute(route) {
  try {
    const url = new URL(route, window.location.href);
    return new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") || "";
  } catch (_error) {
    return "";
  }
}

function broadcastWalletAccountsRefresh() {
  const message = {
    type: "elastos:wallet-refresh",
    schema: "elastos.home.wallet-refresh/v1",
  };
  // Wallet frames are opaque-sandboxed (origin "null"); target must be "*".
  const post = (contentWindow) => {
    try {
      contentWindow?.postMessage(message, "*");
    } catch (_error) {
      // Frame may be unloaded or mid-nav.
    }
  };
  post(document.querySelector("#wallet-rail-frame")?.contentWindow);
  for (const node of document.querySelectorAll(".window[data-target='wallet'] .window-frame")) {
    post(node.contentWindow);
  }
}

async function mountConnectorFrame(targetId, query) {
  if (launching) {
    return;
  }
  launching = true;
  frame.hidden = false;
  frame.classList.remove("is-ready");
  try {
    const launchQuery = {
      ...normalizedQuery(query),
      presentation: "sheet",
    };
    const launched = await launchHomeTarget(targetId, launchQuery);
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
    frame.title = escapeHtml(launched.title || SHEET_TITLES[targetId] || "Connector");
    frame.addEventListener(
      "load",
      () => {
        frame.classList.add("is-ready");
        pushUiPreferencesToFrameWindow(frame.contentWindow);
      },
      { once: true },
    );
    // Ensure the connector sees presentation=sheet even if the gateway
    // does not echo query params onto the route.
    const route = withPresentationSheet(launched.route);
    frame.src = route;
    frame.dataset.route = route;
  } catch (error) {
    console.error("connector sheet launch failed", error);
    hideConnectorSheet();
    throw error;
  } finally {
    launching = false;
  }
}

function normalizedQuery(query) {
  if (!query || typeof query !== "object") {
    return {};
  }
  const next = {};
  for (const [key, value] of Object.entries(query)) {
    if (typeof key !== "string" || !key.trim()) {
      continue;
    }
    next[key] = typeof value === "string" ? value : String(value ?? "");
  }
  return next;
}

function withPresentationSheet(route) {
  try {
    const url = new URL(route, window.location.href);
    url.searchParams.set("presentation", "sheet");
    return `${url.pathname}${url.search}${url.hash}`;
  } catch (_error) {
    return route;
  }
}
