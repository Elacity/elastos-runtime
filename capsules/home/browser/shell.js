import {
  desktop,
  desktopBackdrop,
  desktopShortcuts,
  desktopWorkspace,
  desktopContextMenu,
  launcher,
  launcherSearch,
  launcherToggleButton,
  closeLauncherButton,
  toolbarHomeButton,
  toolbarInboxButton,
  toolbarFullscreenButton,
  SHELL_APP_ID,
  shellState,
  fetchJson,
  initializeShellLayout,
  initializeRecentTargets,
  shouldIgnoreDesktopKeydown,
  targetById,
} from "./shell-core.js?v=home-20260427b";
import {
  syncIdentity,
  clearIdentitySurface,
  updateClock,
} from "./shell-chrome.js?v=home-20260427b";
import {
  renderDesktop,
  renderTaskbar,
  renderLauncher,
  renderInboxBadge,
  refreshLauncherIfVisible,
  updateTaskbarState,
  toggleLauncher,
  hideLauncher,
  filterLauncherItems,
  moveLauncherSelection,
  openSelectedLauncherTarget,
  clearDesktopSelection,
  continueTargetDrag,
  finishTargetDrag,
  openDesktopContextMenu,
  hideDesktopContextMenu,
  handleContextAction,
} from "./shell-surface.js?v=home-20260427b";
import {
  configureWindowHooks,
  renderBootError,
  showDesktopHome,
  openTarget,
  focusWindow,
  closeWindow,
  restoreShellSession,
  cleanupBeforeUnload,
  handleShellResize,
} from "./shell-windows.js?v=home-20260427b";

configureWindowHooks({
  clearIdentitySurface,
  hideLauncher,
  refreshLauncherIfVisible,
  renderDesktop,
  renderTaskbar,
  updateTaskbarState,
});

const SUMMARY_REFRESH_MS = 2500;
const SHELL_MESSAGE_OPEN_TARGET_SOURCES = Object.freeze({
  "chat-room": new Set(["library"]),
  inbox: "visible-target",
  library: new Set(["documents"]),
});
const SHELL_MESSAGE_OPEN_URI_SOURCES = new Set(["documents", "chat-room"]);
const SHELL_MESSAGE_DELIVER_TARGET_SOURCES = Object.freeze({
  library: new Set(["chat-room"]),
});

function fullscreenElement() {
  return document.fullscreenElement || document.webkitFullscreenElement || null;
}

function fullscreenApi() {
  const root = document.documentElement;
  const request = root.requestFullscreen || root.webkitRequestFullscreen;
  const exit = document.exitFullscreen || document.webkitExitFullscreen;
  return { root, request, exit };
}

function syncFullscreenButton() {
  if (!toolbarFullscreenButton) {
    return;
  }
  const active = Boolean(fullscreenElement());
  toolbarFullscreenButton.setAttribute("aria-pressed", active ? "true" : "false");
  toolbarFullscreenButton.setAttribute("aria-label", active ? "Exit fullscreen" : "Enter fullscreen");
  toolbarFullscreenButton.title = active ? "Exit fullscreen" : "Fullscreen";
}

function toggleShellFullscreen() {
  const { root, request, exit } = fullscreenApi();
  if (!request || !exit) {
    return;
  }
  if (fullscreenElement()) {
    const exitResult = exit.call(document);
    exitResult?.catch?.(() => {});
    return;
  }
  const requestResult = request.call(root);
  requestResult?.catch?.(() => {});
}

function registerHomeServiceWorker() {
  if (!("serviceWorker" in navigator)) {
    return;
  }
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("./service-worker.js", { scope: "./" }).catch((error) => {
      console.warn("ElastOS Home service worker registration failed", error);
    });
  }, { once: true });
}

function trackPointerDown(event) {
  shellState.lastPointer = {
    x: event.clientX,
    y: event.clientY,
    at: window.performance ? window.performance.now() : Date.now(),
  };
}

function trackPointerMove(event) {
  const at = window.performance ? window.performance.now() : Date.now();
  shellState.lastPointer = {
    x: event.clientX,
    y: event.clientY,
    at,
  };
  shellState.lastPointerMove = {
    x: event.clientX,
    y: event.clientY,
    at,
  };
}

boot().catch((error) => {
  document.body.dataset.homeStatus = "error";
  console.error("home boot failed", error);
  renderBootError(error);
});

registerHomeServiceWorker();

toolbarHomeButton.addEventListener("click", () => {
  showDesktopHome();
});

toolbarInboxButton.addEventListener("click", () => {
  if (!targetById(shellState.currentSummary, "inbox")) {
    return;
  }
  openTarget("inbox");
});

if (toolbarFullscreenButton) {
  const { request, exit } = fullscreenApi();
  if (!request || !exit) {
    toolbarFullscreenButton.disabled = true;
    toolbarFullscreenButton.title = "Fullscreen is not available in this browser";
  } else {
    toolbarFullscreenButton.addEventListener("click", toggleShellFullscreen);
    document.addEventListener("fullscreenchange", syncFullscreenButton);
    document.addEventListener("webkitfullscreenchange", syncFullscreenButton);
    syncFullscreenButton();
  }
}

launcherToggleButton.addEventListener("click", () => {
  toggleLauncher();
});

closeLauncherButton.addEventListener("click", () => {
  hideLauncher();
});

launcherSearch.addEventListener("input", () => {
  filterLauncherItems(launcherSearch.value);
});

launcherSearch.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") {
    event.preventDefault();
    moveLauncherSelection(1);
    return;
  }
  if (event.key === "ArrowUp") {
    event.preventDefault();
    moveLauncherSelection(-1);
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    openSelectedLauncherTarget();
  }
});

desktopShortcuts.addEventListener("keydown", (event) => {
  if (shouldIgnoreDesktopKeydown(event)) {
    return;
  }
  if (event.key === "Escape") {
    event.preventDefault();
    clearDesktopSelection();
    return;
  }
  if ((event.key === "Enter" || event.key === " ") && shellState.selectedDesktopTargetId) {
    event.preventDefault();
    event.stopPropagation();
    openTarget(shellState.selectedDesktopTargetId);
  }
});

document.addEventListener("pointermove", (event) => {
  trackPointerMove(event);
  continueTargetDrag(event);
});

document.addEventListener("pointerup", (event) => {
  finishTargetDrag(event);
});

document.addEventListener("pointercancel", (event) => {
  finishTargetDrag(event);
});

document.addEventListener("pointerdown", (event) => {
  trackPointerDown(event);
  const now = window.performance ? window.performance.now() : Date.now();
  if (
    shellState.contextMenuOpen &&
    now >= shellState.contextMenuIgnoreOutsideUntil &&
    !event.target.closest("#desktop-context-menu")
  ) {
    hideDesktopContextMenu();
  }
  if (launcher.hidden) {
    return;
  }
  if (
    shellState.launcherIgnoreOutsideUntil > 0 &&
    now < shellState.launcherIgnoreOutsideUntil &&
    event.target.closest("#launcher")
  ) {
    return;
  }
  if (
    event.target.closest("#desktop-context-menu") ||
    event.target.closest(".launcher-popover") ||
    event.target.closest("#launcher-toggle")
  ) {
    return;
  }
  hideLauncher();
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && shellState.contextMenuOpen) {
    hideDesktopContextMenu();
  }
  if (event.key === "Escape" && !launcher.hidden) {
    hideLauncher();
    return;
  }
  if (shouldIgnoreDesktopKeydown(event)) {
    return;
  }
  if (event.key === "Escape") {
    clearDesktopSelection();
    return;
  }
  if ((event.key === "Enter" || event.key === " ") && shellState.selectedDesktopTargetId) {
    event.preventDefault();
    openTarget(shellState.selectedDesktopTargetId);
  }
});

desktopWorkspace.addEventListener("contextmenu", (event) => {
  if (
    event.target.closest(".window") ||
    event.target.closest("#launcher") ||
    event.target.closest(".desktop-shortcut") ||
    event.target.closest(".taskbar-item[data-target]")
  ) {
    return;
  }
  event.preventDefault();
  openDesktopContextMenu(event.clientX, event.clientY, { kind: "desktop" });
});

desktop.addEventListener("pointerdown", (event) => {
  if (
    event.target.closest(".desktop-shortcut") ||
    event.target.closest(".window") ||
    event.target.closest("#desktop-context-menu") ||
    event.target.closest("#launcher")
  ) {
    return;
  }
  clearDesktopSelection();
});

desktopContextMenu.addEventListener("click", (event) => {
  const item = event.target.closest("[data-context-action]");
  if (!item) {
    return;
  }
  hideDesktopContextMenu();
  handleContextAction(item.dataset.contextAction);
});

window.addEventListener("beforeunload", () => {
  cleanupBeforeUnload();
});

window.addEventListener("resize", () => {
  handleShellResize();
});

window.addEventListener("message", (event) => {
  const data = event.data;
  const context = homeMessageContext(event, data);
  if (!context) {
    return;
  }
  if (data.type === "home:refresh-summary") {
    refreshShellSummary().catch((error) => {
      console.error("home summary refresh failed", error);
    });
    return;
  }
  if (data.type === "home:open-uri") {
    if (!canOpenUriFromHomeMessage(context)) {
      console.warn("home ignored unauthorized open-uri message", context.targetId);
      return;
    }
    const resolved = resolveOpenUri(data);
    if (!resolved) {
      console.warn("home could not resolve URI", data.uri);
      return;
    }
    if (!targetById(shellState.currentSummary, resolved.target)) {
      console.warn("home could not open URI because its viewer is not installed", data.uri);
      return;
    }
    openTarget(resolved.target, { query: resolved.query });
    return;
  }
  if (data.type === "home:deliver-to-target") {
    const target = typeof data.target === "string" ? data.target.trim() : "";
    if (!target || !canDeliverTargetFromHomeMessage(context, target)) {
      console.warn("home ignored unauthorized deliver-to-target message", context.targetId, target);
      return;
    }
    const payload = data.payload && typeof data.payload === "object" ? data.payload : null;
    if (!payload || typeof payload.type !== "string") {
      console.warn("home ignored malformed deliver-to-target payload", context.targetId, target);
      return;
    }
    if (!deliverMessageToTargetFrame(target, payload)) {
      console.warn("home could not deliver message to target", target);
    }
    return;
  }
  if (data.type === "home:close-self") {
    if (context.kind !== "app-frame" || !context.windowId) {
      console.warn("home ignored unauthorized close-self message", context.targetId);
      return;
    }
    closeWindow(context.windowId);
    return;
  }
  if (data.type !== "home:open-target") {
    return;
  }
  const target = typeof data.target === "string" ? data.target.trim() : "";
  if (!target) {
    return;
  }
  if (!canOpenTargetFromHomeMessage(context, target)) {
    console.warn("home ignored unauthorized open-target message", context.targetId, target);
    return;
  }
  const query = data.query && typeof data.query === "object" ? data.query : {};
  openTarget(target, { query });
});

function homeMessageContext(event, data) {
  if (event.origin !== window.location.origin || !data || typeof data !== "object") {
    return null;
  }
  if (event.source === window) {
    return { kind: "home", targetId: SHELL_APP_ID };
  }
  const homeToken = typeof data.homeToken === "string" ? data.homeToken.trim() : "";
  if (!homeToken) {
    return null;
  }
  for (const frame of document.querySelectorAll(".window[data-target] .window-frame")) {
    let frameWindow = null;
    try {
      frameWindow = frame.contentWindow;
    } catch (_error) {
      continue;
    }
    if (frameWindow !== event.source) {
      continue;
    }
    const expectedToken = homeLaunchTokenFromRoute(
      frame.dataset.route || frame.getAttribute("src") || "",
    );
    if (!expectedToken || expectedToken !== homeToken) {
      return null;
    }
    const windowNode = frame.closest(".window[data-target]");
    const targetId = typeof windowNode?.dataset?.target === "string"
      ? windowNode.dataset.target
      : "";
    const windowId = typeof windowNode?.dataset?.windowId === "string"
      ? windowNode.dataset.windowId
      : "";
    return { kind: "app-frame", targetId, windowId };
  }
  return null;
}

function homeLaunchTokenFromRoute(route) {
  try {
    return new URL(route, window.location.href).searchParams.get("home_token") || "";
  } catch (_error) {
    return "";
  }
}

function canOpenUriFromHomeMessage(context) {
  return context.kind === "home" || SHELL_MESSAGE_OPEN_URI_SOURCES.has(context.targetId);
}

function canOpenTargetFromHomeMessage(context, target) {
  if (context.kind === "home") {
    return true;
  }
  const policy = SHELL_MESSAGE_OPEN_TARGET_SOURCES[context.targetId];
  if (!policy) {
    return false;
  }
  if (policy === "visible-target") {
    return !!targetById(shellState.currentSummary, target) && target !== SHELL_APP_ID;
  }
  return policy.has(target);
}

function canDeliverTargetFromHomeMessage(context, target) {
  if (context.kind !== "app-frame") {
    return false;
  }
  const policy = SHELL_MESSAGE_DELIVER_TARGET_SOURCES[context.targetId];
  return !!policy && policy.has(target);
}

function deliverMessageToTargetFrame(target, payload) {
  const entries = [...shellState.windows.values()]
    .filter((entry) => entry.kind === "browser" && entry.targetId === target)
    .sort((left, right) => Number(right.serial || 0) - Number(left.serial || 0));
  const entry = entries.find((candidate) => !candidate.node.classList.contains("hidden")) || entries[0];
  const frame = entry?.node?.querySelector(".window-frame");
  if (!frame?.contentWindow) {
    return false;
  }
  frame.contentWindow.postMessage(payload, window.location.origin);
  focusWindow(entry.id);
  return true;
}

function resolveOpenUri(data) {
  const uri = typeof data.uri === "string" ? data.uri.trim() : "";
  if (!uri.startsWith("elastos://")) {
    return null;
  }
  const cid = uri.slice("elastos://".length).split(/[/?#]/)[0].trim();
  if (!cid) {
    return null;
  }
  const preferredViewer = typeof data.preferredViewer === "string" ? data.preferredViewer.trim() : "";
  if (preferredViewer === "documents" || preferredViewer === "") {
    return {
      target: "documents",
      query: {
        cid,
        uri,
        view: "read",
      },
    };
  }
  return null;
}

async function boot() {
  document.body.dataset.homeStatus = "booting";
  const runtimeReady = fetchJson("/api/apps/home/runtime/ensure", { method: "POST" })
    .catch((error) => {
      console.error("home runtime ensure failed", error);
      return null;
    });
  await refreshShellSummary({ initialize: true });
  await restoreShellSession();
  document.body.dataset.homeStatus = "ready";
  runtimeReady.then(() => refreshShellSummary()).catch((error) => {
    console.error("home summary refresh failed after runtime ensure", error);
  });

  updateClock();
  shellState.clockTimer = window.setInterval(updateClock, 30_000);
  window.setInterval(() => {
    refreshShellSummary().catch((error) => {
      console.error("home summary refresh failed", error);
    });
  }, SUMMARY_REFRESH_MS);
}

async function refreshShellSummary({ initialize = false } = {}) {
  const summary = await fetchJson("/api/apps/home/summary");
  const previous = shellState.currentSummary;
  shellState.currentSummary = summary;
  shellState.requestSummaryRefresh = refreshShellSummary;

  if (initialize) {
    initializeShellLayout(summary);
    initializeRecentTargets(summary);
  }

  syncIdentity(summary);
  syncAppearance(summary);
  renderInboxBadge(summary);

  if (initialize || targetsChanged(previous, summary)) {
    renderDesktop(summary);
    renderTaskbar(summary);
    renderLauncher(summary);
    return summary;
  }

  renderTaskbar(summary);
  if (!launcher.hidden) {
    renderLauncher(summary);
  }
  return summary;
}

function syncAppearance(summary) {
  const imageUrl = typeof summary?.appearance?.background_image_url === "string"
    ? summary.appearance.background_image_url.trim()
    : "";
  const overlayEnabled = summary?.appearance?.background_overlay_enabled === true;
  const overlayOpacityRaw = Number(summary?.appearance?.background_overlay_opacity);
  const overlayOpacity = Number.isFinite(overlayOpacityRaw)
    ? Math.min(0.8, Math.max(0, overlayOpacityRaw))
    : 0.55;
  if (!desktopBackdrop) {
    return;
  }
  desktopBackdrop.dataset.overlay = overlayEnabled ? "true" : "false";
  desktopBackdrop.style.setProperty("--desktop-overlay-opacity", String(overlayOpacity));
  if (!imageUrl) {
    desktopBackdrop.style.removeProperty("--desktop-wallpaper");
    return;
  }
  desktopBackdrop.style.setProperty("--desktop-wallpaper", `url("${imageUrl}")`);
}

function targetsChanged(previous, next) {
  const previousTargets = Array.isArray(previous && previous.targets)
    ? previous.targets.map((target) => `${target.target}:${target.title}:${target.description}`).join("|")
    : "";
  const nextTargets = Array.isArray(next && next.targets)
    ? next.targets.map((target) => `${target.target}:${target.title}:${target.description}`).join("|")
    : "";
  return previousTargets !== nextTargets;
}
