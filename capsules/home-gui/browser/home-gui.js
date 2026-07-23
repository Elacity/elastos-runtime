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
  rememberSharedUiPreferences,
  shellInteractionActive,
  shouldIgnoreDesktopKeydown,
  targetById,
} from "./shell-core.js?v=home-20260722w";
import {
  bindIdentityMenu,
  clearIdentitySurface,
  syncIdentity,
  updateClock,
} from "./shell-chrome.js?v=home-20260722w";
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
  setDockAutoHide,
  toggleLauncher,
  updateDesktopMarquee,
  updateTaskbarState,
} from "./shell-surface.js?v=home-20260722w";
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
} from "./shell-windows.js?v=home-20260722w";
import {
  bindShellKeyboard,
  handleDesktopArrowKey,
  retireKeyboardSurfaces,
  toggleShortcutsOverlay,
} from "./shell-keyboard.js?v=home-20260722w";
import {
  bindSpotlight,
  hideSpotlight,
  showSpotlight,
} from "./shell-spotlight.js?v=home-20260722w";
import {
  bindNotificationCenter,
  hideNotificationCenter,
  recordNotifications,
} from "./shell-notifications.js?v=home-20260722w";
import {
  bindMenubar,
  closeMenus,
  setMenuManifest,
  syncMenubar,
} from "./shell-menubar.js?v=home-20260722w";
import {
  bindQuickLook,
  hideQuickLook,
  toggleQuickLook,
} from "./shell-quicklook.js?v=home-20260722w";
import { bindExpose, closeExpose, toggleExpose } from "./shell-expose.js?v=home-20260722w";
import {
  bindSpaceEdgePeek,
  bindSpacePager,
  toggleActiveFullscreenStage,
} from "./shell-stages.js?v=home-20260722w";
import { setUiSoundsEnabled } from "./shell-sounds.js?v=home-20260722w";
import { setFocusModeEnabled } from "./shell-core.js?v=home-20260722w";
import {
  bindControlCentre,
  hideControlCentre,
  syncControlCentre,
} from "./shell-control-centre.js?v=home-20260722w";
import {
  bindWalletRail,
  retireWalletRail,
  showWalletRail,
  syncWalletRailAvailability,
  walletRailFrame,
  walletRailOpen,
  walletRailSessionMounted,
} from "./shell-wallet-rail.js?v=home-20260722w";
import {
  bindInboxRail,
  retireInboxRail,
  toggleInboxRail,
  showInboxRail,
  inboxRailFrame,
  inboxRailOpen,
  inboxRailSessionMounted,
} from "./shell-inbox-rail.js?v=home-20260722w";
import {
  bindConnectorSheet,
  connectorSheetFrame,
  connectorSheetTarget,
  isConnectorSheetTarget,
  noteConnectorSheetSummaryRefresh,
  retireConnectorSheet,
  showConnectorSheet,
} from "./shell-connector-sheet.js?v=home-20260722w";

const OPAQUE_CAPSULE_ORIGIN = "null";
const OPAQUE_FRAME_TARGET = "*";

await ensureHomeGuiDom();
bindIdentityMenu();
bindShellSurfaceDom();
bindSpotlight();
bindNotificationCenter();
bindQuickLook();
bindExpose();
bindSpacePager();
bindSpaceEdgePeek();
bindShellKeyboard();
bindMenubar({ closeWindow, openTarget, supportsNewWindow: supportsMenuNewWindow });
bindControlCentre();
bindWalletRail();
bindInboxRail();
bindConnectorSheet();

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
  "#wallet-rail",
  "#inbox-rail",
  "#connector-sheet",
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
  // Host-mediated launches (Anders): GUI never fetch-launches; shell-windows
  // and the wallet rail call this hook after bindHomeGuiInteractions binds it.
  launchTarget: (...args) => homeGuiHostActions.launchTarget?.(...args),
  // One Wallet session: dock/desktop window launch retires the rail so we
  // do not keep two iframes / home_tokens for the same capsule — including
  // when the rail is hidden but the warm iframe is still mounted.
  retireWalletRailBeforeWindow: () => {
    if (walletRailSessionMounted() || walletRailOpen()) {
      retireWalletRail();
    }
  },
  retireInboxRailBeforeWindow: () => {
    if (inboxRailSessionMounted() || inboxRailOpen()) {
      retireInboxRail();
    }
  },
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
  hideNotificationCenter({ restoreFocus: false });
  hideControlCentre({ restoreFocus: false });
  retireWalletRail();
  retireInboxRail();
  retireConnectorSheet();
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

export function openHomeGuiTarget(target, options = {}) {
  const query = options.query && typeof options.query === "object" ? options.query : {};
  // Wallet connectors open as an in-rail ceremony sheet — not a second
  // desktop product window — when the wallet asks for sheet presentation
  // or the wallet rail is already open.
  if (
    isConnectorSheetTarget(target) &&
    (query.presentation === "sheet" || walletRailOpen())
  ) {
    showConnectorSheet(target, { ...options, query: { ...query, presentation: "sheet" } }).catch(
      (error) => {
        console.error("connector sheet open failed", error);
        openTarget(target, options);
      },
    );
    return;
  }
  openTarget(target, options);
}

export function noteHomeGuiConnectorSheetSummaryRefresh(homeToken) {
  noteConnectorSheetSummaryRefresh(homeToken);
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
  // Prefer an open rail over a desktop window — same capsule, user's
  // current surface. Focus shows the rail; window focus is the fallback.
  if (target === "wallet" && walletRailOpen()) {
    const railFrame = walletRailFrame();
    if (railFrame?.contentWindow) {
      railFrame.contentWindow.postMessage(payload, OPAQUE_FRAME_TARGET);
      if (options?.focus === true) {
        showWalletRail();
      }
      return true;
    }
  }
  if (target === "inbox" && inboxRailOpen()) {
    const railFrame = inboxRailFrame();
    if (railFrame?.contentWindow) {
      railFrame.contentWindow.postMessage(payload, OPAQUE_FRAME_TARGET);
      if (options?.focus === true) {
        showInboxRail();
      }
      return true;
    }
  }
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
  } else if (target === "wallet" && targetById(shellState.currentSummary, "wallet")) {
    showWalletRail();
  } else if (target === "inbox" && targetById(shellState.currentSummary, "inbox")) {
    showInboxRail();
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

/* Shell UI preferences arrive from the host (the canonical store — opaque
   frames have no localStorage). Apply to this document's theme/dock state,
   then fan out to every mounted app frame so their vendored theme runtimes
   follow. Cosmetic only; values were validated against closed sets by the
   host. */
export function applyHomeGuiUiPreferences(preferences) {
  const entries = preferences && typeof preferences === "object" ? preferences : {};
  rememberSharedUiPreferences(entries);
  if (typeof entries.theme === "string" && window.elastosTheme) {
    window.elastosTheme.set(entries.theme);
  }
  if (typeof entries.accent === "string" && window.elastosTheme?.setAccent) {
    window.elastosTheme.setAccent(entries.accent);
  }
  if (typeof entries.dockAutoHide === "string") {
    setDockAutoHide(entries.dockAutoHide === "on");
  }
  if (typeof entries.sounds === "string") {
    setUiSoundsEnabled(entries.sounds === "on");
  }
  if (typeof entries.focusMode === "string") {
    setFocusModeEnabled(entries.focusMode === "on");
  }
  broadcastHomeGuiUiPreferences(entries);
}

function broadcastHomeGuiUiPreferences(preferences) {
  const message = { type: "elastos:ui-preference", preferences };
  for (const entry of shellState.windows.values()) {
    const frame = entry?.node?.querySelector(".window-frame");
    try {
      frame?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
    } catch (_error) {
      // Frame mid-teardown; the boot push covers the next mount.
    }
  }
  try {
    walletRailFrame()?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
  } catch (_error) {
    // Rail not mounted.
  }
  try {
    inboxRailFrame()?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
  } catch (_error) {
    // Rail not mounted.
  }
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
  const railFrame = walletRailFrame();
  try {
    railFrame?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
  } catch (error) {
    console.warn("could not deliver runtime event to wallet rail", error);
  }
  const inboxFrame = inboxRailFrame();
  try {
    inboxFrame?.contentWindow?.postMessage(message, OPAQUE_FRAME_TARGET);
  } catch (error) {
    console.warn("could not deliver runtime event to inbox rail", error);
  }
}

export function homeGuiMessageContextForSource(source, origin, homeToken) {
  if (!source || !origin || !homeToken) {
    return null;
  }
  const railFrame = walletRailFrame();
  let railWindow = null;
  try {
    railWindow = railFrame?.contentWindow || null;
  } catch (_error) {
    railWindow = null;
  }
  if (railWindow && railWindow === source) {
    if (origin !== OPAQUE_CAPSULE_ORIGIN) {
      return null;
    }
    const expectedToken = homeLaunchTokenFromRoute(
      railFrame?.dataset?.route || railFrame?.getAttribute("src") || "",
    );
    if (expectedToken && expectedToken === homeToken) {
      return {
        kind: "app-frame",
        targetId: "wallet",
        windowId: "wallet-rail",
        homeToken,
      };
    }
    return null;
  }
  const inboxFrame = inboxRailFrame();
  let inboxWindow = null;
  try {
    inboxWindow = inboxFrame?.contentWindow || null;
  } catch (_error) {
    inboxWindow = null;
  }
  if (inboxWindow && inboxWindow === source) {
    if (origin !== OPAQUE_CAPSULE_ORIGIN) {
      return null;
    }
    const expectedToken = homeLaunchTokenFromRoute(
      inboxFrame?.dataset?.route || inboxFrame?.getAttribute("src") || "",
    );
    if (expectedToken && expectedToken === homeToken) {
      return {
        kind: "app-frame",
        targetId: "inbox",
        windowId: "inbox-rail",
        homeToken,
      };
    }
    return null;
  }
  const sheetFrame = connectorSheetFrame();
  let sheetWindow = null;
  try {
    sheetWindow = sheetFrame?.contentWindow || null;
  } catch (_error) {
    sheetWindow = null;
  }
  if (sheetWindow && sheetWindow === source) {
    const expectedToken = homeLaunchTokenFromRoute(
      sheetFrame?.dataset?.route || sheetFrame?.getAttribute("src") || "",
    );
    if (expectedToken && expectedToken === homeToken) {
      return {
        kind: "app-frame",
        targetId: connectorSheetTarget() || "wallet-metamask",
        windowId: "connector-sheet",
        homeToken,
      };
    }
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
  syncWalletRailAvailability(summary);
  maybeShowWalletApprovalToast(previous, summary);
  recordNotifications(summary);
  syncControlCentre(summary);
}

/* Menus are self-declared UI, not authority: the host verifies the sender's
   frame identity and hands us only that window's id. */
export function setHomeGuiMenuManifest(windowId, menus, homeToken = "") {
  // The host forwards the sender's launch token; only this GUI knows which
  // of its windows carries that token. Rail/sheet frames have no menubar
  // entry, so an unresolved token is dropped, not guessed.
  let resolvedId = windowId;
  if (!resolvedId && homeToken) {
    resolvedId = homeGuiWindowEntryForToken(homeToken)?.id || "";
  }
  if (!resolvedId) {
    return;
  }
  setMenuManifest(resolvedId, menus);
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

/* About ElastOS: window-chrome close, verified runtime version, and an
   honest update path (System About owns check/install — no fake badges). */
let aboutOverlayBound = false;

function aboutOverlayNode() {
  return document.querySelector("#about-overlay");
}

function aboutRuntimeVersion(summary) {
  const version = summary?.runtime?.version;
  return typeof version === "string" && version.trim() ? version.trim() : "";
}

/* Prefer an explicit summary flag when present; never invent “up to date”. */
function aboutUpdateSignal(summary) {
  const runtime = summary?.runtime;
  if (!runtime || typeof runtime !== "object") {
    return null;
  }
  if (runtime.update_available === true) {
    return { state: "available", label: "Update available" };
  }
  if (runtime.update_available === false) {
    return { state: "current", label: "Up to date" };
  }
  const note = typeof runtime.update_status === "string" ? runtime.update_status.trim() : "";
  if (note) {
    return { state: "info", label: note };
  }
  return null;
}

function syncAboutOverlayFacts(overlay) {
  const summary = shellState.currentSummary;
  const versionLine = overlay.querySelector("#about-version");
  if (versionLine) {
    const version = aboutRuntimeVersion(summary);
    versionLine.textContent = version ? `Version ${version}` : "";
    versionLine.hidden = !version;
  }
  const identityLine = overlay.querySelector("#about-identity");
  if (identityLine) {
    const name = document.querySelector("#toolbar-identity-menu-name")?.textContent?.trim();
    identityLine.textContent = name ? `Signed in as ${name}` : "";
    identityLine.hidden = !name;
  }
  const updateLine = overlay.querySelector("#about-update");
  if (updateLine) {
    const signal = aboutUpdateSignal(summary);
    if (signal) {
      updateLine.textContent = signal.label;
      updateLine.dataset.state = signal.state;
      updateLine.hidden = false;
    } else {
      updateLine.textContent = "";
      delete updateLine.dataset.state;
      updateLine.hidden = true;
    }
  }
}

function openSystemAboutFromOverlay() {
  hideAboutOverlay({ restoreFocus: false });
  if (!targetById(shellState.currentSummary, "system")) {
    return;
  }
  openTarget("system", { query: { settings: "about" } });
}

function showAboutOverlay() {
  const overlay = aboutOverlayNode();
  if (!overlay) {
    return;
  }
  syncAboutOverlayFacts(overlay);
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
  overlay.querySelector("#about-more")?.addEventListener("click", () => openSystemAboutFromOverlay());
  overlay.querySelector("#about-software-update")?.addEventListener("click", () => {
    openSystemAboutFromOverlay();
  });
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
      const focusables = [
        overlay.querySelector("#about-close"),
        overlay.querySelector("#about-more"),
        overlay.querySelector("#about-software-update"),
      ].filter((node) => node && !node.disabled && !node.hidden);
      if (focusables.length < 2) {
        event.preventDefault();
        focusables[0]?.focus();
        return;
      }
      event.preventDefault();
      const index = focusables.indexOf(document.activeElement);
      const next = event.shiftKey
        ? (index <= 0 ? focusables.length - 1 : index - 1)
        : (index >= focusables.length - 1 ? 0 : index + 1);
      focusables[next].focus();
    }
  });
}

function syncFullscreenButton() {
  if (!toolbarFullscreenButton) {
    return;
  }
  // Window fullscreen stage (dedicated Space) — not the browser Fullscreen API.
  const id = shellState.activeWindowId;
  const entry = id ? shellState.windows.get(id) : null;
  const active = entry?.fullscreenStage === true;
  const label = toolbarFullscreenButton.querySelector(".control-centre-row-label");
  (label || toolbarFullscreenButton).textContent = active
    ? "Exit Fullscreen"
    : "Enter Fullscreen";
}

function toggleHomeGuiFullscreen() {
  hideControlCentre({ restoreFocus: false });
  toggleActiveFullscreenStage();
  syncFullscreenButton();
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
  toolbarFullscreenButton.hidden = false;
  toolbarFullscreenButton.addEventListener("click", toggleHomeGuiFullscreen);
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
    toggleInboxRail();
  });

  bindHomeGuiFullscreenControl();
  bindLauncherViewToggle();

  identityMenuSystemButton?.addEventListener("click", () => {
    if (!targetById(shellState.currentSummary, "system")) {
      return;
    }
    openTarget("system");
  });

  document.querySelector("#identity-menu-marketplace")?.addEventListener("click", () => {
    if (!targetById(shellState.currentSummary, "marketplace")) {
      return;
    }
    openTarget("marketplace");
  });

  document.querySelector("#identity-menu-shortcuts")?.addEventListener("click", () => {
    toggleShortcutsOverlay();
  });

  document.querySelector("#identity-menu-lock")?.addEventListener("click", () => {
    window.dispatchEvent(new CustomEvent("elastos:request-lock"));
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

  document.querySelector("#toolbar-mission-control")?.addEventListener("click", () => {
    toggleExpose();
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
