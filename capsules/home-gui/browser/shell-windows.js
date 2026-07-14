import {
  desktop,
  HOME_GUI_SHELL_ID,
  windowTemplate,
  windowErrorTemplate,
  PEOPLE_TARGET_ID,
  SYSTEM_APP_ID,
  shellState,
  fetchJson,
  targetTitle,
  canonicalTargetTitle,
  escapeHtml,
  shouldOpenMaximizedByDefault,
  mountGlyph,
  glyphTone,
  rememberRecentTarget,
  clampDesktopLayoutToViewport,
  saveShellLayoutState,
  loadShellSessionState,
  saveShellSessionState,
  clearShellSessionState,
  ignoreRepeatedAction,
  targetById,
} from "./shell-core.js?v=home-20260705a";
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
} from "./shell-window-geometry.js?v=home-20260705a";

let windowHooks = null;
const PEOPLE_DISCOVERY_AUTO_REFRESH_INITIAL_MS = 1_500;
const PEOPLE_DISCOVERY_AUTO_REFRESH_FAST_MS = 3_000;
const PEOPLE_DISCOVERY_AUTO_REFRESH_STABLE_MS = 15_000;
const PEOPLE_DISCOVERY_AUTO_REFRESH_IDLE_MS = 30_000;
const PEOPLE_DISCOVERY_AUTO_REFRESH_MAX_MS = 60_000;
const REQUIRED_WINDOW_HOOKS = [
  "clearIdentitySurface",
  "hideLauncher",
  "refreshLauncherIfVisible",
  "renderDesktop",
  "renderTaskbar",
  "updateTaskbarState",
];
const WINDOW_CONTROL_GUARD_MS = 400;
const WINDOW_MAXIMIZE_CLOSE_GUARD_MS = 360;
const WINDOW_OPEN_CLOSE_GHOST_GUARD_MS = 2600;
const WINDOW_CLOSE_GUARD_MOVE_PX = 18;
const BROWSER_DESKTOP_OPEN_GUARD_MS = 700;
const MAX_SESSION_WINDOWS = 24;
const SINGLE_SESSION_TARGETS = new Set([PEOPLE_TARGET_ID, "inbox", "wallet"]);
const COMMON_IFRAME_SANDBOX = [
  "allow-downloads",
  "allow-forms",
  "allow-modals",
  "allow-pointer-lock",
  "allow-scripts",
];
// Same-origin frames are presentation compatibility for current local API
// capsules. Runtime launch tokens plus provider gates are authoritative.
const SAME_ORIGIN_PRESENTATION_IFRAME_TARGETS = new Set([
  "agent",
  "archive-manager",
  "browser",
  "chat",
  "chat-room",
  "documents",
  "gba-emulator",
  "gba-ucity",
  "inbox",
  "library",
  "marketplace",
  "services",
  SYSTEM_APP_ID,
  "wallet",
  "wallet-metamask",
  "wallet-unisat",
  "wallet-walletconnect",
]);
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
const BROWSER_IFRAME_ALLOW_EXTRAS = ["clipboard-read", "clipboard-write"];
const WEBAUTHN_IFRAME_ALLOW_TARGETS = new Set(["inbox", "wallet"]);
const pendingWindowLaunches = new Set();
const peopleDiscoveryRefreshTimers = new WeakMap();

function iframeSandboxForLaunch(launched) {
  const tokens = [...COMMON_IFRAME_SANDBOX];
  if (SAME_ORIGIN_PRESENTATION_IFRAME_TARGETS.has(launched?.target)) {
    tokens.push("allow-same-origin");
  }
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

function iframeAllowForLaunch(launched) {
  const tokens = [...COMMON_IFRAME_ALLOW];
  if (launched?.target === "browser") {
    tokens.push(...BROWSER_IFRAME_ALLOW_EXTRAS);
  }
  if (WEBAUTHN_IFRAME_ALLOW_TARGETS.has(launched?.target)) {
    tokens.push("publickey-credentials-get");
  }
  return tokens.join("; ");
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
    browserWindowEntries(),
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

function persistBrowserSession() {
  if (shellState.restoringSession) {
    return;
  }
  const rootShell = currentRootShellSessionId();
  const windows = persistedBrowserSessionEntries();
  if (windows.length === 0) {
    saveShellSessionState({ root_shell: rootShell, windows: [] });
    return;
  }
  saveShellSessionState({ root_shell: rootShell, windows });
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
  for (const entry of entries) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
    entry.node.setAttribute("aria-hidden", "true");
  }
  if (hidActiveWindow) {
    shellState.activeWindowId = null;
    focusTopVisibleWindow();
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
  for (const entry of entries) {
    cleanupFrameAutoFit(entry.node);
    cleanupPeopleDiscoveryAutoRefresh(entry.node);
    shellState.windows.delete(entry.id);
    entry.node.remove();
  }
  renderWindowTaskbar();
  if (removedActiveWindow) {
    shellState.activeWindowId = null;
    focusTopVisibleWindow();
  } else {
    requireWindowHooks().refreshLauncherIfVisible();
    persistBrowserSession();
  }
  return true;
}

function activateTargetGroup(targetId) {
  const visibleTop = topBrowserWindowEntryForTarget(targetId, { includeHidden: false });
  if (visibleTop) {
    focusWindow(visibleTop.id);
    return true;
  }
  const restoreTarget = topBrowserWindowEntryForTarget(targetId);
  if (!restoreTarget) {
    return false;
  }
  return restoreWindow(restoreTarget.id);
}

export function hideAllTargetWindows(targetId) {
  return hideWindowEntries(browserWindowEntriesForTarget(targetId));
}

export function closeAllTargetWindows(targetId) {
  return removeWindowEntries(browserWindowEntriesForTarget(targetId));
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
  errorNode.querySelector(".window-error-detail").textContent = detail;
  body.appendChild(errorNode);
  focusWindow(id);
}

function renderTargetLaunchError(targetId, error) {
  const title = shellState.currentSummary ? targetTitle(shellState.currentSummary, targetId) : targetId;
  renderSystemErrorWindow({
    id: "shell-launch-error",
    title,
    headline: `Could not open ${title}`,
    copy: "Home asked the runtime to open this item, but the launch did not complete.",
    subjectLabel: "Item ID",
    subjectValue: targetId,
    detail: String(error.message || error),
  });
}

function createWindow({ id, title, x, y, width, height, tone, glyphTarget }) {
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
    hideWindow(id);
  });
  node.querySelector("[data-action='maximize']").addEventListener("click", () => {
    if (shouldIgnoreWindowControl(node, "maximize")) {
      return;
    }
    toggleWindowMaximize(id);
  });

  const handle = node.querySelector(".window-head");
  handle.addEventListener("dblclick", () => {
    toggleWindowMaximize(id);
  });
  attachWindowDrag(node, handle, focusWindow, persistBrowserSession);
  attachWindowResize(node, focusWindow, persistBrowserSession);

  if (shouldOpenMaximizedByDefault()) {
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
  if (SINGLE_SESSION_TARGETS.has(targetId) && browserWindowCount(targetId) > 0) {
    activateTargetGroup(targetId);
    return;
  }
  if (targetId === PEOPLE_TARGET_ID) {
    openPeopleWindow(options);
    return;
  }
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

function openPeopleWindow(options = {}) {
  const summary = shellState.currentSummary;
  if (!summary) {
    return null;
  }
  const restoredPlacement = options.restoredPlacement || null;
  const offset = browserWindowEntries().length;
  const windowSpec = restoredPlacement || peopleWindowSpec(offset);
  const windowId = nextBrowserWindowId(PEOPLE_TARGET_ID);
  const node = createWindow({
    id: windowId,
    title: "People",
    x: windowSpec.x,
    y: windowSpec.y,
    width: windowSpec.width,
    height: windowSpec.height,
    tone: glyphTone(PEOPLE_TARGET_ID),
    glyphTarget: PEOPLE_TARGET_ID,
  });
  armWindowControlGuard(node, { closeMs: WINDOW_OPEN_CLOSE_GHOST_GUARD_MS });
  node.dataset.target = PEOPLE_TARGET_ID;
  const entry = {
    id: windowId,
    targetId: PEOPLE_TARGET_ID,
    serial: shellState.browserWindowSerial,
    node,
    kind: "browser",
    title: "People",
  };
  shellState.windows.set(windowId, entry);
  renderPeopleWindowBody(entry, summary);
  windowHostContainer().appendChild(node);
  if (restoredPlacement) {
    applyWindowPlacement(node, restoredPlacement);
  }
  renderWindowTaskbar();
  focusWindow(windowId);
  if (restoredPlacement?.hidden) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
    entry.node.setAttribute("aria-hidden", "true");
  }
  if (!shellState.restoringSession) {
    persistBrowserSession();
  }
  return entry;
}

export function refreshHomeInternalWindows(summary) {
  for (const entry of shellState.windows.values()) {
    if (entry.targetId === PEOPLE_TARGET_ID) {
      renderPeopleWindowBody(entry, summary || shellState.currentSummary);
    }
  }
}

function renderPeopleWindowBody(entry, summary) {
  if (!entry || !summary) {
    return;
  }
  const people = summary.people && typeof summary.people === "object" ? summary.people : {};
  const identity = summary.identity && typeof summary.identity === "object" ? summary.identity : {};
  const contacts = Array.isArray(people.contacts) ? people.contacts : [];
  const discovery = people.discovery && typeof people.discovery === "object" ? people.discovery : {};
  const discoveredPeers = filterDiscoveredPeople(
    Array.isArray(discovery.discovered_peers) ? discovery.discovered_peers : [],
    contacts,
  );
  const discoveryRequests = Array.isArray(discovery.requests)
    ? discovery.requests.filter(peopleDiscoveryRequestIsVisible)
    : [];
  const body = entry.node.querySelector(".window-body");
  body.classList.remove("window-body-frame");
  body.classList.add("home-people-body");
  const profileMarkup = peopleProfileMarkup(identity);
  const peopleMarkup = contacts.length === 0
    ? `
      <div class="home-people-empty">
        <h3>No people yet</h3>
        <p>Turn on Discovery to find another ElastOS home and send a request.</p>
      </div>
    `
    : contacts.map(peopleListCardMarkup).join("");
  body.innerHTML = `
    <section class="home-people-shell" aria-label="People">
      <aside class="home-people-sidebar" aria-label="People sections">
        <button class="home-people-sidebar-item active" type="button" data-people-jump="people">
          <span class="home-people-sidebar-icon home-people-sidebar-icon-people" aria-hidden="true"></span>
          <span class="home-people-sidebar-text">People</span>
        </button>
        <button class="home-people-sidebar-item" type="button" data-people-jump="discovery">
          <span class="home-people-sidebar-icon home-people-sidebar-icon-discovery" aria-hidden="true"></span>
          <span class="home-people-sidebar-text">Discovery</span>
        </button>
      </aside>
      <main class="home-people-main-panel">
        <div class="home-people-status" role="status" hidden></div>
        <div class="home-people-content">
          <section class="home-people-section" data-people-section="people" aria-label="People">
            ${profileMarkup}
            <div class="home-people-list">${peopleMarkup}</div>
          </section>
          <section class="home-people-section" data-people-section="discovery" aria-label="Discovery">
            ${peopleDiscoveryMarkup(discovery, discoveredPeers, discoveryRequests)}
          </section>
        </div>
      </main>
    </section>
  `;
  bindPeopleWindowActions(body);
  schedulePeopleDiscoveryAutoRefresh(body, discovery);
}

function peopleProfileMarkup(identity) {
  const displayName = peopleProfileDisplayName(identity);
  return `
    <section class="home-people-profile-card" aria-labelledby="home-people-profile-title">
      <div class="home-people-profile-copy">
        <h3 id="home-people-profile-title">My Profile</h3>
        <p>Shown to people you connect with.</p>
      </div>
      <form class="home-people-profile-form" data-people-profile-form>
        <label class="home-people-profile-label" for="home-people-profile-name">Display name</label>
        <input id="home-people-profile-name" class="home-people-profile-input" type="text" maxlength="32" autocomplete="nickname" placeholder="Your name" value="${escapeHtml(displayName)}" data-people-profile-input>
        <button class="home-people-action" type="submit" data-people-profile-save>Save</button>
      </form>
    </section>
  `;
}

function peopleDiscoveryMarkup(discovery, peers, requests) {
  const visibleRequests = requests.filter(peopleDiscoveryRequestIsVisible);
  const remainingSeconds = peopleDiscoveryRemainingSeconds(discovery);
  const enabled = discovery.enabled === true && remainingSeconds > 0;
  const remainingLabel = peopleDiscoveryRemainingText(remainingSeconds);
  const peerMarkup = peers.length
    ? peers.map(peopleDiscoveryPeerMarkup).join("")
    : `<div class="home-people-empty"><h3>No visible people yet</h3><p>Turn on discovery for 10 minutes while another ElastOS home is discoverable. People will appear automatically.</p></div>`;
  const requestMarkup = visibleRequests.length
    ? visibleRequests.map(peopleDiscoveryRequestMarkup).join("")
    : `<div class="home-people-empty"><h3>No requests</h3><p>Requests to add people will appear here.</p></div>`;
  return `
    <div class="home-people-discovery-grid">
      <div class="home-people-discovery-column">
        <div class="home-people-discovery-header">
          <h4>Visible People</h4>
          <div class="home-people-discovery-actions" aria-label="Discovery controls">
            ${enabled ? `<span class="home-people-discovery-countdown" data-people-discovery-countdown>Discoverable for ${remainingLabel}</span>` : ""}
            <button class="home-people-action" type="button" data-people-action="toggle-discovery" data-discovery-enabled="${enabled ? "false" : "true"}">${enabled ? "Stop" : "Turn On"}</button>
            <button class="home-people-action" type="button" data-people-action="refresh-discovery">Refresh</button>
          </div>
        </div>
        <div class="home-people-list">${peerMarkup}</div>
      </div>
      <div class="home-people-discovery-column">
        <h4>Requests</h4>
        <div class="home-people-list">${requestMarkup}</div>
      </div>
    </div>
  `;
}

function peopleDiscoveryRemainingSeconds(discovery) {
  const value = Number(discovery?.remaining_seconds || 0);
  return Number.isFinite(value) && value > 0 ? Math.ceil(value) : 0;
}

function peopleDiscoveryRemainingText(seconds) {
  if (seconds <= 0) {
    return "0 sec";
  }
  if (seconds >= 60) {
    const minutes = Math.ceil(seconds / 60);
    return `${minutes} min`;
  }
  return `${seconds} sec`;
}

function peopleDiscoveryPeerMarkup(peer) {
  const peerId = normalizePeopleText(peer?.peer_id, "");
  const rawDisplayName = peopleDisplayName(peer, "Visible person");
  const displayName = escapeHtml(rawDisplayName);
  const handle = normalizePeopleText(peer?.handle, "");
  const status = escapeHtml(normalizePeopleText(peer?.status, "visible"));
  const handleCopy = handle && handle !== rawDisplayName ? handle : "Discoverable";
  return `
    <article class="home-people-card">
      <div class="home-people-avatar" aria-hidden="true">${displayName.slice(0, 1).toUpperCase() || "E"}</div>
      <div class="home-people-card-copy">
        <h3>${displayName}</h3>
        <p><span>${escapeHtml(handleCopy)}</span><span>${status}</span></p>
      </div>
      <div class="home-people-card-actions">
        <button class="home-people-action" type="button" data-people-action="request-peer" data-peer-id="${escapeHtml(peerId)}" ${peerId ? "" : "disabled"}>Request</button>
      </div>
    </article>
  `;
}

function peopleDiscoveryRequestMarkup(request) {
  const requestId = escapeHtml(normalizePeopleText(request?.request_id, ""));
  const rawDisplayName = peopleDisplayName(request, "Person");
  const displayName = escapeHtml(rawDisplayName);
  const status = escapeHtml(normalizePeopleText(request?.status, "requested"));
  const rawStatus = normalizePeopleText(request?.status, "requested");
  const actionMarkup = rawStatus === "incoming"
    ? `<button class="home-people-action" type="button" data-people-action="accept-request" data-request-id="${requestId}" ${requestId ? "" : "disabled"}>Accept</button>`
    : rawStatus === "requested"
      ? `<span class="home-people-badge">Requested</span>`
      : "";
  return `
    <article class="home-people-card">
      <div class="home-people-avatar" aria-hidden="true">${displayName.slice(0, 1).toUpperCase() || "E"}</div>
      <div class="home-people-card-copy">
        <h3>${displayName}</h3>
        <p><span>${status}</span></p>
      </div>
      ${actionMarkup ? `<div class="home-people-card-actions">${actionMarkup}</div>` : ""}
    </article>
  `;
}

function peopleListCardMarkup(contact) {
  const rawDisplayName = peopleDisplayName(contact, "Person");
  const displayName = escapeHtml(rawDisplayName);
  const relationship = escapeHtml(normalizePeopleText(contact?.relationship, "connected"));
  const handle = normalizePeopleText(contact?.handle, "");
  const device = normalizePeopleText(contact?.device_label, "");
  const handleLine = handle && handle !== rawDisplayName ? `<span>${escapeHtml(handle)}</span>` : "";
  const deviceLine = device && device !== rawDisplayName ? `<span>${escapeHtml(device)}</span>` : "";
  const contactId = escapeHtml(normalizePeopleText(contact?.contact_id, ""));
  const route = normalizePeopleText(contact?.route, "");
  const chatAction = contact?.can_message === true && route
    ? `<button class="home-people-action" type="button" data-people-action="chat" data-contact-route="${escapeHtml(route)}">Chat</button>`
    : "";
  return `
    <article class="home-people-card">
      <div class="home-people-avatar" aria-hidden="true">${escapeHtml(rawDisplayName.slice(0, 1).toUpperCase() || "E")}</div>
      <div class="home-people-card-copy">
        <h3>${displayName}</h3>
        <p><span>${relationship}</span>${handleLine}${deviceLine}</p>
      </div>
      <div class="home-people-card-actions">
        ${chatAction}
        <button class="home-people-action home-people-action-danger" type="button" data-people-action="remove" data-contact-id="${contactId}">Remove</button>
      </div>
    </article>
  `;
}

function bindPeopleWindowActions(body) {
  body.querySelector("[data-people-profile-form]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    savePeopleProfile(body, event.currentTarget).catch((error) => {
      setPeopleStatus(body, error.message || "Could not save profile.", "error");
    });
  });
  for (const button of body.querySelectorAll("[data-people-jump]")) {
    button.addEventListener("click", (event) => {
      const sectionId = event.currentTarget?.dataset?.peopleJump || "";
      const section = body.querySelector(`[data-people-section="${sectionId}"]`);
      if (section) {
        section.scrollIntoView({ block: "start", behavior: "smooth" });
      }
      for (const item of body.querySelectorAll("[data-people-jump]")) {
        item.classList.toggle("active", item === event.currentTarget);
      }
    });
  }
  body.querySelector("[data-people-action='toggle-discovery']")?.addEventListener("click", (event) => {
    togglePeopleDiscovery(body, event.currentTarget).catch((error) => {
      setPeopleStatus(body, error.message || "Could not update discovery.", "error");
    });
  });
  body.querySelector("[data-people-action='refresh-discovery']")?.addEventListener("click", () => {
    refreshPeopleDiscovery(body).catch((error) => {
      setPeopleStatus(body, error.message || "Could not refresh discovery.", "error");
    });
  });
  for (const button of body.querySelectorAll("[data-people-action='request-peer']")) {
    button.addEventListener("click", (event) => {
      requestPeopleDiscoveryPeer(body, event.currentTarget).catch((error) => {
        setPeopleStatus(body, error.message || "Could not request this person.", "error");
      });
    });
  }
  for (const button of body.querySelectorAll("[data-people-action='accept-request']")) {
    button.addEventListener("click", (event) => {
      acceptPeopleDiscoveryRequest(body, event.currentTarget).catch((error) => {
        setPeopleStatus(body, error.message || "Could not accept this request.", "error");
      });
    });
  }
  for (const button of body.querySelectorAll("[data-people-action='chat']")) {
    button.addEventListener("click", (event) => {
      try {
        openPersonChat(body, event.currentTarget);
      } catch (error) {
        setPeopleStatus(body, error.message || "Could not open chat.", "error");
      }
    });
  }
  for (const button of body.querySelectorAll("[data-people-action='remove']")) {
    button.addEventListener("click", (event) => {
      removePersonFromPeople(body, event.currentTarget).catch((error) => {
        setPeopleStatus(body, error.message || "Could not remove person.", "error");
      });
    });
  }
}

function peopleDiscoveryRequestIsVisible(request) {
  const status = normalizePeopleText(request?.status, "requested");
  return status === "incoming" || status === "requested";
}

function filterDiscoveredPeople(peers, contacts) {
  const existingPeerIds = new Set();
  for (const contact of contacts) {
    const deviceLabel = normalizePeopleText(contact?.device_label, "");
    if (deviceLabel) {
      existingPeerIds.add(deviceLabel);
    }
    const route = normalizePeopleText(contact?.route, "");
    if (route.startsWith("elastos://peer/")) {
      const peerId = route.slice("elastos://peer/".length).trim();
      if (peerId) {
        existingPeerIds.add(peerId);
      }
    }
  }
  return peers.filter((peer) => {
    const peerId = normalizePeopleText(peer?.peer_id, "");
    return !peerId || !existingPeerIds.has(peerId);
  });
}

function peopleDisplayName(person, fallback) {
  const profileCard = person?.profile_card && typeof person.profile_card === "object" ? person.profile_card : {};
  const displayName =
    normalizePeopleText(profileCard.display_name, "") ||
    normalizePeopleText(person?.display_name, "");
  const handle =
    normalizePeopleText(profileCard.handle, "") ||
    normalizePeopleText(person?.handle, "");
  const peer = normalizePeopleText(person?.device_label || person?.peer_id, "");
  if (displayName && displayName !== "ElastOS user") {
    return displayName;
  }
  return handle || peer || fallback;
}

function peopleProfileDisplayName(identity) {
  const profileCard = identity?.profile_card && typeof identity.profile_card === "object" ? identity.profile_card : {};
  return normalizePeopleText(profileCard.display_name, "") || normalizePeopleText(identity?.handle, "");
}

async function savePeopleProfile(body, form) {
  const input = form?.querySelector("[data-people-profile-input]");
  const button = form?.querySelector("[data-people-profile-save]");
  if (!(input instanceof HTMLInputElement)) {
    throw new Error("Profile name field is missing.");
  }
  const handle = input.value.trim();
  input.disabled = true;
  setPeopleBusy(button, true);
  try {
    await fetchJson("/api/apps/people/profile-card", {
      method: "POST",
      body: JSON.stringify({ handle }),
    });
    setPeopleStatus(body, "Profile saved.", "ok");
    await shellState.requestSummaryRefresh?.();
  } finally {
    input.disabled = false;
    setPeopleBusy(button, false);
  }
}

async function togglePeopleDiscovery(body, button) {
  const enabled = button?.dataset?.discoveryEnabled === "true";
  setPeopleBusy(button, true);
  try {
    const discovery = await fetchJson("/api/apps/people/discovery", {
      method: "POST",
      body: JSON.stringify({ enabled }),
    });
    const remainingText = peopleDiscoveryRemainingText(peopleDiscoveryRemainingSeconds(discovery));
    setPeopleStatus(body, enabled ? `Discoverable for ${remainingText}.` : "Discovery is off.", "ok");
    if (!enabled) {
      cleanupPeopleDiscoveryAutoRefresh(body);
    }
    await shellState.requestSummaryRefresh?.();
  } finally {
    setPeopleBusy(button, false);
  }
}

async function refreshPeopleDiscovery(body, options = {}) {
  const discovery = await fetchJson("/api/apps/people/discovery/refresh", { method: "POST" });
  if (!options.silent) {
    setPeopleStatus(body, "Discovery refreshed.", "ok");
  }
  if (options.updateSummary !== false) {
    await shellState.requestSummaryRefresh?.();
  }
  return discovery;
}

function schedulePeopleDiscoveryAutoRefresh(body, discovery) {
  if (!body || discovery.enabled !== true) {
    cleanupPeopleDiscoveryAutoRefresh(body);
    return;
  }
  let state = peopleDiscoveryRefreshTimers.get(body);
  if (state) {
    state.lastFingerprint = peopleDiscoveryFingerprint(discovery) || state.lastFingerprint;
    if (!state.timer) {
      queuePeopleDiscoveryAutoRefresh(body, state, PEOPLE_DISCOVERY_AUTO_REFRESH_INITIAL_MS);
    }
    return;
  }
  state = {
    inFlight: false,
    timer: 0,
    emptyTicks: 0,
    lastFingerprint: peopleDiscoveryFingerprint(discovery),
  };
  peopleDiscoveryRefreshTimers.set(body, state);
  queuePeopleDiscoveryAutoRefresh(body, state, PEOPLE_DISCOVERY_AUTO_REFRESH_INITIAL_MS);
}

function queuePeopleDiscoveryAutoRefresh(body, state, delayMs) {
  window.clearTimeout(state.timer);
  state.timer = window.setTimeout(() => {
    runPeopleDiscoveryAutoRefresh(body, state);
  }, clampPeopleDiscoveryRefreshDelay(delayMs));
}

async function runPeopleDiscoveryAutoRefresh(body, state) {
  state.timer = 0;
  if (!body.isConnected) {
    cleanupPeopleDiscoveryAutoRefresh(body);
    return;
  }
  const windowNode = body.closest(".window");
  if (windowNode?.classList.contains("hidden")) {
    queuePeopleDiscoveryAutoRefresh(body, state, PEOPLE_DISCOVERY_AUTO_REFRESH_STABLE_MS);
    return;
  }
  if (state.inFlight) {
    queuePeopleDiscoveryAutoRefresh(body, state, PEOPLE_DISCOVERY_AUTO_REFRESH_FAST_MS);
    return;
  }
  state.inFlight = true;
  try {
    const discovery = await refreshPeopleDiscovery(body, { silent: true, updateSummary: false });
    const fingerprint = peopleDiscoveryFingerprint(discovery);
    const changed = discovery?.changed === true || (fingerprint && state.lastFingerprint && fingerprint !== state.lastFingerprint);
    state.lastFingerprint = fingerprint || state.lastFingerprint;
    if (changed || discovery?.enabled === false) {
      await shellState.requestSummaryRefresh?.();
    } else {
      updatePeopleDiscoveryCountdown(body, discovery);
    }
    if (discovery?.enabled === true) {
      queuePeopleDiscoveryAutoRefresh(body, state, peopleDiscoveryNextAutoRefreshDelay(discovery, state, changed));
    } else {
      cleanupPeopleDiscoveryAutoRefresh(body);
    }
  } catch {
    // Manual Refresh still reports errors; background discovery should stay quiet.
    queuePeopleDiscoveryAutoRefresh(body, state, PEOPLE_DISCOVERY_AUTO_REFRESH_STABLE_MS);
  } finally {
    state.inFlight = false;
  }
}

function peopleDiscoveryNextAutoRefreshDelay(discovery, state, changed) {
  if (changed) {
    state.emptyTicks = 0;
    return PEOPLE_DISCOVERY_AUTO_REFRESH_FAST_MS;
  }
  const hasVisibleWork = Number(discovery?.discovered_count || 0) > 0 || Number(discovery?.request_count || 0) > 0;
  if (hasVisibleWork) {
    state.emptyTicks = 0;
    return Math.max(Number(discovery?.next_refresh_after_ms || 0), PEOPLE_DISCOVERY_AUTO_REFRESH_IDLE_MS);
  }
  state.emptyTicks += 1;
  if (state.emptyTicks <= 3) {
    return PEOPLE_DISCOVERY_AUTO_REFRESH_FAST_MS;
  }
  return Math.max(Number(discovery?.next_refresh_after_ms || 0), PEOPLE_DISCOVERY_AUTO_REFRESH_STABLE_MS);
}

function peopleDiscoveryFingerprint(discovery) {
  if (typeof discovery?.refresh_fingerprint === "string" && discovery.refresh_fingerprint.trim() !== "") {
    return discovery.refresh_fingerprint;
  }
  const peers = Array.isArray(discovery?.discovered_peers)
    ? discovery.discovered_peers.map((peer) => `${peer.peer_id || ""}:${peer.last_seen_at || 0}:${peer.status || ""}`).sort()
    : [];
  const requests = Array.isArray(discovery?.requests)
    ? discovery.requests.map((request) => `${request.request_id || ""}:${request.status || ""}:${request.created_at || 0}`).sort()
    : [];
  return JSON.stringify({
    enabled: discovery?.enabled === true,
    status: discovery?.status || "",
    local_peer_id: discovery?.local_peer_id || "",
    peers,
    requests,
  });
}

function clampPeopleDiscoveryRefreshDelay(value) {
  const delay = Number(value || 0);
  if (!Number.isFinite(delay) || delay <= 0) {
    return PEOPLE_DISCOVERY_AUTO_REFRESH_STABLE_MS;
  }
  return Math.min(Math.max(delay, PEOPLE_DISCOVERY_AUTO_REFRESH_INITIAL_MS), PEOPLE_DISCOVERY_AUTO_REFRESH_MAX_MS);
}

function updatePeopleDiscoveryCountdown(body, discovery) {
  const countdown = body.querySelector("[data-people-discovery-countdown]");
  if (!countdown) {
    return;
  }
  const remainingSeconds = peopleDiscoveryRemainingSeconds(discovery);
  countdown.textContent = `Discoverable for ${peopleDiscoveryRemainingText(remainingSeconds)}`;
}

function cleanupPeopleDiscoveryAutoRefresh(nodeOrBody) {
  const body = nodeOrBody?.classList?.contains("window-body")
    ? nodeOrBody
    : nodeOrBody?.querySelector?.(".window-body");
  if (!body) {
    return;
  }
  const state = peopleDiscoveryRefreshTimers.get(body);
  if (!state) {
    return;
  }
  window.clearTimeout(state.timer);
  peopleDiscoveryRefreshTimers.delete(body);
}

async function requestPeopleDiscoveryPeer(body, button) {
  const peerId = typeof button?.dataset?.peerId === "string" ? button.dataset.peerId : "";
  if (!peerId) {
    throw new Error("Discovery peer id is missing.");
  }
  setPeopleBusy(button, true);
  try {
    await fetchJson("/api/apps/people/discovery/requests", {
      method: "POST",
      body: JSON.stringify({ peer_id: peerId }),
    });
    setPeopleStatus(body, "Request sent.", "ok");
    await shellState.requestSummaryRefresh?.();
  } finally {
    setPeopleBusy(button, false);
  }
}

async function acceptPeopleDiscoveryRequest(body, button) {
  const requestId = typeof button?.dataset?.requestId === "string" ? button.dataset.requestId : "";
  if (!requestId) {
    throw new Error("Discovery request id is missing.");
  }
  setPeopleBusy(button, true);
  try {
    await fetchJson(`/api/apps/people/discovery/requests/${encodeURIComponent(requestId)}/accept`, {
      method: "POST",
    });
    setPeopleStatus(body, "Request accepted. This person is now in People.", "ok");
    await shellState.requestSummaryRefresh?.();
  } finally {
    setPeopleBusy(button, false);
  }
}

async function removePersonFromPeople(body, button) {
  const contactId = typeof button?.dataset?.contactId === "string" ? button.dataset.contactId : "";
  if (!contactId) {
    throw new Error("Person id is missing.");
  }
  const card = button.closest(".home-people-card");
  const label = card?.querySelector(".home-people-card-copy h3")?.textContent?.trim() || "this person";
  if (!window.confirm(`Remove ${label} from People?`)) {
    return;
  }
  setPeopleBusy(button, true);
  try {
    await fetchJson("/api/apps/people/contacts/remove", {
      method: "POST",
      body: JSON.stringify({ contact_id: contactId }),
    });
    setPeopleStatus(body, "Removed from People.", "ok");
    await shellState.requestSummaryRefresh?.();
  } finally {
    setPeopleBusy(button, false);
  }
}

function openPersonChat(body, button) {
  const route = normalizePeopleText(button?.dataset?.contactRoute, "");
  const targetId = homeTargetFromRoute(route);
  if (targetId !== "chat-room") {
    throw new Error("Chat is not available for this person yet.");
  }
  setPeopleStatus(body, "Opening chat.", "ok");
  openTarget(targetId);
}

function homeTargetFromRoute(route) {
  if (!route) {
    return "";
  }
  try {
    const url = new URL(route, window.location.origin);
    const match = url.pathname.match(/^\/apps\/([^/]+)\/?$/);
    return match ? decodeURIComponent(match[1]) : "";
  } catch (_error) {
    return "";
  }
}

function setPeopleBusy(button, busy) {
  if (button instanceof HTMLButtonElement) {
    button.disabled = busy;
  }
}

function setPeopleStatus(body, text, tone = "muted") {
  const node = body.querySelector(".home-people-status");
  if (!node) {
    return;
  }
  node.textContent = text;
  node.dataset.tone = tone;
  node.hidden = !text;
}

function normalizePeopleText(value, fallback) {
  return typeof value === "string" && value.trim() ? value.trim() : fallback;
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
  return `browser:${Date.now()}:${Math.random().toString(16).slice(2)}`;
}

export function showDesktopHome() {
  requireWindowHooks().hideLauncher();
  hideWindowSnapPreview();
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
  const launched = await fetchJson("/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify({
      target: targetId,
      query: launchQuery,
    }),
  });
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
  });
  armWindowControlGuard(node, { closeMs: WINDOW_OPEN_CLOSE_GHOST_GUARD_MS });
  node.dataset.target = launched.target;
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
  const entry = {
    id: windowId,
    targetId: launched.target,
    serial: shellState.browserWindowSerial,
    node,
    kind: "browser",
    title: launched.title,
    launchQuery,
  };
  shellState.windows.set(windowId, entry);
  syncBrowserWindow(entry, launched);
  if (entry.targetId === "browser") {
    fitLaunchedWindow(entry);
  }
  if (restoredPlacement?.hidden) {
    entry.node.classList.add("hidden");
    entry.node.classList.remove("window-active");
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

function launchDidFail(launched) {
  return (
    typeof launched.launch_status === "string" &&
    launched.launch_status.trim() !== "" &&
    launched.launch_status !== "launched"
  );
}

function syncBrowserWindow(entry, launched) {
  const node = entry.node;
  const frame = node.querySelector(".window-frame");
  node.querySelector(".window-head-title").textContent = launched.title;
  node.setAttribute("aria-label", launched.title);
  cleanupFrameAutoFit(node);

  const syncLoadedFrame = () => {
    if (entry.targetId !== "browser") {
      installFrameAutoFit(node, frame);
    }
    fitLaunchedWindow(entry);
  };

  frame.onload = syncLoadedFrame;
  if (frame.dataset.route !== launched.route) {
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
    return;
  }
  removeWindowEntries([entry]);
}

function focusTopVisibleWindow() {
  const visible = sortWindowEntriesByZOrder(
    Array.from(shellState.windows.values()).filter(
      (entry) => !entry.node.classList.contains("hidden"),
    ),
  );

  if (visible.length === 0) {
    shellState.activeWindowId = null;
    refreshWindowUi();
    persistBrowserSession();
    return;
  }

  focusWindow(visible[0].id);
}

function browserWindowSpec(launched, offset) {
  if (launched.target === SYSTEM_APP_ID) {
    return {
      x: 36,
      y: 44,
      width: 980,
      height: 620,
    };
  }
  if (typeof launched.route === "string" && launched.route.startsWith("/apps/gba-emulator/")) {
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

function peopleWindowSpec(offset) {
  return {
    x: 72 + offset * 18,
    y: 72 + offset * 18,
    width: 680,
    height: 520,
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
  entry.node.classList.add("window-active");
  entry.node.setAttribute("aria-hidden", "false");
  shellState.zIndexCounter += 1;
  entry.node.style.zIndex = String(shellState.zIndexCounter);
  shellState.activeWindowId = id;
  if (entry.kind === "browser") {
    rememberRecentTarget(entry.targetId);
    fitLaunchedWindow(entry);
  }
  refreshWindowUi();
  persistBrowserSession();
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
  const restoredWindows = normalizeRestorableSession(
    shellState.currentSummary,
    loadShellSessionState(),
    { rootShell: currentRootShellSessionId() },
  );
  if (restoredWindows.length === 0) {
    clearShellSessionState();
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
      const entry = restoredWindow.target === PEOPLE_TARGET_ID
        ? openPeopleWindow({ restoredPlacement: restoredWindow })
        : await launchBrowserTargetWindow(restoredWindow.target, {
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

  if (restoredEntries.length === 0) {
    clearShellSessionState();
    return;
  }

  const activeEntry = restoredEntries.find(
    ({ restoredWindow }) => restoredWindow.active && !restoredWindow.hidden,
  );
  if (activeEntry) {
    focusWindow(activeEntry.entry.id);
    return;
  }
  focusTopVisibleWindow();
}

export function cleanupBeforeUnload() {
  persistBrowserSession();
  if (shellState.clockTimer !== null) {
    window.clearInterval(shellState.clockTimer);
  }
  for (const entry of shellState.windows.values()) {
    cleanupFrameAutoFit(entry.node);
    cleanupPeopleDiscoveryAutoRefresh(entry.node);
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
