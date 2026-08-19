import {
  desktop,
  HOME_GUI_SHELL_ID,
  windowTemplate,
  windowErrorTemplate,
  SYSTEM_APP_ID,
  shellState,
  targetTitle,
  canonicalTargetTitle,
  escapeHtml,
  applyWindowChrome,
  shouldOpenMaximizedByDefault,
  mountGlyph,
  glyphTone,
  rememberRecentTarget,
  clampDesktopLayoutToViewport,
  saveShellLayoutState,
  loadShellSessionState,
  saveShellSessionState,
  ignoreRepeatedAction,
  targetById,
} from "./shell-core.js?v=home-20260814a";
import {
  fitWindowBounds,
  fitWindowToBrowserAspect,
  fitWindowToLargestBrowserAspect,
  applyWindowPlacement,
  rememberWindowRestoreBounds,
  restoreWindowFromSpecialState,
  hideWindowSnapPreview,
  attachWindowDrag,
  attachWindowResize,
} from "./shell-window-geometry.js?v=home-20260814a";
import {
  applyFullscreenStageFromPlacement,
  bindStageWindowHooks,
  desktopStageId,
  ensureDesktopForNewLaunch,
  exitFullscreenStage,
  forgetClosedFullscreenSpace,
  getActiveStageId,
  isDesktopSpace,
  neighborSpaceAfterClosing,
  playCloseFullscreenSpaceMotion,
  restoreExtraDesktops,
  restoreSpaceOrder,
  setActiveStage,
  toggleFullscreenStage,
  windowVisibleOnActiveSpace,
} from "./shell-stages.js?v=home-20260814a";

let windowHooks = null;
const REQUIRED_WINDOW_HOOKS = [
  "clearIdentitySurface",
  "hideLauncher",
  "refreshLauncherIfVisible",
  "renderDesktop",
  "renderTaskbar",
  "updateTaskbarState",
  "launchTarget",
  "syncMenubar",
];
const WINDOW_CONTROL_GUARD_MS = 400;
const WINDOW_MAXIMIZE_CLOSE_GUARD_MS = 360;
const WINDOW_OPEN_CLOSE_GHOST_GUARD_MS = 2600;
const WINDOW_CLOSE_GUARD_MOVE_PX = 18;
const BROWSER_DESKTOP_OPEN_GUARD_MS = 700;
// Four 8s Runtime close attempts plus bounded 1.2s, 3s, and 7s retries.
const BROWSER_WINDOW_CLOSE_TIMEOUT_MS = 50_000;
const BROWSER_WINDOW_CLOSE_REQUEST_TYPE =
  "elastos.browser.window-close.request/v1";
const BROWSER_WINDOW_CLOSE_RESULT_TYPE =
  "elastos.browser.window-close.result/v1";
const OPAQUE_CAPSULE_ORIGIN = "null";
const MAX_SESSION_WINDOWS = 24;
const SINGLE_SESSION_TARGETS = new Set(["people", "inbox", "wallet"]);

/** Single-session apps refocus rather than open a second window, so the menu
 *  bar does not offer them a New Window that would not do that. */
export function supportsMenuNewWindow(targetId) {
  return !SINGLE_SESSION_TARGETS.has(targetId);
}
const WALLET_CONNECTOR_TARGETS = new Set([
  "wallet-metamask",
  "wallet-unisat",
]);
const NON_RESTORABLE_SESSION_TARGETS = new Set(WALLET_CONNECTOR_TARGETS);
const WALLET_CONNECTOR_WINDOW_WIDTH = 480;
const WALLET_CONNECTOR_WINDOW_HEIGHT = 560;
const WALLET_CONNECTOR_WINDOW_EDGE_INSET = 24;
const COMMON_IFRAME_SANDBOX = [
  "allow-downloads",
  "allow-forms",
  "allow-modals",
  "allow-pointer-lock",
  "allow-scripts",
];
const BROWSER_IFRAME_SANDBOX_EXTRAS = [
  "allow-popups",
  "allow-popups-to-escape-sandbox",
];
const WALLET_CONNECTOR_IFRAME_SANDBOX_EXTRAS = [
  "allow-popups",
  "allow-popups-to-escape-sandbox",
];
const SYSTEM_IFRAME_SANDBOX_EXTRAS = [
  "allow-top-navigation-by-user-activation",
  "allow-top-navigation-to-custom-protocols",
];
const COMMON_IFRAME_ALLOW = ["autoplay", "fullscreen"];
const pendingWindowLaunches = new Set();
const pendingBrowserWindowCloses = new Map();

window.addEventListener("message", handleBrowserWindowCloseResult);

export function iframeSandboxForLaunch(launched) {
  const tokens = [...COMMON_IFRAME_SANDBOX];
  if (launched?.target === "browser") {
    tokens.push(...BROWSER_IFRAME_SANDBOX_EXTRAS);
  }
  if (launched?.target === "wallet-unisat") {
    tokens.push(...WALLET_CONNECTOR_IFRAME_SANDBOX_EXTRAS);
  }
  if (launched?.target === SYSTEM_APP_ID) {
    tokens.push(...SYSTEM_IFRAME_SANDBOX_EXTRAS);
  }
  return tokens.join(" ");
}

export function iframeAllowForLaunch() {
  return COMMON_IFRAME_ALLOW.join("; ");
}

export async function launchHomeTarget(targetId, query = {}) {
  const launched = await requireWindowHooks().launchTarget(targetId, query);
  if (!launched || typeof launched !== "object") {
    throw new Error("Home launch returned no result");
  }
  return launched;
}

export function configureWindowHooks(nextHooks) {
  if (windowHooks) {
    throw new Error("Home window hooks are already configured");
  }
  for (const name of REQUIRED_WINDOW_HOOKS) {
    if (!nextHooks || typeof nextHooks[name] !== "function") {
      throw new Error(`Home window hooks missing required function: ${name}`);
    }
  }
  windowHooks = Object.freeze({ ...nextHooks });
}

function requireWindowHooks() {
  if (!windowHooks) {
    throw new Error("Home window hooks are not configured");
  }
  return windowHooks;
}

function refreshWindowUi() {
  const hooks = requireWindowHooks();
  hooks.updateTaskbarState();
  hooks.refreshLauncherIfVisible();
  // The bar names the focused app, so it follows focus, open and close.
  hooks.syncMenubar();
}

function currentWindowBounds(node) {
  return {
    x: Number.parseFloat(node.style.left) || node.offsetLeft || 48,
    y: Number.parseFloat(node.style.top) || node.offsetTop || 60,
    width: Number.parseFloat(node.style.width) || node.offsetWidth || 560,
    height: Number.parseFloat(node.style.height) || node.offsetHeight || 404,
  };
}

function currentWindowRestoreBounds(node) {
  return {
    x: Number.parseFloat(node.dataset.restoreLeft) || Number.parseFloat(node.style.left) || node.offsetLeft || 48,
    y: Number.parseFloat(node.dataset.restoreTop) || Number.parseFloat(node.style.top) || node.offsetTop || 60,
    width: Number.parseFloat(node.dataset.restoreWidth) || Number.parseFloat(node.style.width) || node.offsetWidth || 560,
    height: Number.parseFloat(node.dataset.restoreHeight) || Number.parseFloat(node.style.height) || node.offsetHeight || 404,
  };
}

function persistedBrowserSessionEntries() {
  return sortWindowEntriesByZOrder(
    browserWindowEntries().filter(
      (entry) => !NON_RESTORABLE_SESSION_TARGETS.has(entry.targetId),
    ),
  )
    .reverse()
    .slice(0, MAX_SESSION_WINDOWS)
    .map((entry) => {
      const bounds = currentWindowBounds(entry.node);
      const restoreBounds = currentWindowRestoreBounds(entry.node);
      return {
        target: entry.targetId,
        hidden: entry.node.classList.contains("hidden"),
        active: shellState.activeWindowId === entry.id,
        maximized:
          entry.node.dataset.maximized === "true" ||
          entry.node.dataset.browserMaximized === "true",
        snap: entry.node.dataset.snap || "",
        fullscreenStage: entry.fullscreenStage === true,
        desktopSpaceId: entry.desktopSpaceId || desktopStageId(),
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        restoreX: restoreBounds.x,
        restoreY: restoreBounds.y,
        restoreWidth: restoreBounds.width,
        restoreHeight: restoreBounds.height,
        query: normalizedLaunchQuery(entry.launchQuery),
      };
    });
}

function currentRootShellSessionId() {
  return HOME_GUI_SHELL_ID;
}

let agentWorkspaceSnapshotFn = null;
let agentWorkspacePersistTimer = 0;

/** Agent harness registers a snapshotter so chat/projects ride the host session. */
export function bindAgentWorkspaceSnapshot(getSnapshot) {
  agentWorkspaceSnapshotFn = typeof getSnapshot === "function" ? getSnapshot : null;
}

function agentWorkspaceForPersist() {
  try {
    const snap = agentWorkspaceSnapshotFn?.();
    return snap && typeof snap === "object" ? snap : null;
  } catch {
    return null;
  }
}

/** Debounced persist after pin / project / chat edits (host session blob). */
export function scheduleAgentWorkspacePersist() {
  if (shellState.restoringSession) {
    return;
  }
  window.clearTimeout(agentWorkspacePersistTimer);
  agentWorkspacePersistTimer = window.setTimeout(() => {
    agentWorkspacePersistTimer = 0;
    persistBrowserSession();
  }, 200);
}

function persistBrowserSession() {
  if (shellState.restoringSession) {
    return;
  }
  saveShellSessionState(snapshotBrowserSession());
}

export function snapshotBrowserSession() {
  const agent = agentWorkspaceForPersist();
  return {
    root_shell: currentRootShellSessionId(),
    windows: persistedBrowserSessionEntries(),
    desktops: [...(Array.isArray(shellState.extraDesktops) ? shellState.extraDesktops : [])],
    active_stage: stableSpaceKeyForId(getActiveStageId()),
    space_order: (Array.isArray(shellState.spaceOrder) ? shellState.spaceOrder : [])
      .map((id) => stableSpaceKeyForId(id)),
    ...(agent ? { agent } : {}),
  };
}

/* Fullscreen Spaces are keyed by window id, which does not survive a reload.
   Persist them as fs:<target>:<instance> and remap to live ids on restore. */
function stableSpaceKeyForId(spaceId) {
  if (!spaceId || isDesktopSpace(spaceId)) {
    return spaceId || desktopStageId();
  }
  const entry = shellState.windows.get(spaceId);
  if (!entry?.fullscreenStage) {
    return spaceId;
  }
  const inst =
    typeof entry.launchQuery?.browser_instance === "string"
      ? entry.launchQuery.browser_instance
      : "";
  return `fs:${entry.targetId}:${inst}`;
}

function resolveStableSpaceKey(key, restoredEntries) {
  if (typeof key !== "string" || !key) {
    return desktopStageId();
  }
  if (!key.startsWith("fs:")) {
    return key;
  }
  const [, targetId = "", inst = ""] = key.split(":");
  for (const { entry } of restoredEntries) {
    if (!entry?.fullscreenStage || entry.targetId !== targetId) {
      continue;
    }
    const entryInst =
      typeof entry.launchQuery?.browser_instance === "string"
        ? entry.launchQuery.browser_instance
        : "";
    if (entryInst === inst) {
      return entry.id;
    }
  }
  const fallback = restoredEntries.find(
    ({ entry }) => entry?.fullscreenStage && entry.targetId === targetId,
  );
  return fallback ? fallback.entry.id : desktopStageId();
}

function storedSessionRootShell(storedSession) {
  const rootShell = typeof storedSession?.root_shell === "string"
    ? storedSession.root_shell.trim()
    : "";
  return rootShell;
}

export function normalizeRestorableSession(summary, storedSession, options = {}) {
  const requestedRootShell = typeof options.rootShell === "string"
    ? options.rootShell.trim()
    : "";
  if (requestedRootShell && storedSessionRootShell(storedSession) !== requestedRootShell) {
    return [];
  }
  const storedWindows = Array.isArray(storedSession?.windows) ? storedSession.windows : [];
  const seenTargets = new Set();
  const normalized = [];
  for (const item of storedWindows) {
    const targetId = typeof item?.target === "string" ? item.target : "";
    if (
      !targetId ||
      (SINGLE_SESSION_TARGETS.has(targetId) && seenTargets.has(targetId)) ||
      !targetById(summary, targetId)
    ) {
      continue;
    }
    if (SINGLE_SESSION_TARGETS.has(targetId)) {
      seenTargets.add(targetId);
    }
    normalized.push({
      target: targetId,
      hidden: item?.hidden === true,
      active: item?.active === true,
      maximized: item?.maximized === true,
      snap: typeof item?.snap === "string" ? item.snap : "",
      fullscreenStage: item?.fullscreenStage === true,
      desktopSpaceId:
        typeof item?.desktopSpaceId === "string" && item.desktopSpaceId.startsWith("desk-")
          ? item.desktopSpaceId
          : desktopStageId(),
      query: restorableLaunchQuery(targetId, item),
      x: Number.isFinite(item?.x) ? item.x : 48,
      y: Number.isFinite(item?.y) ? item.y : 60,
      width: Number.isFinite(item?.width) ? item.width : 560,
      height: Number.isFinite(item?.height) ? item.height : 404,
      restoreX: Number.isFinite(item?.restoreX) ? item.restoreX : undefined,
      restoreY: Number.isFinite(item?.restoreY) ? item.restoreY : undefined,
      restoreWidth: Number.isFinite(item?.restoreWidth) ? item.restoreWidth : undefined,
      restoreHeight: Number.isFinite(item?.restoreHeight) ? item.restoreHeight : undefined,
    });
    if (normalized.length >= MAX_SESSION_WINDOWS) {
      break;
    }
  }
  return normalized;
}

function restorableLaunchQuery(targetId, item) {
  const query = normalizedLaunchQuery(item?.query);
  if (targetId === "browser" && !query.browser_instance) {
    query.browser_instance = nextBrowserInstanceId();
  }
  return query;
}

function renderWindowTaskbar() {
  if (!shellState.currentSummary) {
    return;
  }
  requireWindowHooks().renderTaskbar(shellState.currentSummary);
}

function windowHostContainer() {
  return desktop;
}

function rerenderShellSurfaces({ desktop: rerenderDesktop = false, taskbar: rerenderTaskbar = false } = {}) {
  if (!shellState.currentSummary) {
    return;
  }
  const hooks = requireWindowHooks();
  if (rerenderDesktop) {
    hooks.renderDesktop(shellState.currentSummary);
  }
  if (rerenderTaskbar) {
    hooks.renderTaskbar(shellState.currentSummary);
  }
}

function nextBrowserWindowId(targetId) {
  shellState.browserWindowSerial += 1;
  return `${targetId}--${shellState.browserWindowSerial}`;
}

function nowMs() {
  return window.performance ? window.performance.now() : Date.now();
}

function armWindowControlGuard(
  node,
  { controlMs = WINDOW_CONTROL_GUARD_MS, closeMs = 0 } = {},
) {
  node.dataset.controlGuardUntil = String(nowMs() + controlMs);
  node.dataset.closeGuardUntil = String(nowMs() + closeMs);
  if (closeMs > 0) {
    node.dataset.closeGuardPointerX = String(shellState.lastPointer.x);
    node.dataset.closeGuardPointerY = String(shellState.lastPointer.y);
    node.dataset.closeGuardIssuedAt = String(nowMs());
    return;
  }
  delete node.dataset.closeGuardPointerX;
  delete node.dataset.closeGuardPointerY;
  delete node.dataset.closeGuardIssuedAt;
}

function clearWindowCloseGuard(node) {
  node.dataset.closeGuardUntil = "0";
  delete node.dataset.closeGuardPointerX;
  delete node.dataset.closeGuardPointerY;
  delete node.dataset.closeGuardIssuedAt;
}

function clearWindowCloseGuardIfPointerMoved(node) {
  const originX = Number.parseFloat(node.dataset.closeGuardPointerX || "NaN");
  const originY = Number.parseFloat(node.dataset.closeGuardPointerY || "NaN");
  const issuedAt = Number.parseFloat(node.dataset.closeGuardIssuedAt || "NaN");
  if (
    !Number.isFinite(originX) ||
    !Number.isFinite(originY) ||
    !Number.isFinite(issuedAt) ||
    shellState.lastPointerMove.at <= issuedAt
  ) {
    return;
  }
  const distance = Math.hypot(
    shellState.lastPointerMove.x - originX,
    shellState.lastPointerMove.y - originY,
  );
  if (distance >= WINDOW_CLOSE_GUARD_MOVE_PX) {
    clearWindowCloseGuard(node);
  }
}

function shouldIgnoreWindowControl(node, action, event = null) {
  const isKeyboardActivation =
    event &&
    event.detail === 0 &&
    event.clientX === 0 &&
    event.clientY === 0;
  if (action === "close" && !isKeyboardActivation) {
    clearWindowCloseGuardIfPointerMoved(node);
  }
  const key = action === "close" ? "closeGuardUntil" : "controlGuardUntil";
  const until = Number.parseFloat(node.dataset[key] || "0");
  return Number.isFinite(until) && until > nowMs();
}

export function browserWindowEntries() {
  return Array.from(shellState.windows.values()).filter(
    (entry) => entry.kind === "browser",
  );
}

export function sortWindowEntriesByZOrder(entries) {
  return [...entries].sort(
    (left, right) => Number(right.node.style.zIndex || 0) - Number(left.node.style.zIndex || 0),
  );
}

export function browserWindowEntriesForTarget(targetId) {
  return browserWindowEntries().filter((entry) => entry.targetId === targetId);
}

export function browserWindowCount(targetId) {
  return browserWindowEntriesForTarget(targetId).length;
}

function topBrowserWindowEntryForTarget(targetId, options = {}) {
  const includeHidden = options.includeHidden !== false;
  const entries = browserWindowEntriesForTarget(targetId).filter(
    (entry) => includeHidden || !entry.node.classList.contains("hidden"),
  );
  return sortWindowEntriesByZOrder(entries)[0] || null;
}

export function activeBrowserTargetId() {
  const entry = shellState.activeWindowId
    ? shellState.windows.get(shellState.activeWindowId)
    : null;
  if (!entry || entry.kind !== "browser" || entry.node.classList.contains("hidden")) {
    return null;
  }
  return entry.targetId;
}

export function browserWindowDisplayTitle(entry) {
  if (!entry || entry.kind !== "browser") {
    return entry ? entry.title : "";
  }
  const entries = browserWindowEntriesForTarget(entry.targetId).sort(
    (left, right) => left.serial - right.serial,
  );
  if (entries.length <= 1) {
    return entry.title;
  }
  const ordinal = entries.findIndex((candidate) => candidate.id === entry.id) + 1;
  return `${entry.title} ${ordinal}`;
}

function restoreWindow(id) {
  const entry = shellState.windows.get(id);
  if (!entry) {
    return false;
  }
  entry.node.classList.remove("hidden");
  entry.node.inert = false;
  entry.node.setAttribute("aria-hidden", "false");
  armWindowControlGuard(entry.node);
  focusWindow(id);
  persistBrowserSession();
  return true;
}

export function showAllTargetWindows(targetId) {
  const entries = browserWindowEntriesForTarget(targetId);
  if (entries.length === 0) {
    return false;
  }
  for (const entry of entries) {
    entry.node.classList.remove("hidden");
    entry.node.inert = false;
    entry.node.setAttribute("aria-hidden", "false");
    armWindowControlGuard(entry.node);
  }
  const top = topBrowserWindowEntryForTarget(targetId);
  if (top) {
    focusWindow(top.id);
  } else {
    refreshWindowUi();
    persistBrowserSession();
  }
  return true;
}

function hideWindowEntries(entries) {
  if (entries.length === 0) {
    return false;
  }
  const hidActiveWindow = entries.some((entry) => shellState.activeWindowId === entry.id);
  const hidFocusedWindow = entries.some((entry) => windowOwnsFocus(entry.node));
  const movedFocus = hidActiveWindow || hidFocusedWindow
    ? moveFocusAwayFromEntries(entries)
    : false;
  for (const entry of entries) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
    entry.node.inert = true;
    entry.node.setAttribute("aria-hidden", "true");
  }
  if (hidActiveWindow || hidFocusedWindow) {
    if (!movedFocus) {
      shellState.activeWindowId = null;
    }
    refreshWindowUi();
    persistBrowserSession();
  } else {
    refreshWindowUi();
    persistBrowserSession();
  }
  return true;
}

function removeWindowEntries(entries) {
  if (entries.length === 0) {
    return false;
  }
  const removedActiveWindow = entries.some(
    (entry) => shellState.activeWindowId === entry.id,
  );
  const removedFocusedWindow = entries.some((entry) => windowOwnsFocus(entry.node));
  const returnFocusToWallet = entries.some(
    (entry) =>
      shellState.activeWindowId === entry.id &&
      WALLET_CONNECTOR_TARGETS.has(entry.targetId),
  );
  if (removedActiveWindow || removedFocusedWindow) {
    moveFocusAwayFromEntries(entries);
  }
  // Closing a fullscreen Space's only app retires that Space and lands next door.
  const activeStage = getActiveStageId();
  const closingFullscreen = entries.filter((entry) => entry.fullscreenStage === true);
  const leavingFullscreenSpace = closingFullscreen.find((entry) => entry.id === activeStage);
  const nextSpace = leavingFullscreenSpace
    ? neighborSpaceAfterClosing(leavingFullscreenSpace.id)
    : null;

  // Slide the dying fullscreen Space away while the neighbor slides in; the
  // closing window stays mounted until that motion finishes.
  if (leavingFullscreenSpace && nextSpace) {
    const closing = leavingFullscreenSpace;
    for (const entry of entries.filter((candidate) => candidate.id !== closing.id)) {
      tearDownWindowEntry(entry);
    }
    shellState.activeWindowId = null;
    const motioned = playCloseFullscreenSpaceMotion(closing.id, nextSpace, {
      onComplete: () => {
        tearDownWindowEntry(closing);
        renderWindowTaskbar();
        if (isDesktopSpace(nextSpace)) {
          focusTopVisibleWindow();
        } else {
          requireWindowHooks().refreshLauncherIfVisible();
          persistBrowserSession();
        }
      },
    });
    if (motioned) {
      renderWindowTaskbar();
      return true;
    }
    // Reduced motion or failed choreography — fall through to instant teardown.
  }

  for (const entry of entries) {
    tearDownWindowEntry(entry);
  }
  renderWindowTaskbar();
  if (nextSpace) {
    shellState.activeWindowId = null;
    setActiveStage(nextSpace, { animate: true, focus: true, announce: true });
    if (isDesktopSpace(nextSpace)) {
      focusTopVisibleWindow();
    }
    return true;
  }
  if (removedActiveWindow || removedFocusedWindow) {
    shellState.activeWindowId = null;
    const wallet = returnFocusToWallet
      ? topBrowserWindowEntryForTarget("wallet", { includeHidden: false })
      : null;
    if (wallet) {
      focusWindow(wallet.id);
    } else {
      focusTopVisibleWindow();
    }
  } else {
    requireWindowHooks().refreshLauncherIfVisible();
    persistBrowserSession();
  }
  return true;
}

function tearDownWindowEntry(entry) {
  if (!entry) {
    return;
  }
  cleanupFrameAutoFit(entry.node);
  shellState.windows.delete(entry.id);
  entry.node.remove();
  if (entry.fullscreenStage) {
    forgetClosedFullscreenSpace(entry.id);
  }
}

function hasExactKeys(value, expectedKeys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length &&
    actual.every((key, index) => key === expected[index]);
}

function browserWindowCloseContext(entry) {
  if (!entry || entry.targetId !== "browser") {
    return null;
  }
  const frame = entry.node.querySelector(".window-frame");
  let frameWindow = null;
  try {
    frameWindow = frame?.contentWindow || null;
  } catch (_error) {
    return null;
  }
  const route = frame?.dataset?.route || frame?.getAttribute("src") || "";
  let homeToken = "";
  try {
    const url = new URL(route, window.location.href);
    homeToken =
      new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") || "";
  } catch (_error) {
    return null;
  }
  const browserInstance =
    typeof entry.launchQuery?.browser_instance === "string"
      ? entry.launchQuery.browser_instance
      : "";
  if (!frameWindow || !homeToken || !browserInstance) {
    return null;
  }
  return { browserInstance, frameWindow, homeToken };
}

function browserWindowCloseRequestId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return window.crypto.randomUUID();
  }
  if (window.crypto && typeof window.crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return Array.from(
      bytes,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("");
  }
  throw new Error("Home GUI requires browser crypto for close isolation");
}

function markBrowserWindowCloseState(entry, state) {
  if (!entry || !shellState.windows.has(entry.id)) {
    return;
  }
  const closeButton = entry.node.querySelector("[data-action='close']");
  entry.node.dataset.browserCloseState = state;
  closeButton.disabled = state === "pending";
  if (state === "retry") {
    closeButton.setAttribute("aria-label", "Retry Browser close");
    closeButton.title = "Runtime cleanup is pending. Activate to retry close.";
    return;
  }
  closeButton.setAttribute("aria-label", "Close");
  closeButton.title = state === "pending" ? "Closing Browser…" : "";
}

function settleBrowserWindowClose(record, terminal) {
  window.clearTimeout(record.timeout);
  pendingBrowserWindowCloses.delete(record.requestId);
  if (record.entry.browserCloseRequest === record) {
    delete record.entry.browserCloseRequest;
  }
  if (terminal && shellState.windows.get(record.entry.id) === record.entry) {
    removeWindowEntries([record.entry]);
    record.resolve(true);
    return;
  }
  markBrowserWindowCloseState(record.entry, "retry");
  record.resolve(false);
}

function handleBrowserWindowCloseResult(event) {
  const message = event.data;
  if (
    event.origin !== OPAQUE_CAPSULE_ORIGIN ||
    !hasExactKeys(message, [
      "type",
      "requestId",
      "homeToken",
      "browserInstance",
      "state",
      "pageId",
      "generation",
      "cleanupId",
      "terminalKind",
      "reason",
    ]) ||
    message.type !== BROWSER_WINDOW_CLOSE_RESULT_TYPE
  ) {
    return;
  }
  const record = pendingBrowserWindowCloses.get(message.requestId);
  if (
    !record ||
    event.source !== record.frameWindow ||
    message.homeToken !== record.homeToken ||
    message.browserInstance !== record.browserInstance ||
    !["terminal", "pending", "error"].includes(message.state) ||
    typeof message.pageId !== "string" ||
    !Number.isSafeInteger(message.generation) ||
    message.generation < 0 ||
    typeof message.cleanupId !== "string" ||
    typeof message.terminalKind !== "string" ||
    typeof message.reason !== "string"
  ) {
    return;
  }
  const lifecycle = {
    pageId: message.pageId,
    generation: message.generation,
    cleanupId: message.cleanupId,
  };
  const matchesBoundLifecycle =
    !record.lifecycle ||
    (record.lifecycle.pageId === lifecycle.pageId &&
      record.lifecycle.generation === lifecycle.generation &&
      record.lifecycle.cleanupId === lifecycle.cleanupId);
  if (message.state === "pending") {
    if (
      message.terminalKind !== "" ||
      !message.reason ||
      !matchesBoundLifecycle
    ) {
      return;
    }
    const hasLifecycle = Boolean(message.pageId && message.cleanupId);
    if (
      hasLifecycle !== Boolean(message.pageId || message.cleanupId) ||
      (!hasLifecycle && message.generation !== 0)
    ) {
      return;
    }
    if (!record.lifecycle && hasLifecycle) {
      record.lifecycle = Object.freeze(lifecycle);
    }
    return;
  }
  if (message.state === "error") {
    if (message.terminalKind === "" && message.reason) {
      settleBrowserWindowClose(record, false);
    }
    return;
  }
  const terminal =
    message.reason === "" &&
    ((["closed", "already_absent"].includes(message.terminalKind) &&
      Boolean(message.pageId && message.cleanupId) &&
      Boolean(record.lifecycle) &&
      matchesBoundLifecycle) ||
      (message.terminalKind === "no_page" &&
        !record.lifecycle &&
        message.pageId === "" &&
        message.generation === 0 &&
        message.cleanupId === ""));
  if (terminal) {
    settleBrowserWindowClose(record, true);
  }
}

function requestBrowserWindowClose(entry) {
  if (entry.browserCloseRequest) {
    return entry.browserCloseRequest.promise;
  }
  const context = browserWindowCloseContext(entry);
  if (!context) {
    markBrowserWindowCloseState(entry, "retry");
    return Promise.resolve(false);
  }
  const requestId = browserWindowCloseRequestId();
  let resolveRequest;
  const promise = new Promise((resolve) => {
    resolveRequest = resolve;
  });
  const record = {
    ...context,
    entry,
    promise,
    requestId,
    resolve: resolveRequest,
    timeout: 0,
  };
  record.timeout = window.setTimeout(() => {
    settleBrowserWindowClose(record, false);
  }, BROWSER_WINDOW_CLOSE_TIMEOUT_MS);
  entry.browserCloseRequest = record;
  pendingBrowserWindowCloses.set(requestId, record);
  markBrowserWindowCloseState(entry, "pending");
  context.frameWindow.postMessage(
    {
      type: BROWSER_WINDOW_CLOSE_REQUEST_TYPE,
      requestId,
      homeToken: context.homeToken,
      browserInstance: context.browserInstance,
    },
    "*",
  );
  return promise;
}

function browserLaunchAuthority(route) {
  try {
    const url = new URL(route, window.location.href);
    return {
      browserInstance: url.searchParams.get("browser_instance") || "",
      homeToken:
        new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") ||
        "",
    };
  } catch (_error) {
    return null;
  }
}

export function renewBrowserWindowAuthority(id, options = {}) {
  const entry = shellState.windows.get(id);
  if (!entry || entry.targetId !== "browser") {
    return Promise.resolve(false);
  }
  if (entry.browserAuthorityRenewal) {
    return entry.browserAuthorityRenewal;
  }
  const launchQuery = normalizedLaunchQuery(entry.launchQuery);
  const browserInstance = launchQuery.browser_instance || "";
  const frame = entry.node.querySelector(".window-frame");
  const currentAuthority = browserLaunchAuthority(
    frame?.dataset?.route || frame?.getAttribute("src") || "",
  );
  if (!browserInstance || currentAuthority?.browserInstance !== browserInstance) {
    return Promise.resolve(false);
  }
  let renewal;
  renewal = Promise.resolve()
    .then(() =>
      requireWindowHooks().launchTarget("browser", launchQuery, options),
    )
    .then((launched) => {
      if (shellState.windows.get(id) !== entry) {
        return false;
      }
      const nextAuthority = browserLaunchAuthority(launched?.route || "");
      if (
        launched?.target !== "browser" ||
        launched.attach_kind !== "iframe" ||
        launchDidFail(launched) ||
        nextAuthority?.browserInstance !== browserInstance ||
        !nextAuthority.homeToken ||
        nextAuthority.homeToken === currentAuthority.homeToken
      ) {
        throw new Error("Browser authority renewal returned an invalid launch");
      }
      launched.title = canonicalTargetTitle(launched.target, launched.title);
      entry.title = launched.title;
      fitLaunchedWindow(entry);
      if (entry.browserCloseRequest) {
        settleBrowserWindowClose(entry.browserCloseRequest, false);
      }
      syncBrowserWindow(entry, launched);
      try {
        renderWindowTaskbar();
        persistBrowserSession();
      } catch (error) {
        console.warn("Home GUI could not persist renewed Browser authority", error);
      }
      return Object.freeze({
        browserInstance,
        freshHomeToken: nextAuthority.homeToken,
      });
    })
    .finally(() => {
      if (entry.browserAuthorityRenewal === renewal) {
        delete entry.browserAuthorityRenewal;
      }
    });
  entry.browserAuthorityRenewal = renewal;
  return renewal;
}

function activateTargetGroup(targetId) {
  const visibleTop = topBrowserWindowEntryForTarget(targetId, { includeHidden: false });
  const entry = visibleTop || topBrowserWindowEntryForTarget(targetId);
  if (!entry) {
    return false;
  }
  // Dock activation must Space-switch like Mission Control — focus alone
  // leaves a fullscreen app invisible while another Space is active.
  if (entry.fullscreenStage) {
    setActiveStage(entry.id);
    return true;
  }
  if (entry.node.classList.contains("hidden")) {
    return restoreWindow(entry.id);
  }
  const home = entry.desktopSpaceId || desktopStageId();
  if (!isDesktopSpace(getActiveStageId()) || getActiveStageId() !== home) {
    setActiveStage(home, { focus: false });
  }
  focusWindow(entry.id);
  return true;
}

export function hideAllTargetWindows(targetId) {
  return hideWindowEntries(browserWindowEntriesForTarget(targetId));
}

export function closeAllTargetWindows(targetId) {
  const entries = browserWindowEntriesForTarget(targetId);
  if (targetId !== "browser") {
    return removeWindowEntries(entries);
  }
  if (entries.length === 0) {
    return false;
  }
  return Promise.all(entries.map((entry) => closeWindow(entry.id))).then(
    (results) => results.every((result) => result === true),
  );
}

function renderSystemErrorWindow({
  id,
  title,
  headline,
  copy,
  subjectLabel,
  subjectValue,
  detail,
}) {
  let entry = shellState.windows.get(id);
  if (!entry) {
    const node = createWindow({
      id,
      title,
      x: 48,
      y: 60,
      width: 520,
      height: 220,
      tone: "default",
    });
    windowHostContainer().appendChild(node);
    entry = {
      id,
      node,
      kind: "system",
      title,
    };
    shellState.windows.set(id, entry);
  }

  entry.title = title;
  entry.node.querySelector(".window-head-title").textContent = title;
  entry.node.setAttribute("aria-label", title);
  const body = entry.node.querySelector(".window-body");
  body.replaceChildren();
  const errorNode = windowErrorTemplate.content.firstElementChild.cloneNode(true);
  errorNode.querySelector(".window-error-title").textContent = headline;
  errorNode.querySelector(".window-error-copy").textContent = copy;
  errorNode.querySelector(".window-error-subject-label").textContent = subjectLabel;
  errorNode.querySelector(".window-error-subject-value").textContent = subjectValue;
  const detailNode = errorNode.querySelector(".window-error-detail");
  detailNode.textContent = detail;
  detailNode.hidden = !detail;
  body.appendChild(errorNode);
  focusWindow(id);
}

function renderTargetLaunchError(targetId, error) {
  const title = shellState.currentSummary ? targetTitle(shellState.currentSummary, targetId) : targetId;
  console.error(`failed to launch ${targetId}`, error);
  renderSystemErrorWindow({
    id: "shell-launch-error",
    title,
    headline: `Could not open ${title}`,
    copy: "The app did not start. Close this window and try again.",
    subjectLabel: "App",
    subjectValue: title,
    detail: "",
  });
}

function createWindow({
  id,
  title,
  x,
  y,
  width,
  height,
  tone,
  glyphTarget,
  maximizeByDefault = true,
}) {
  const bounds = fitWindowBounds({ x, y, width, height });
  const node = windowTemplate.content.firstElementChild.cloneNode(true);
  node.dataset.windowId = id;
  node.dataset.maximized = "false";
  node.dataset.browserMaximized = "false";
  node.dataset.snap = "";
  node.setAttribute("aria-label", title);
  node.setAttribute("aria-hidden", "false");
  armWindowControlGuard(node);
  node.style.left = `${bounds.x}px`;
  node.style.top = `${bounds.y}px`;
  node.style.width = `${bounds.width}px`;
  node.style.height = `${bounds.height}px`;
  node.querySelector(".window-head-title").textContent = title;
  mountGlyph(node.querySelector(".window-head-icon"), glyphTarget || id, tone);

  node.addEventListener("pointerdown", () => {
    focusWindow(id);
  });

  node.querySelector("[data-action='close']").addEventListener("click", (event) => {
    if (shouldIgnoreWindowControl(node, "close", event)) {
      return;
    }
    clearWindowCloseGuard(node);
    closeWindow(id);
  });
  node.querySelector("[data-action='minimize']").addEventListener("click", () => {
    if (shouldIgnoreWindowControl(node, "minimize")) {
      return;
    }
    minimizeWindow(id);
  });
  node.querySelector("[data-action='maximize']").addEventListener("click", () => {
    if (shouldIgnoreWindowControl(node, "maximize")) {
      return;
    }
    toggleFullscreenStage(id);
  });

  const handle = node.querySelector(".window-head");
  handle.addEventListener("dblclick", () => {
    toggleWindowMaximize(id);
  });
  attachWindowDrag(node, handle, focusWindow, persistBrowserSession);
  attachWindowResize(node, focusWindow, persistBrowserSession);

  if (maximizeByDefault && shouldOpenMaximizedByDefault()) {
    node.dataset.restoreLeft = node.style.left;
    node.dataset.restoreTop = node.style.top;
    node.dataset.restoreWidth = node.style.width;
    node.dataset.restoreHeight = node.style.height;
    node.dataset.maximized = "true";
  }

  return node;
}

function normalizedLaunchQuery(query) {
  if (!query || typeof query !== "object") {
    return {};
  }
  const normalized = {};
  for (const [key, value] of Object.entries(query)) {
    if (typeof key !== "string" || !key.trim()) {
      continue;
    }
    normalized[key] = typeof value === "string" ? value : String(value ?? "");
  }
  return normalized;
}

function launchActionKey(targetId, query) {
  const pairs = Object.entries(normalizedLaunchQuery(query));
  if (pairs.length === 0) {
    return `open-target:${targetId}`;
  }
  return `open-target:${targetId}:${JSON.stringify(pairs)}`;
}

export function openTarget(targetId, options = {}) {
  if (windowHooks?.holdHomeSetupAct?.(targetId) === true) {
    return;
  }
  if (SINGLE_SESSION_TARGETS.has(targetId) && browserWindowCount(targetId) > 0) {
    activateTargetGroup(targetId);
    return;
  }
  // Launching from a fullscreen Space returns to a Desktop first, so the new
  // window opens somewhere it is visible.
  ensureDesktopForNewLaunch();
  const baseQuery = normalizedLaunchQuery(options.query);
  let guardedByBrowserActivation = false;
  if (targetId === "browser" && !baseQuery.browser_instance) {
    if (ignoreRepeatedAction("open-target:browser", BROWSER_DESKTOP_OPEN_GUARD_MS)) {
      return;
    }
    guardedByBrowserActivation = true;
  }
  const launchOptions = targetId === "browser"
    ? withBrowserInstanceQuery({ ...options, query: baseQuery })
    : { ...options, query: baseQuery };
  const pendingLaunchKey = guardedByBrowserActivation
    ? "open-target:browser:desktop"
    : launchActionKey(targetId, launchOptions.query);
  if (pendingWindowLaunches.has(pendingLaunchKey)) {
    return;
  }
  if (!guardedByBrowserActivation && ignoreRepeatedAction(pendingLaunchKey)) {
    return;
  }
  pendingWindowLaunches.add(pendingLaunchKey);
  launchBrowserTargetWindow(targetId, launchOptions)
    .catch((error) => {
      const status = Number(error && error.status);
      if (status === 401 || status === 403) {
        requireWindowHooks().requestHomeUnlock?.();
        return;
      }
      console.error(`failed to open ${targetId}`, error);
      renderTargetLaunchError(targetId, error);
    })
    .finally(() => {
      pendingWindowLaunches.delete(pendingLaunchKey);
    });
}

function withBrowserInstanceQuery(options) {
  const query = normalizedLaunchQuery(options.query);
  if (!query.browser_instance) {
    query.browser_instance = nextBrowserInstanceId();
  }
  return { ...options, query };
}

function nextBrowserInstanceId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return `browser:${window.crypto.randomUUID()}`;
  }
  if (window.crypto && typeof window.crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return `browser:${Array.from(
      bytes,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("")}`;
  }
  throw new Error("Home GUI requires browser crypto for window isolation");
}

export function showDesktopHome() {
  requireWindowHooks().hideLauncher();
  hideWindowSnapPreview();
  if (!isDesktopSpace(getActiveStageId())) {
    setActiveStage(desktopStageId(), { announce: false, focus: false });
  }
  for (const entry of shellState.windows.values()) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
    entry.node.setAttribute("aria-hidden", "true");
  }
  shellState.activeWindowId = null;
  requireWindowHooks().updateTaskbarState();
  persistBrowserSession();
}

export function handleTaskbarTargetClick(targetId) {
  if (ignoreRepeatedAction(`taskbar-target:${targetId}`)) {
    return;
  }
  const entry = shellState.activeWindowId
    ? shellState.windows.get(shellState.activeWindowId)
    : null;
  if (
    entry &&
    entry.kind === "browser" &&
    entry.targetId === targetId &&
    !entry.node.classList.contains("hidden")
  ) {
    hideWindow(entry.id);
    return;
  }
  if (browserWindowCount(targetId) === 0) {
    openTarget(targetId);
    return;
  }
  activateTargetGroup(targetId);
}

async function launchBrowserTargetWindow(targetId, options = {}) {
  const launchQuery = targetId === "browser"
    ? withBrowserInstanceQuery({ query: options.query }).query
    : normalizedLaunchQuery(options.query);
  const launched = options.authorizedLaunch
    ? { ...options.authorizedLaunch }
    : await requireWindowHooks().launchTarget(targetId, launchQuery);
  launched.title = canonicalTargetTitle(launched.target, launched.title);
  if (launched.attach_kind !== "iframe") {
    throw new Error(`unsupported attach kind: ${launched.attach_kind || "unknown"}`);
  }
  if (launched.target !== SYSTEM_APP_ID && launchDidFail(launched)) {
    throw new Error(
      typeof launched.launch_detail === "string" && launched.launch_detail.trim() !== ""
        ? launched.launch_detail.trim()
        : `launch status: ${launched.launch_status}`,
    );
  }
  const offset = browserWindowEntries().length;
  const restoredPlacement = options.restoredPlacement || null;
  const windowSpec = restoredPlacement || browserWindowSpec(launched, offset);
  const windowId = nextBrowserWindowId(launched.target);
  const node = createWindow({
    id: windowId,
    title: launched.title,
    x: windowSpec.x,
    y: windowSpec.y,
    width: windowSpec.width,
    height: windowSpec.height,
    tone: glyphTone(launched.target),
    glyphTarget: launched.target,
    maximizeByDefault: !WALLET_CONNECTOR_TARGETS.has(launched.target),
  });
  armWindowControlGuard(node, { closeMs: WINDOW_OPEN_CLOSE_GHOST_GUARD_MS });
  node.dataset.target = launched.target;
  applyWindowChrome(node, launched.target);
  const body = node.querySelector(".window-body");
  body.classList.add("window-body-frame");
  body.innerHTML = `
    <iframe
      class="window-frame"
      title="${escapeHtml(launched.title)}"
      allow="${iframeAllowForLaunch(launched)}"
      sandbox="${iframeSandboxForLaunch(launched)}"
    ></iframe>
  `;

  windowHostContainer().appendChild(node);
  if (restoredPlacement) {
    applyWindowPlacement(node, restoredPlacement);
  } else if (launched.target === "browser" && node.dataset.maximized === "true") {
    node.dataset.maximized = "false";
    node.dataset.browserMaximized = "true";
    fitWindowToLargestBrowserAspect(node);
  }
  const activeSpace = getActiveStageId();
  const entry = {
    id: windowId,
    targetId: launched.target,
    serial: shellState.browserWindowSerial,
    node,
    kind: "browser",
    title: launched.title,
    launchQuery,
    fullscreenStage: false,
    desktopSpaceId: isDesktopSpace(activeSpace) ? activeSpace : desktopStageId(),
  };
  shellState.windows.set(windowId, entry);
  syncBrowserWindow(entry, launched);
  if (restoredPlacement) {
    applyFullscreenStageFromPlacement(entry, restoredPlacement);
  }
  if (entry.targetId === "browser") {
    fitLaunchedWindow(entry);
  }
  if (restoredPlacement?.hidden) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
    entry.node.inert = true;
    entry.node.setAttribute("aria-hidden", "true");
  } else {
    focusWindow(windowId);
  }
  renderWindowTaskbar();
  if (!shellState.restoringSession) {
    persistBrowserSession();
  }
  return entry;
}

export function attachAuthorizedTarget(launched) {
  return launchBrowserTargetWindow(launched?.target, {
    authorizedLaunch: launched,
    query: {},
  });
}

function launchDidFail(launched) {
  return (
    typeof launched.launch_status === "string" &&
    launched.launch_status.trim() !== "" &&
    launched.launch_status !== "launched"
  );
}

// The frame starts transparent so a capsule never flashes white before it
// paints, but nothing ever revealed it again: every app window stayed
// invisible, painting correct, fully styled content into an opacity-0 frame.
// Load reveals it, and a fail-open timer reveals it regardless — a slow
// capsule is worth a flash, an unreadable window never is.
const FRAME_REVEAL_FAIL_OPEN_MS = 1_500;

function revealWindowFrame(frame, cause) {
  if (!frame) {
    return;
  }
  if (frame.dataset.frameRevealTimer) {
    window.clearTimeout(Number(frame.dataset.frameRevealTimer));
    delete frame.dataset.frameRevealTimer;
  }
  frame.classList.add("is-ready");
  frame.dataset.frameVisible = "true";
  frame.dataset.frameVisibleCause = cause;
  // The launch skeleton pulses under the frame and its rule says it is
  // "hidden the moment the iframe reveals" — this is that moment.
  const body = frame.closest(".window-body-frame");
  if (body) {
    body.dataset.frameVisible = "true";
    body.dataset.frameVisibleCause = cause;
  }
}

function armWindowFrameReveal(frame) {
  if (!frame) {
    return;
  }
  if (frame.dataset.frameRevealTimer) {
    window.clearTimeout(Number(frame.dataset.frameRevealTimer));
  }
  frame.classList.remove("is-ready");
  delete frame.dataset.frameVisible;
  delete frame.dataset.frameVisibleCause;
  delete frame.closest(".window-body-frame")?.dataset.frameVisible;
  delete frame.closest(".window-body-frame")?.dataset.frameVisibleCause;
  frame.dataset.frameRevealTimer = String(
    window.setTimeout(() => revealWindowFrame(frame, "timeout"), FRAME_REVEAL_FAIL_OPEN_MS),
  );
}

function syncBrowserWindow(entry, launched) {
  const node = entry.node;
  const frame = node.querySelector(".window-frame");
  node.dataset.target = launched.target;
  applyWindowChrome(node, launched.target);
  node.querySelector(".window-head-title").textContent = launched.title;
  node.setAttribute("aria-label", launched.title);
  cleanupFrameAutoFit(node);

  const syncLoadedFrame = () => {
    if (entry.targetId !== "browser") {
      installFrameAutoFit(node, frame);
    }
    fitLaunchedWindow(entry);
    revealWindowFrame(frame, "load");
  };

  frame.onload = syncLoadedFrame;
  if (frame.dataset.route !== launched.route) {
    armWindowFrameReveal(frame);
    frame.src = launched.route;
    frame.dataset.route = launched.route;
    return;
  }
  syncLoadedFrame();
}

function fitLaunchedWindow(entry) {
  if (!entry || entry.kind !== "browser") {
    return;
  }
  if (entry.targetId === "browser") {
    fitWindowToBrowserAspect(entry.node);
    rememberWindowRestoreBounds(entry.node);
    return;
  }
  fitWindowToFrame(entry.node, entry.node.querySelector(".window-frame"));
}

function installFrameAutoFit(node, frame) {
  cleanupFrameAutoFit(node);

  let rafId = 0;
  const scheduleFit = () => {
    if (rafId !== 0) {
      return;
    }
    rafId = window.requestAnimationFrame(() => {
      rafId = 0;
      fitWindowToFrame(node, frame);
    });
  };

  const timeouts = [
    window.setTimeout(scheduleFit, 90),
    window.setTimeout(scheduleFit, 280),
    window.setTimeout(scheduleFit, 900),
  ];

  let resizeObserver = null;
  let frameWindow = null;
  try {
    const doc = frame.contentDocument;
    frameWindow = frame.contentWindow;
    if (doc && window.ResizeObserver) {
      resizeObserver = new ResizeObserver(() => {
        scheduleFit();
      });
      if (doc.documentElement) {
        resizeObserver.observe(doc.documentElement);
      }
      if (doc.body && doc.body !== doc.documentElement) {
        resizeObserver.observe(doc.body);
      }
    }
    if (frameWindow) {
      frameWindow.addEventListener("resize", scheduleFit);
    }
  } catch (_error) {
    // Cross-frame observer setup is best-effort.
  }

  shellState.frameAutoFitCleanup.set(node, () => {
    if (rafId !== 0) {
      window.cancelAnimationFrame(rafId);
    }
    for (const timeout of timeouts) {
      window.clearTimeout(timeout);
    }
    if (resizeObserver) {
      resizeObserver.disconnect();
    }
    if (frameWindow) {
      try {
        frameWindow.removeEventListener("resize", scheduleFit);
      } catch (_error) {
        // The frame may have navigated into a cross-origin or failed state.
      }
    }
  });
}

function cleanupFrameAutoFit(node) {
  const cleanup = shellState.frameAutoFitCleanup.get(node);
  if (!cleanup) {
    return;
  }
  try {
    cleanup();
  } catch (_error) {
    // Cross-origin teardown can throw when a frame navigates before cleanup.
  }
  shellState.frameAutoFitCleanup.delete(node);
}

export function minimizeWindow(id) {
  const entry = shellState.windows.get(id);
  // Same rule as the yellow control — never leave a hidden fullscreen Space ghost.
  if (entry?.fullscreenStage) {
    exitFullscreenStage(id);
    return;
  }
  hideWindow(id);
}

export function maximizeActiveWindow() {
  if (shellState.activeWindowId) {
    toggleWindowMaximize(shellState.activeWindowId);
  }
}

function hideWindow(id) {
  const entry = shellState.windows.get(id);
  if (!entry) {
    return;
  }
  hideWindowEntries([entry]);
}

export function closeWindow(id) {
  const entry = shellState.windows.get(id);
  if (!entry) {
    return false;
  }
  if (entry.targetId === "browser") {
    return requestBrowserWindowClose(entry);
  }
  return removeWindowEntries([entry]);
}

function focusTopVisibleWindow() {
  const active = getActiveStageId();
  const visible = sortWindowEntriesByZOrder(
    Array.from(shellState.windows.values()).filter(
      (entry) =>
        !entry.node.classList.contains("hidden") &&
        windowVisibleOnActiveSpace(entry, active),
    ),
  );

  if (visible.length === 0) {
    shellState.activeWindowId = null;
    focusStableShellControl();
    refreshWindowUi();
    persistBrowserSession();
    return;
  }

  focusWindow(visible[0].id);
}

function browserWindowSpec(launched, offset) {
  if (WALLET_CONNECTOR_TARGETS.has(launched.target)) {
    return walletConnectorWindowSpec();
  }
  if (launched.target === SYSTEM_APP_ID) {
    return {
      x: 36,
      y: 44,
      width: 980,
      height: 620,
    };
  }
  if (launched.target === "gba-emulator") {
    return {
      x: 88 + offset * 24,
      y: 62 + offset * 20,
      width: 900,
      height: 620,
    };
  }
  if (launched.target === "browser") {
    return {
      x: 48 + offset * 18,
      y: 54 + offset * 18,
      width: 1280,
      height: 804,
    };
  }
  return {
    x: 104 + offset * 26,
    y: 78 + offset * 22,
    width: 1040,
    height: 720,
  };
}

function walletConnectorWindowSpec() {
  const workspaceRect = desktop.getBoundingClientRect();
  const workspaceWidth = Math.max(
    WALLET_CONNECTOR_WINDOW_WIDTH,
    window.innerWidth - workspaceRect.left,
  );
  const leftX = WALLET_CONNECTOR_WINDOW_EDGE_INSET;
  const rightX = Math.max(
    WALLET_CONNECTOR_WINDOW_EDGE_INSET,
    workspaceWidth -
      WALLET_CONNECTOR_WINDOW_WIDTH -
      WALLET_CONNECTOR_WINDOW_EDGE_INSET,
  );
  const wallet = topBrowserWindowEntryForTarget("wallet", {
    includeHidden: false,
  });
  if (!wallet) {
    return {
      x: rightX,
      y: 72,
      width: WALLET_CONNECTOR_WINDOW_WIDTH,
      height: WALLET_CONNECTOR_WINDOW_HEIGHT,
    };
  }
  const walletBounds = currentWindowBounds(wallet.node);
  const overlapWidth = (x) => Math.max(
    0,
    Math.min(
      x + WALLET_CONNECTOR_WINDOW_WIDTH,
      walletBounds.x + walletBounds.width,
    ) - Math.max(x, walletBounds.x),
  );
  return {
    x: overlapWidth(leftX) <= overlapWidth(rightX) ? leftX : rightX,
    y: walletBounds.y + 28,
    width: WALLET_CONNECTOR_WINDOW_WIDTH,
    height: WALLET_CONNECTOR_WINDOW_HEIGHT,
  };
}

function fitWindowToFrame(node, frame) {
  if (!node || !frame || node.dataset.maximized === "true" || node.dataset.snap) {
    return;
  }
  try {
    const doc = frame.contentDocument;
    if (!doc) {
      return;
    }
    if (doc.fullscreenElement) {
      return;
    }
    const body = doc.body;
    const root = doc.documentElement;
    if (!body || !root) {
      return;
    }
    if (body.dataset.shellWindowFit === "fixed") {
      return;
    }
    const contentWidth = Math.max(
      body.scrollWidth,
      body.offsetWidth,
      root.scrollWidth,
      root.offsetWidth,
    );
    const contentHeight = Math.max(
      body.scrollHeight,
      body.offsetHeight,
      root.scrollHeight,
      root.offsetHeight,
    );
    if (!contentWidth || !contentHeight) {
      return;
    }
    const windowBody = node.querySelector(".window-body");
    if (!windowBody) {
      return;
    }
    const currentWidth = Number.parseFloat(node.style.width) || 0;
    const currentHeight = Number.parseFloat(node.style.height) || 0;
    const chromeHeight = node.offsetHeight - windowBody.getBoundingClientRect().height;
    const bounds = fitWindowBounds({
      x: Number.parseFloat(node.style.left) || 48,
      y: Number.parseFloat(node.style.top) || 60,
      width: Math.max(currentWidth, 440, contentWidth),
      height: Math.max(currentHeight, 220, contentHeight + chromeHeight),
    }, { allowPartial: true });
    node.style.left = `${bounds.x}px`;
    node.style.top = `${bounds.y}px`;
    node.style.width = `${bounds.width}px`;
    node.style.height = `${bounds.height}px`;
  } catch (_error) {
    // Auto-fit is best-effort and should not surface console noise.
  }
}

export function focusWindow(id) {
  const entry = shellState.windows.get(id);
  if (!entry) {
    return;
  }

  for (const candidate of shellState.windows.values()) {
    candidate.node.classList.remove("window-active");
  }

  entry.node.classList.remove("hidden");
  entry.node.inert = false;
  entry.node.classList.add("window-active");
  entry.node.setAttribute("aria-hidden", "false");
  shellState.zIndexCounter += 1;
  entry.node.style.zIndex = String(shellState.zIndexCounter);
  shellState.activeWindowId = id;
  if (entry.kind === "browser") {
    rememberRecentTarget(entry.targetId);
    fitLaunchedWindow(entry);
  }
  focusWindowControl(entry);
  refreshWindowUi();
  persistBrowserSession();
}

function windowOwnsFocus(node) {
  const active = document.activeElement;
  return Boolean(active && node && (active === node || node.contains?.(active)));
}

function focusWindowControl(entry) {
  const node = entry?.node;
  if (!node) {
    return false;
  }
  const target =
    node.querySelector("[data-action='close']") ||
    node.querySelector("[data-action='maximize']") ||
    node.querySelector("[data-action='minimize']") ||
    node.querySelector(".window-head-draggable") ||
    node;
  if (
    !target ||
    target.hidden ||
    target.inert ||
    target.disabled ||
    typeof target.focus !== "function"
  ) {
    return false;
  }
  target.focus();
  return true;
}

function topVisibleWindowEntryExcluding(entries) {
  const excludedIds = new Set(entries.map((entry) => entry.id));
  const active = getActiveStageId();
  return sortWindowEntriesByZOrder(
    Array.from(shellState.windows.values()).filter(
      (entry) =>
        !excludedIds.has(entry.id) &&
        !entry.node.classList.contains("hidden") &&
        windowVisibleOnActiveSpace(entry, active),
    ),
  )[0] || null;
}

function moveFocusAwayFromEntries(entries) {
  const next = topVisibleWindowEntryExcluding(entries);
  if (next) {
    focusWindow(next.id);
    return true;
  }
  return focusStableShellControl();
}

function focusStableShellControl() {
  for (const selector of ["#toolbar-home", "#launcher-toggle", ".desktop-shortcut", "body"]) {
    const node = document.querySelector(selector);
    if (!node || node.hidden || node.inert || node.disabled) {
      continue;
    }
    if (typeof node.focus === "function") {
      node.focus();
      return true;
    }
  }
  return false;
}

function toggleWindowMaximize(id) {
  const entry = shellState.windows.get(id);
  if (!entry) {
    return;
  }
  const node = entry.node;
  armWindowControlGuard(node, { closeMs: WINDOW_MAXIMIZE_CLOSE_GUARD_MS });
  if (
    node.dataset.maximized === "true" ||
    node.dataset.browserMaximized === "true"
  ) {
    restoreWindowFromSpecialState(node);
    fitLaunchedWindow(entry);
    focusWindow(id);
    persistBrowserSession();
    return;
  }
  rememberWindowRestoreBounds(node);
  node.dataset.snap = "";
  if (entry.targetId === "browser") {
    node.dataset.maximized = "false";
    node.dataset.browserMaximized = "true";
    fitWindowToLargestBrowserAspect(node);
    focusWindow(id);
    persistBrowserSession();
    return;
  }
  node.dataset.browserMaximized = "false";
  node.dataset.maximized = "true";
  focusWindow(id);
  persistBrowserSession();
}

export async function restoreShellSession() {
  if (!shellState.currentSummary || browserWindowEntries().length > 0) {
    return;
  }
  const storedSession = loadShellSessionState();
  restoreExtraDesktops(storedSession?.desktops);
  const restoredWindows = normalizeRestorableSession(
    shellState.currentSummary,
    storedSession,
    { rootShell: currentRootShellSessionId() },
  );
  if (restoredWindows.length === 0) {
    restoreSessionSpaces(storedSession, []);
    return;
  }

  shellState.restoringSession = true;
  const restoredEntries = [];
  const restoredSingleSessionTargets = new Set();
  for (const restoredWindow of restoredWindows) {
    if (SINGLE_SESSION_TARGETS.has(restoredWindow.target)) {
      if (restoredSingleSessionTargets.has(restoredWindow.target)) {
        continue;
      }
      restoredSingleSessionTargets.add(restoredWindow.target);
    }
    try {
      const entry = await launchBrowserTargetWindow(restoredWindow.target, {
        restoredPlacement: restoredWindow,
        query: restoredWindow.query,
      });
      if (entry) {
        restoredEntries.push({ entry, restoredWindow });
      }
    } catch (_error) {
      // Skip targets that can no longer be opened and normalize the saved session below.
    }
  }
  shellState.restoringSession = false;

  restoreSessionSpaces(storedSession, restoredEntries);

  if (restoredEntries.length === 0) {
    return;
  }

  /* One liquid dock intro (width + Bin ride + fade runners) — not N breathes. */
  const intro = requireWindowHooks().introduceDockAfterSessionRestore;
  if (typeof intro === "function") {
    try {
      await intro();
    } catch (_error) {
      renderWindowTaskbar();
    }
  }

  const activeEntry = restoredEntries.find(
    ({ restoredWindow }) => restoredWindow.active && !restoredWindow.hidden,
  );
  if (activeEntry) {
    const entry = activeEntry.entry;
    if (windowVisibleOnActiveSpace(entry, getActiveStageId())) {
      focusWindow(entry.id);
    } else {
      focusTopVisibleWindow();
    }
    return;
  }
  focusTopVisibleWindow();
}

/* Restore the Space the user was on — not "whichever window was focused".
   Stable fs: keys remap to live window ids only after windows remount. */
function restoreSessionSpaces(storedSession, restoredEntries) {
  const remappedOrder = (
    Array.isArray(storedSession?.space_order) ? storedSession.space_order : []
  ).map((key) => resolveStableSpaceKey(key, restoredEntries));
  restoreSpaceOrder(remappedOrder);
  const savedStageKey =
    typeof storedSession?.active_stage === "string" && storedSession.active_stage.trim()
      ? storedSession.active_stage.trim()
      : desktopStageId();
  const savedStage = resolveStableSpaceKey(savedStageKey, restoredEntries);
  setActiveStage(savedStage, { announce: false, animate: false, focus: false });
}

bindStageWindowHooks({
  focusWindow,
  persistSession: persistBrowserSession,
});

export function cleanupBeforeUnload() {
  persistBrowserSession();
  if (shellState.clockTimer !== null) {
    window.clearInterval(shellState.clockTimer);
  }
  for (const entry of shellState.windows.values()) {
    cleanupFrameAutoFit(entry.node);
  }
}

export function handleShellResize() {
  hideWindowSnapPreview();
  for (const entry of shellState.windows.values()) {
    if (entry.kind !== "browser") {
      continue;
    }
    fitLaunchedWindow(entry);
  }
  if (clampDesktopLayoutToViewport()) {
    saveShellLayoutState();
  }
  rerenderShellSurfaces({ desktop: true, taskbar: true });
  persistBrowserSession();
}
