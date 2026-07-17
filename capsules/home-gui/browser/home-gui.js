import {
  desktop,
  desktopBackdrop,
  desktopShortcuts,
  desktopContextMenu,
  desktopWorkspace,
  launcher,
  launcherSearch,
  launcherToggleButton,
  launcherViewToggle,
  closeLauncherButton,
  identityMenuSystemButton,
  shellState,
  taskbarTargets,
  toolbarFullscreenButton,
  toolbarHomeButton,
  identityMenuShowDesktopButton,
  toolbarInboxButton,
  toolbarSignOutButton,
  ensureHomeGuiDom,
  initializeRecentTargets,
  initializeShellLayout,
  shellInteractionActive,
  shouldIgnoreDesktopKeydown,
  targetById,
} from "./shell-core.js?v=home-20260718n";
import {
  bindIdentityMenu,
  clearIdentitySurface,
  syncIdentity,
  updateClock,
} from "./shell-chrome.js?v=home-20260718n";
import {
  beginDesktopMarquee,
  bindShellSurfaceDom,
  clearDesktopSelection,
  continueTargetDrag,
  filterLauncherItems,
  finishDesktopMarquee,
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
  selectAllDesktopIcons,
  toggleLauncher,
  updateDesktopMarquee,
  updateTaskbarState,
} from "./shell-surface.js?v=home-20260718n";
import {
  closeWindow,
  cleanupBeforeUnload,
  configureWindowHooks,
  handleShellResize,
  openTarget,
  focusWindow,
  restoreShellSession,
  showDesktopHome,
  supportsMenuNewWindow,
} from "./shell-windows.js?v=home-20260718n";
import {
  bindShellKeyboard,
  handleDesktopArrowKey,
  retireKeyboardSurfaces,
  toggleShortcutsOverlay,
} from "./shell-keyboard.js?v=home-20260718n";
import {
  bindSpotlight,
  hideSpotlight,
  showSpotlight,
} from "./shell-spotlight.js?v=home-20260718n";
import {
  bindNotificationCenter,
  hideNotificationCenter,
  recordNotifications,
} from "./shell-notifications.js?v=home-20260718n";
import {
  bindMenubar,
  closeMenus,
  setMenuManifest,
  syncMenubar,
} from "./shell-menubar.js?v=home-20260718n";
import {
  bindQuickLook,
  hideQuickLook,
  toggleQuickLook,
} from "./shell-quicklook.js?v=home-20260718n";
import { bindExpose, closeExpose } from "./shell-expose.js?v=home-20260718n";
import {
  bindControlCentre,
  hideControlCentre,
} from "./shell-control-centre.js?v=home-20260718n";

const OPAQUE_CAPSULE_ORIGIN = "null";
const OPAQUE_FRAME_TARGET = "*";

await ensureHomeGuiDom();
bindIdentityMenu();
bindShellSurfaceDom();
bindSpotlight();
bindNotificationCenter();
bindQuickLook();
bindExpose();
bindShellKeyboard();
bindMenubar({ closeWindow, openTarget, supportsNewWindow: supportsMenuNewWindow });
bindControlCentre();

const HOME_GUI_HOST_SELECTORS = Object.freeze([
  ".desktop-backdrop",
  ".toolbar",
  ".desktop-workspace",
  ".taskbar",
  ".launcher",
  "#desktop-context-menu",
  "#home-notification-toast",
  "#notification-center",
  "#control-centre",
  "#spotlight",
  "#window-switcher",
  "#quick-look",
  "#shortcuts-overlay",
  "#about-overlay",
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
  syncMenubar,
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
  hideSpotlight({ restoreFocus: false });
  hideNotificationCenter();
  hideControlCentre({ restoreFocus: false });
  hideAboutOverlay({ restoreFocus: false });
  hideQuickLook();
  closeExpose();
  retireKeyboardSurfaces();
  closeMenus({ restoreFocus: false });
  if (options.closeWindows === true) {
    for (const id of [...shellState.windows.keys()]) {
      closeWindow(id);
    }
  }
  desktopShortcuts?.replaceChildren();
  taskbarTargets?.replaceChildren();
}

export function setHomeGuiMounted(mounted, options = {}) {
  const wasMounted = document.body.dataset.homeGui === "mounted";
  document.body.dataset.homeGui = mounted ? "mounted" : "dormant";
  if (mounted) {
    shellState.homeGuiMounted = true;
    document.body.dataset.homeShell = "desktop";
    startHomeGuiClock();
    for (const node of homeGuiHostNodes()) {
      setHomeGuiHostNodeMounted(node, true);
    }
    if (!wasMounted) {
      playHomeGuiArrival();
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

// Arrival: the desktop settles in as the GUI mounts (unlock hand-off or a
// switch back from an alternate shell). Opacity/transform only — compositor
// work, no layout. The host's neutral mask never carries the desktop; this
// beat is the GUI's own. Reduced motion skips it.
function playHomeGuiArrival() {
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true) {
    return;
  }
  const arriving = [".desktop-backdrop", ".toolbar", ".desktop-workspace", ".taskbar"]
    .map((selector) => document.querySelector(selector))
    .filter((node) => node && !node.hidden);
  for (const node of arriving) {
    node.classList.add("home-gui-arriving");
  }
  window.setTimeout(() => {
    for (const node of arriving) {
      node.classList.remove("home-gui-arriving");
    }
  }, 760);
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
  recordNotifications(summary);
}

/* Menus are self-declared UI, not authority: the host verifies the sender's
   frame identity and hands us only that window's id. */
export function setHomeGuiMenuManifest(windowId, menus) {
  setMenuManifest(windowId, menus);
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

/* About ElastOS: the smallest honest dialog — logo, product name, which
   shell this is, who is signed in. No version claims the shell cannot
   verify from runtime facts. */
let aboutOverlayBound = false;

function aboutOverlayNode() {
  return document.querySelector("#about-overlay");
}

function showAboutOverlay() {
  const overlay = aboutOverlayNode();
  if (!overlay) {
    return;
  }
  const identityLine = overlay.querySelector("#about-identity");
  if (identityLine) {
    const name = document.querySelector("#toolbar-identity-menu-name")?.textContent?.trim();
    identityLine.textContent = name ? `Signed in as ${name}` : "";
    identityLine.hidden = !name;
  }
  overlay.hidden = false;
  overlay.inert = false;
  overlay.setAttribute("aria-hidden", "false");
  overlay.querySelector("#about-close")?.focus();
}

function hideAboutOverlay({ restoreFocus = true } = {}) {
  const overlay = aboutOverlayNode();
  if (!overlay || overlay.hidden) {
    return;
  }
  overlay.hidden = true;
  overlay.inert = true;
  overlay.setAttribute("aria-hidden", "true");
  if (restoreFocus) {
    toolbarHomeButton?.focus();
  }
}

function bindAboutOverlay() {
  if (aboutOverlayBound) {
    return;
  }
  aboutOverlayBound = true;
  document.querySelector("#identity-menu-about")?.addEventListener("click", () => {
    showAboutOverlay();
  });
  const overlay = aboutOverlayNode();
  if (!overlay) {
    return;
  }
  overlay.querySelector("#about-close")?.addEventListener("click", () => hideAboutOverlay());
  overlay.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".about-card")) {
      hideAboutOverlay({ restoreFocus: false });
    }
  });
  overlay.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideAboutOverlay();
      return;
    }
    if (event.key === "Tab") {
      // Single focusable control; keep focus inside the modal.
      event.preventDefault();
    }
  });
}

function syncFullscreenButton() {
  if (!toolbarFullscreenButton) {
    return;
  }
  // The fullscreen control is a Control Centre row; its visible label is its
  // accessible name.
  const active = Boolean(fullscreenElement());
  const label = toolbarFullscreenButton.querySelector(".control-centre-row-label");
  (label || toolbarFullscreenButton).textContent = active ? "Exit fullscreen" : "Enter fullscreen";
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
    toolbarFullscreenButton.hidden = true;
    return;
  }
  toolbarFullscreenButton.addEventListener("click", toggleHomeGuiFullscreen);
  document.addEventListener("fullscreenchange", syncFullscreenButton);
  document.addEventListener("webkitfullscreenchange", syncFullscreenButton);
  syncFullscreenButton();
}

/* Grid/list view for the launcher (macOS Apps panel view control). The
   preference is a pure browser concern, so localStorage — same store the
   theme runtime uses. */
const LAUNCHER_VIEW_KEY = "elastos.ui.launcherView";

function applyLauncherView(view) {
  const list = view === "list";
  launcher.dataset.view = list ? "list" : "grid";
  launcherViewToggle?.setAttribute("aria-pressed", list ? "true" : "false");
  launcherViewToggle?.setAttribute(
    "aria-label",
    list ? "Switch to grid view" : "Switch to list view",
  );
}

function bindLauncherViewToggle() {
  try {
    applyLauncherView(localStorage.getItem(LAUNCHER_VIEW_KEY));
  } catch (_error) {
    applyLauncherView("grid");
  }
  launcherViewToggle?.addEventListener("click", () => {
    const next = launcher.dataset.view === "list" ? "grid" : "list";
    applyLauncherView(next);
    try {
      localStorage.setItem(LAUNCHER_VIEW_KEY, next);
    } catch (_error) {
      // Preference still applies for this session.
    }
  });
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

  // The brand button itself toggles the ElastOS menu (bound in
  // bindIdentityMenu); the go-home action lives inside it as Show desktop.
  identityMenuShowDesktopButton?.addEventListener("click", () => {
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
  bindLauncherViewToggle();

  identityMenuSystemButton?.addEventListener("click", () => {
    if (!targetById(shellState.currentSummary, "system")) {
      return;
    }
    openTarget("system");
  });

  document.querySelector("#identity-menu-shortcuts")?.addEventListener("click", () => {
    toggleShortcutsOverlay();
  });

  bindAboutOverlay();

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

  document.querySelector("#toolbar-spotlight")?.addEventListener("click", () => {
    showSpotlight();
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
    // Space = Quick Look (macOS); Enter opens. Bare Space must not also open.
    if (event.code === "Space" && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      event.stopPropagation();
      toggleQuickLook();
      return;
    }
    if (event.key === "Enter" && shellState.selectedDesktopTargetId) {
      event.preventDefault();
      event.stopPropagation();
      openSelectedDesktopEntry();
      return;
    }
    if ((event.key === "a" || event.key === "A") && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      selectAllDesktopIcons();
      return;
    }
    if (event.key.startsWith("Arrow") && !event.metaKey && !event.ctrlKey && !event.altKey) {
      handleDesktopArrowKey(event);
    }
  });

  document.addEventListener("pointermove", (event) => {
    trackPointerMove(event);
    continueTargetDrag(event);
    updateDesktopMarquee(event);
  });

  document.addEventListener("pointerup", (event) => {
    finishTargetDrag(event);
    finishDesktopMarquee(event);
  });

  document.addEventListener("pointercancel", (event) => {
    finishTargetDrag(event);
    finishDesktopMarquee(event);
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
      hideDesktopContextMenu({ restoreFocus: true });
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
    beginDesktopMarquee(event);
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
