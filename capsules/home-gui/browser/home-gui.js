import {
  desktop,
  desktopBackdrop,
  desktopShortcuts,
  desktopContextMenu,
  desktopWorkspace,
  launcher,
  launcherSearch,
  launcherToggleButton,
  closeLauncherButton,
  shellState,
  taskbarTargets,
  toolbarFullscreenButton,
  toolbarHomeButton,
  toolbarInboxButton,
  toolbarSignOutButton,
  ensureHomeGuiDom,
  initializeRecentTargets,
  initializeShellLayout,
  shellInteractionActive,
  shouldIgnoreDesktopKeydown,
  targetById,
} from "./shell-core.js?v=home-20260725a";
import {
  clearIdentitySurface,
  syncIdentity,
  updateClock,
} from "./shell-chrome.js?v=home-20260725a";
import {
  clearDesktopSelection,
  continueTargetDrag,
  filterLauncherItems,
  finishTargetDrag,
  handleContextAction,
  hideDesktopContextMenu,
  hideLauncher,
  moveLauncherSelection,
  maybeShowWalletApprovalToast,
  openDesktopContextMenu,
  openSelectedDesktopEntry,
  openSelectedLauncherTarget,
  renderInboxBadge,
  refreshLauncherIfVisible,
  renderDesktop,
  renderLauncher,
  renderTaskbar,
  toggleLauncher,
  updateTaskbarState,
} from "./shell-surface.js?v=home-20260726a";
import {
  attachAuthorizedTarget,
  closeWindow,
  cleanupBeforeUnload,
  configureWindowHooks,
  handleShellResize,
  openTarget,
  focusWindow,
  restoreShellSession,
  showDesktopHome,
} from "./shell-windows.js?v=home-20260726a";

const OPAQUE_CAPSULE_ORIGIN = "null";
const OPAQUE_FRAME_TARGET = "*";

await ensureHomeGuiDom();

const HOME_GUI_HOST_SELECTORS = Object.freeze([
  ".desktop-backdrop",
  ".toolbar",
  ".desktop-workspace",
  ".taskbar",
  ".launcher",
  "#desktop-context-menu",
  "#home-notification-toast",
]);
let homeGuiInteractionsBound = false;

export const homeGuiWindowHooks = Object.freeze({
  hideLauncher,
  refreshLauncherIfVisible,
  clearIdentitySurface,
  updateClock,
  renderDesktop,
  renderTaskbar,
  updateTaskbarState,
});

const homeGuiHostActions = {
  launchTarget: null,
  requestHomeUnlock: null,
  requestSummaryRefresh: null,
};

configureWindowHooks({
  clearIdentitySurface,
  requestHomeUnlock: () => homeGuiHostActions.requestHomeUnlock?.(),
  hideLauncher,
  refreshLauncherIfVisible,
  renderDesktop,
  renderTaskbar,
  updateTaskbarState,
  launchTarget: (...args) => homeGuiHostActions.launchTarget?.(...args),
});

function homeGuiHostNodes() {
  return HOME_GUI_HOST_SELECTORS
    .map((selector) => document.querySelector(selector))
    .filter(Boolean);
}

function setHomeGuiHostNodeMounted(node, mounted) {
  if ("inert" in node) {
    node.inert = !mounted;
  }
  if (!mounted) {
    if (!node.dataset.homeGuiHiddenByHost) {
      node.dataset.homeGuiHiddenByHost = node.hidden ? "already-hidden" : "hidden";
    }
    node.hidden = true;
    node.setAttribute("aria-hidden", "true");
    return;
  }
  if (node.dataset.homeGuiHiddenByHost === "hidden") {
    node.hidden = false;
  }
  delete node.dataset.homeGuiHiddenByHost;
  if (node.hidden) {
    node.setAttribute("aria-hidden", "true");
    return;
  }
  node.removeAttribute("aria-hidden");
}

export function retireHomeGuiSurface(options = {}) {
  hideLauncher();
  hideDesktopContextMenu();
  clearDesktopSelection();
  if (options.closeWindows === true) {
    for (const id of [...shellState.windows.keys()]) {
      closeWindow(id);
    }
  }
  desktopShortcuts?.replaceChildren();
  taskbarTargets?.replaceChildren();
}

export function setHomeGuiMounted(mounted, options = {}) {
  document.body.dataset.homeGui = mounted ? "mounted" : "dormant";
  if (mounted) {
    shellState.homeGuiMounted = true;
    document.body.dataset.homeShell = "desktop";
    startHomeGuiClock();
    for (const node of homeGuiHostNodes()) {
      setHomeGuiHostNodeMounted(node, true);
    }
    return;
  }
  retireHomeGuiSurface({ closeWindows: options.closeWindows === true });
  shellState.homeGuiMounted = false;
  document.body.dataset.homeShell = "alternate";
  stopHomeGuiClock();
  for (const node of homeGuiHostNodes()) {
    setHomeGuiHostNodeMounted(node, false);
  }
}

function startHomeGuiClock() {
  updateClock();
  if (shellState.clockTimer) {
    return;
  }
  shellState.clockTimer = window.setInterval(updateClock, 30_000);
}

function stopHomeGuiClock() {
  if (!shellState.clockTimer) {
    return;
  }
  window.clearInterval(shellState.clockTimer);
  shellState.clockTimer = null;
}

export function showHomeGuiDesktop() {
  showDesktopHome();
}

export function restoreHomeGuiSession() {
  return restoreShellSession();
}

export function closeHomeGuiWindowsForTarget(targetId) {
  for (const entry of [...shellState.windows.values()]) {
    if (entry?.targetId === targetId) {
      closeWindow(entry.id);
    }
  }
}

export function homeGuiHasWindows() {
  return shellState.windows.size > 0;
}

export function homeGuiInteractionActive() {
  return shellInteractionActive();
}

export function closeHomeGuiWindow(windowId) {
  closeWindow(windowId);
}

export function relaunchHomeGuiTarget(windowId, target) {
  closeWindow(windowId);
  window.setTimeout(() => openTarget(target), 0);
}

export function deliverMessageToHomeGuiTargetFrame(target, payload, options = null) {
  const entries = [...shellState.windows.values()]
    .filter((entry) => entry.kind === "browser" && entry.targetId === target)
    .sort((left, right) => Number(right.serial || 0) - Number(left.serial || 0));
  const entry = entries.find((candidate) => !candidate.node.classList.contains("hidden")) || entries[0];
  const frame = entry?.node?.querySelector(".window-frame");
  if (!frame?.contentWindow) {
    return false;
  }
  frame.contentWindow.postMessage(payload, OPAQUE_FRAME_TARGET);
  if (options?.focus === true) {
    focusWindow(entry.id);
  }
  return true;
}

export function openHomeGuiTargetWithPayload(target, payload) {
  let deliveredCount = 0;
  if (deliverMessageToHomeGuiTargetFrame(target, payload, { focus: true })) {
    deliveredCount += 1;
  } else if (targetById(shellState.currentSummary, target)) {
    openTarget(target);
  } else {
    return false;
  }
  let attempts = 0;
  const retry = window.setInterval(() => {
    attempts += 1;
    if (deliverMessageToHomeGuiTargetFrame(target, payload, { focus: true })) {
      deliveredCount += 1;
    }
    if (deliveredCount >= 4 || attempts >= 40) {
      window.clearInterval(retry);
    }
  }, 150);
  return true;
}

export function broadcastHomeGuiRuntimeEvents(events) {
  const message = {
    type: "elastos:runtime-events",
    schema: "elastos.home.runtime-events/v1",
    events,
  };
  for (const entry of shellState.windows.values()) {
    const frame = entry?.node?.querySelector(".window-frame");
    try {
      frame?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
    } catch (error) {
      console.warn("could not deliver runtime event to app frame", error);
    }
  }
}

export function homeGuiMessageContextForSource(source, origin, homeToken) {
  if (!source || !origin || !homeToken) {
    return null;
  }
  for (const entry of shellState.windows.values()) {
    const frame = entry?.node?.querySelector(".window-frame");
    let frameWindow = null;
    try {
      frameWindow = frame?.contentWindow || null;
    } catch (_error) {
      continue;
    }
    if (frameWindow !== source) {
      continue;
    }
    if (origin !== OPAQUE_CAPSULE_ORIGIN) {
      return null;
    }
    const expectedToken = homeLaunchTokenFromRoute(
      frame?.dataset?.route || frame?.getAttribute("src") || "",
    );
    if (!expectedToken || expectedToken !== homeToken) {
      return null;
    }
    return {
      kind: "app-frame",
      targetId: entry.targetId || "",
      windowId: entry.id || "",
      homeToken,
    };
  }
  return null;
}

function homeLaunchTokenFromRoute(route) {
  try {
    const url = new URL(route, window.location.href);
    return new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") || "";
  } catch (_error) {
    return "";
  }
}

function homeGuiWindowEntryForToken(homeToken) {
  if (!homeToken) {
    return null;
  }
  for (const entry of shellState.windows.values()) {
    const frame = entry?.node?.querySelector(".window-frame");
    const token = homeLaunchTokenFromRoute(
      frame?.dataset?.route || frame?.getAttribute("src") || "",
    );
    if (token === homeToken) {
      return entry;
    }
  }
  return null;
}

export function closeHomeGuiWindowForToken(homeToken) {
  const entry = homeGuiWindowEntryForToken(homeToken);
  if (!entry) {
    return false;
  }
  closeWindow(entry.id);
  return true;
}

export function openHomeGuiTarget(target, options = {}) {
  return openTarget(target, options);
}

export function attachAuthorizedHomeGuiTarget(launched) {
  return attachAuthorizedTarget(launched);
}

export function relaunchHomeGuiWindowForToken(homeToken) {
  const entry = homeGuiWindowEntryForToken(homeToken);
  if (!entry) {
    return false;
  }
  const { id, targetId, launchQuery } = entry;
  closeWindow(id);
  window.setTimeout(() => openTarget(targetId, { query: launchQuery || {} }), 0);
  return true;
}

export function hideHomeGuiLauncher() {
  hideLauncher();
}

export function homeGuiLauncherIsOpen() {
  return !launcher.hidden;
}

export function syncHomeGuiProjection(previous, summary, options = {}) {
  shellState.currentSummary = summary;
  shellState.requestSummaryRefresh = homeGuiHostActions.requestSummaryRefresh;
  if (options.initialize === true || options.principalChanged === true) {
    initializeShellLayout(summary);
    initializeRecentTargets(summary);
  }
  if (options.activeShellMode !== "alternate") {
    syncHomeGuiChrome(previous, summary);
  }
  syncHomeGuiAppearance(summary);
  if (options.activeShellMode === "alternate") {
    hideLauncher();
    return;
  }
  if (
    options.initialize === true ||
    options.principalChanged === true ||
    options.activeShellChanged === true ||
    options.homeGuiWasMounted !== true ||
    targetsChanged(previous, summary) ||
    desktopObjectsChanged(previous, summary)
  ) {
    renderHomeGuiShell(summary, { desktop: true, launcherOpen: true });
    return;
  }
  renderHomeGuiShell(summary, { launcherOpen: homeGuiLauncherIsOpen() });
}

export function syncHomeGuiChrome(previous, summary) {
  updateClock();
  syncIdentity(summary);
  renderInboxBadge(summary);
  maybeShowWalletApprovalToast(previous, summary);
}

export function renderHomeGuiShell(summary, options = {}) {
  const renderDesktopSurface = options.desktop === true;
  const renderTaskbarSurface = options.taskbar !== false;
  const renderLauncherSurface = options.launcherOpen === true;
  if (renderDesktopSurface) {
    renderDesktop(summary);
  }
  if (renderTaskbarSurface) {
    renderTaskbar(summary);
  }
  if (renderLauncherSurface) {
    renderLauncher(summary);
  }
}

export function syncHomeGuiAppearance(summary) {
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

function desktopObjectsChanged(previous, next) {
  return desktopObjectsSignature(previous) !== desktopObjectsSignature(next);
}

function desktopObjectsSignature(summary) {
  const objects = summary &&
    summary.desktop_objects &&
    Array.isArray(summary.desktop_objects.objects)
    ? summary.desktop_objects.objects
    : [];
  return objects
    .map((object) => [
      object && object.uri,
      object && object.revision,
      object && object.kind,
      object && object.name,
    ].join(":"))
    .join("|");
}

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

function toggleHomeGuiFullscreen() {
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

function bindHomeGuiFullscreenControl() {
  if (!toolbarFullscreenButton) {
    return;
  }
  const { request, exit } = fullscreenApi();
  if (!request || !exit) {
    toolbarFullscreenButton.disabled = true;
    toolbarFullscreenButton.title = "Fullscreen is not available in this browser";
    return;
  }
  toolbarFullscreenButton.addEventListener("click", toggleHomeGuiFullscreen);
  document.addEventListener("fullscreenchange", syncFullscreenButton);
  document.addEventListener("webkitfullscreenchange", syncFullscreenButton);
  syncFullscreenButton();
}

export function bindHomeGuiInteractions(options = {}) {
  if (homeGuiInteractionsBound) {
    return;
  }
  homeGuiInteractionsBound = true;
  const activateHomeGui = typeof options.activateHomeGui === "function"
    ? options.activateHomeGui
    : () => Promise.resolve(showHomeGuiDesktop());
  const signOut = typeof options.signOut === "function"
    ? options.signOut
    : () => Promise.resolve();
  homeGuiHostActions.requestHomeUnlock = typeof options.requestHomeUnlock === "function"
    ? options.requestHomeUnlock
    : null;
  homeGuiHostActions.launchTarget = typeof options.launchTarget === "function"
    ? options.launchTarget
    : null;
  homeGuiHostActions.requestSummaryRefresh = typeof options.requestSummaryRefresh === "function"
    ? options.requestSummaryRefresh
    : null;
  shellState.requestSummaryRefresh = homeGuiHostActions.requestSummaryRefresh;

  toolbarHomeButton?.addEventListener("click", () => {
    activateHomeGui().catch((error) => {
      console.error("home-gui activation failed", error);
    });
  });

  toolbarInboxButton?.addEventListener("click", () => {
    if (!targetById(shellState.currentSummary, "inbox")) {
      return;
    }
    openTarget("inbox");
  });

  bindHomeGuiFullscreenControl();

  toolbarSignOutButton?.addEventListener("click", () => {
    document.body.dataset.homeStatus = "booting";
    signOut()
      .catch((error) => {
        console.error("home-gui sign out failed", error);
      })
      .finally(() => {
        window.location.reload();
      });
  });

  launcherToggleButton?.addEventListener("click", () => {
    toggleLauncher();
  });

  closeLauncherButton?.addEventListener("click", () => {
    hideLauncher();
  });

  launcherSearch?.addEventListener("input", () => {
    filterLauncherItems(launcherSearch.value);
  });

  launcherSearch?.addEventListener("keydown", (event) => {
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

  desktopShortcuts?.addEventListener("keydown", (event) => {
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
      openSelectedDesktopEntry();
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
      openSelectedDesktopEntry();
    }
  });

  desktopWorkspace?.addEventListener("contextmenu", (event) => {
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

  desktop?.addEventListener("pointerdown", (event) => {
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

  desktopContextMenu?.addEventListener("click", (event) => {
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
}
