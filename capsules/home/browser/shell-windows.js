import {
  desktop,
  windowTemplate,
  windowErrorTemplate,
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
} from "./shell-core.js?v=home-20260615b";
import {
  fitWindowBounds,
  fitWindowToBrowserAspect,
  applyWindowPlacement,
  rememberWindowRestoreBounds,
  restoreWindowFromSpecialState,
  hideWindowSnapPreview,
  attachWindowDrag,
  attachWindowResize,
} from "./shell-window-geometry.js?v=home-20260615b";

let windowHooks = null;
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
const MAX_SESSION_WINDOWS = 24;
const SINGLE_SESSION_TARGETS = new Set(["browser"]);
const COMMON_IFRAME_SANDBOX = [
  "allow-downloads",
  "allow-forms",
  "allow-modals",
  "allow-pointer-lock",
  "allow-same-origin",
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
const BROWSER_IFRAME_ALLOW_EXTRAS = ["clipboard-read", "clipboard-write"];
const WEBAUTHN_IFRAME_ALLOW_TARGETS = new Set(["wallet"]);

function iframeSandboxForLaunch(launched) {
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
  return sortWindowEntriesByZOrder(browserWindowEntries())
    .reverse()
    .slice(0, MAX_SESSION_WINDOWS)
    .map((entry) => {
      const bounds = currentWindowBounds(entry.node);
      const restoreBounds = currentWindowRestoreBounds(entry.node);
      return {
        target: entry.targetId,
        hidden: entry.node.classList.contains("hidden"),
        active: shellState.activeWindowId === entry.id,
        maximized: entry.node.dataset.maximized === "true",
        snap: entry.node.dataset.snap || "",
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        restoreX: restoreBounds.x,
        restoreY: restoreBounds.y,
        restoreWidth: restoreBounds.width,
        restoreHeight: restoreBounds.height,
      };
    });
}

function persistBrowserSession() {
  if (shellState.restoringSession) {
    return;
  }
  const windows = persistedBrowserSessionEntries();
  if (windows.length === 0) {
    saveShellSessionState({ windows: [] });
    return;
  }
  saveShellSessionState({ windows });
}

function normalizeRestorableSession(summary, storedSession) {
  const storedWindows = Array.isArray(storedSession?.windows) ? storedSession.windows : [];
  const seenTargets = new Set();
  const normalized = [];
  for (const item of storedWindows) {
    const targetId = typeof item?.target === "string" ? item.target : "";
    if (!targetId || seenTargets.has(targetId) || !targetById(summary, targetId)) {
      continue;
    }
    seenTargets.add(targetId);
    normalized.push({
      target: targetId,
      hidden: item?.hidden === true,
      active: item?.active === true,
      maximized: item?.maximized === true,
      snap: typeof item?.snap === "string" ? item.snap : "",
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

function renderWindowTaskbar() {
  if (!shellState.currentSummary) {
    return;
  }
  requireWindowHooks().renderTaskbar(shellState.currentSummary);
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
    releaseFrameRuntimePage(entry.node);
    cleanupFrameAutoFit(entry.node);
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

export function renderBootError(error) {
  requireWindowHooks().clearIdentitySurface();
  renderSystemErrorWindow({
    id: "shell-error",
    title: "Home",
    headline: "Runtime data not attached on this host",
    copy: "Home could not attach runtime-backed summary data here. Static surface facts are still available below.",
    subjectLabel: "Surface",
    subjectValue: "Home",
    detail: String(error.message || error),
  });
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
    desktop.appendChild(node);
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

function createWindow({ id, title, x, y, width, height, tone }) {
  const bounds = fitWindowBounds({ x, y, width, height });
  const node = windowTemplate.content.firstElementChild.cloneNode(true);
  node.dataset.windowId = id;
  node.dataset.maximized = "false";
  node.dataset.snap = "";
  node.setAttribute("aria-label", title);
  node.setAttribute("aria-hidden", "false");
  armWindowControlGuard(node);
  node.style.left = `${bounds.x}px`;
  node.style.top = `${bounds.y}px`;
  node.style.width = `${bounds.width}px`;
  node.style.height = `${bounds.height}px`;
  node.querySelector(".window-head-title").textContent = title;
  mountGlyph(node.querySelector(".window-head-icon"), id, tone);

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
  // The dDRM viewers open in one of two modes:
  //   • bound to a REAL owned object (Library passes objectUri/uri) — the gateway
  //     seals THAT file through the local key-authority and picks the viewer itself;
  //   • standalone from the launcher (no object) — a sample asset demo.
  // Either way the CEK stays in the decrypt boundary; the browser only ever sees
  // already-decrypted bytes.
  if (targetId === "elacity-player" || targetId === "ddrm-viewer") {
    const ownedUri = libraryUriFromQuery(options.query);
    const openKey = launchActionKey(targetId, options.query);
    // A protected open can take several seconds (wallet sign + 2-of-3 geo-quorum recover +
    // decrypt). The 350ms repeat-guard is far too short for that, so a second double-click
    // would start a DUPLICATE recover. If one is already in flight, just focus its window.
    const inflightId = inFlightOwnedOpens.get(openKey);
    if (inflightId && shellState.windows.has(inflightId)) {
      focusWindow(inflightId);
      return;
    }
    const loading = openLoadingWindow(
      targetId,
      "Opening protected asset…",
      "Verifying your on-chain access and recovering keys from the dKMS quorum…",
      OWNED_OPEN_STAGES,
    );
    inFlightOwnedOpens.set(openKey, loading.id);
    const launch = ownedUri
      ? () => launchOwnedFromLibrary(ownedUri, options, loading)
      : targetId === "elacity-player"
        ? () => launchOwnedMediaWindow(options, loading)
        : () => launchOwnedObjectWindow(options, loading);
    launch()
      .catch((error) => {
        const status = Number(error && error.status);
        if (status === 401 || status === 403) {
          closeWindow(loading.id);
          requireWindowHooks().requestHomeUnlock?.();
          return;
        }
        console.error("failed to open owned asset", error);
        renderLoadingWindowError(loading, error);
      })
      .finally(() => {
        inFlightOwnedOpens.delete(openKey);
      });
    return;
  }
  if (SINGLE_SESSION_TARGETS.has(targetId) && browserWindowCount(targetId) > 0) {
    activateTargetGroup(targetId);
    return;
  }
  const launchOptions = targetId === "browser"
    ? withBrowserInstanceQuery(options)
    : options;
  if (ignoreRepeatedAction(launchActionKey(targetId, launchOptions.query))) {
    return;
  }
  launchBrowserTargetWindow(targetId, launchOptions).catch((error) => {
    const status = Number(error && error.status);
    if (status === 401 || status === 403) {
      requireWindowHooks().requestHomeUnlock?.();
      return;
    }
    console.error(`failed to open ${targetId}`, error);
    renderTargetLaunchError(targetId, error);
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
  const launched = await fetchJson("/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify({
      target: targetId,
      query: normalizedLaunchQuery(options.query),
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

  return openLaunchedWindow(launched, options);
}

// Owned/protected opens in flight, keyed by launch action. A dKMS open takes seconds
// (wallet sign + 2-of-3 geo-quorum recover + decrypt), so a second double-click would
// otherwise kick off a DUPLICATE recover. We track the loading window and re-focus it.
const inFlightOwnedOpens = new Map();

// The ordered phases a protected (dKMS quorum) open passes through, shown as a live checklist
// so the user sees WHERE the open is. These are the genuine, sequential phases the client drives
// + the gateway runs; the quorum recover (phase 1) is the long, variable pole (cold geo round-trip).
const OWNED_OPEN_STAGES = [
  "Preparing secure session",
  "Verifying access & recovering keys (2-of-3 quorum)",
  "Decrypting & preparing playback",
];

function loadingStagesHtml(stages) {
  return `<ul class="window-loading-stages" aria-hidden="false">${stages
    .map(
      (label, i) =>
        `<li data-stage="${i}" class="${i === 0 ? "is-active" : "is-pending"}">` +
        `<span class="stage-mark" aria-hidden="true"></span>` +
        `<span class="stage-label">${escapeHtml(label)}</span></li>`,
    )
    .join("")}</ul>`;
}

function loadingBodyHtml(title, detail, stages) {
  if (Array.isArray(stages) && stages.length) {
    return `
    <div class="window-loading" role="status" aria-live="polite">
      <div class="window-loading-title">${escapeHtml(title || "Opening…")}</div>
      ${loadingStagesHtml(stages)}
    </div>
  `;
  }
  return `
    <div class="window-loading" role="status" aria-live="polite">
      <div class="window-loading-spinner" aria-hidden="true"></div>
      <div class="window-loading-title">${escapeHtml(title || "Opening…")}</div>
      <div class="window-loading-detail">${escapeHtml(detail || "")}</div>
    </div>
  `;
}

// Advance the staged checklist: every phase before `activeIndex` is done (✓), `activeIndex` is the
// live one (spinner), the rest stay pending. No-op for windows without stages (legacy spinner).
function setLoadingStage(entry, activeIndex) {
  if (!entry || !entry.node) return;
  const items = entry.node.querySelectorAll(".window-loading-stages li");
  items.forEach((li, i) => {
    li.classList.toggle("is-done", i < activeIndex);
    li.classList.toggle("is-active", i === activeIndex);
    li.classList.toggle("is-pending", i > activeIndex);
  });
}


// Open a window with an immediate loading state. The real iframe is swapped in by
// `navigateLoadingWindow` once the (slow) open resolves; failures are surfaced in-place
// by `renderLoadingWindowError` instead of leaving a dead spinner.
function openLoadingWindow(targetId, title, detail, stages) {
  const offset = browserWindowEntries().length;
  const windowSpec = browserWindowSpec({ target: targetId }, offset);
  const windowId = nextBrowserWindowId(targetId);
  const node = createWindow({
    id: windowId,
    title,
    x: windowSpec.x,
    y: windowSpec.y,
    width: windowSpec.width,
    height: windowSpec.height,
    tone: glyphTone(targetId),
  });
  armWindowControlGuard(node, { closeMs: WINDOW_OPEN_CLOSE_GHOST_GUARD_MS });
  node.dataset.target = targetId;
  const body = node.querySelector(".window-body");
  body.classList.add("window-body-frame");
  body.innerHTML = loadingBodyHtml(title, detail, stages);

  desktop.appendChild(node);
  const entry = {
    id: windowId,
    targetId,
    serial: shellState.browserWindowSerial,
    node,
    kind: "browser",
    title,
    loading: true,
  };
  shellState.windows.set(windowId, entry);
  renderWindowTaskbar();
  focusWindow(windowId);
  return entry;
}

// Swap a loading window over to the resolved iframe route.
function navigateLoadingWindow(entry, launched) {
  entry.loading = false;
  entry.title = launched.title;
  const node = entry.node;
  node.querySelector(".window-head-title").textContent = launched.title;
  node.setAttribute("aria-label", launched.title);
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
  syncBrowserWindow(entry, launched);
  renderWindowTaskbar();
}

// Surface a failed slow-open INSIDE its loading window so the user sees why playback
// didn't start (rather than a dead spinner or a window that silently never appears).
function renderLoadingWindowError(entry, error) {
  if (!entry || !shellState.windows.has(entry.id)) {
    return;
  }
  entry.loading = false;
  const body = entry.node.querySelector(".window-body");
  if (!body) {
    return;
  }
  const detail = String((error && error.message) || error || "The open did not complete.");
  body.innerHTML = `
    <div class="window-loading is-error" role="alert">
      <div class="window-loading-title">Couldn’t open this asset</div>
      <div class="window-loading-detail">${escapeHtml(detail)}</div>
    </div>
  `;
}

// Either navigate an already-open loading window to the resolved route, or open a fresh
// window when there's no loading window (keeps non-owned callers working unchanged).
function placeLaunched(launched, options, loading) {
  if (loading && shellState.windows.has(loading.id)) {
    navigateLoadingWindow(loading, launched);
    return loading;
  }
  return openLaunchedWindow(launched, options);
}

// Open a window for an already-resolved launch descriptor (route + title +
// attach_kind). Shared by the generic Home launch and the owned-media launch.
function openLaunchedWindow(launched, options = {}) {
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

  desktop.appendChild(node);
  if (restoredPlacement) {
    applyWindowPlacement(node, restoredPlacement);
  }
  const entry = {
    id: windowId,
    targetId: launched.target,
    serial: shellState.browserWindowSerial,
    node,
    kind: "browser",
    title: launched.title,
  };
  shellState.windows.set(windowId, entry);
  syncBrowserWindow(entry, launched);
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

// Owned-media: ask the gateway to stand up a decrypt session through the local
// key-authority + a SEPARATE decrypt-provider boundary, then open elacity-player
// at the returned play URL (session + scoped launch token baked in). The CEK
// never reaches the browser — only already-decrypted segment bytes are loaded.
async function launchOwnedMediaWindow(options = {}, loading = null) {
  // Single server call does the recover + decrypt, so jump the checklist to the recover phase.
  setLoadingStage(loading, 1);
  const opened = await fetchJson("/api/viewers/elacity-player/media/open", {
    method: "POST",
  });
  if (typeof opened.play_url !== "string" || opened.play_url === "") {
    throw new Error("media open did not return a play URL");
  }
  setLoadingStage(loading, 2);
  const launched = {
    target: "elacity-player",
    title: "Owned video",
    route: opened.play_url,
    attach_kind: "iframe",
    launch_status: "launched",
  };
  return placeLaunched(launched, options, loading);
}

// The Library object URI an open carries, if any. Library hands us `objectUri`
// (preferred) or `uri` when a user opens an item with one of the dDRM viewers.
function libraryUriFromQuery(query) {
  const q = normalizedLaunchQuery(query);
  const uri = q.objectUri || q.uri || "";
  return typeof uri === "string" && uri.trim() ? uri.trim() : null;
}

// Owned object bound to a REAL Library file: ask the gateway to seal THAT object
// through the local key-authority + a SEPARATE decrypt-provider boundary. The gateway
// resolves the URI inside the principal's own root (ownership gate), reads the
// plaintext, picks the viewer by content type, and returns { viewer, play_url }. The
// CEK never reaches the browser — only already-decrypted bytes are loaded.
async function launchOwnedFromLibrary(uri, options = {}, loading = null) {
  let opened;
  try {
    opened = await openOwnedRequest(uri, loading);
  } catch (error) {
    // A rights-denied open (no access token yet) is recoverable: buy the access token,
    // then retry the open ONCE. Auth failures (no wallet / locked) are not retried here.
    if (isRightsDeniedError(error)) {
      await fetchJson("/api/market/buy", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ uri }),
      });
      opened = await openOwnedRequest(uri, loading);
    } else {
      throw error;
    }
  }
  if (typeof opened.play_url !== "string" || opened.play_url === "") {
    throw new Error("owned open did not return a view URL");
  }
  // Keys recovered + decrypted server-side; the window is about to swap to the player.
  setLoadingStage(loading, 2);
  const target = typeof opened.viewer === "string" && opened.viewer ? opened.viewer : "ddrm-viewer";
  const launched = {
    target,
    title: typeof opened.title === "string" && opened.title ? opened.title : "Owned asset",
    route: opened.play_url,
    attach_kind: "iframe",
    launch_status: "launched",
  };
  return placeLaunched(launched, options, loading);
}

// TRUSTLESS open: a protected dKMS asset is opened by handing the quorum a WALLET-SIGNED grant the
// nodes verify themselves (no server-side trust). Phase 1 asks the gateway to bind a fresh session
// key to (this asset's on-chain contentId, this quorum's node-set, the user's wallet) and return
// the canonical delegation; the user signs it ONCE with their wallet (EIP-191 personal_sign);
// phase 2 submits the signature so the gateway assembles + forwards the grant.
//
// Falls back to the plain open (legacy enrolled-caller path) when the asset is not a quorum capsule
// (prepare-grant 400) or no injected wallet is available — so non-protected opens are unchanged.
async function openOwnedRequest(uri, loading = null) {
  setLoadingStage(loading, 0);
  let prepared = null;
  try {
    prepared = await fetchJson("/api/viewers/prepare-grant", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ uri }),
    });
  } catch (error) {
    // 400 => not a dKMS quorum capsule (no wallet grant needed). Anything else (e.g. 403 no wallet)
    // we surface by falling through to the plain open, which returns the precise gateway error.
    if (Number(error && error.status) !== 400) {
      // A non-400 prepare failure still lets the legacy open produce the authoritative error.
    }
    prepared = null;
  }

  const body = { uri };
  if (prepared && prepared.already_delegated) {
    // PC2 secure-view session: the wallet already signed a delegation for this asset earlier in the
    // window — open with just { uri } and the gateway assembles a fresh grant from the cached
    // delegation (no MetaMask popup). The live on-chain check still gates this open.
  } else if (prepared && typeof prepared.delegation_canonical === "string" && prepared.grant_handle) {
    const sig = await walletPersonalSign(prepared.delegation_canonical, prepared.owner_address);
    if (sig) {
      body.grant_handle = prepared.grant_handle;
      body.delegation_sig_hex = sig;
    }
    // If signing was declined / unavailable, fall through with just { uri } — the node then decides
    // via the legacy path (enrolled caller), or fails closed if it has no allow-list.
  }

  // Session ready (delegation prepared/signed or reused) — the open call now runs the live on-chain
  // check + the 2-of-3 quorum recover, the long pole. Advance the checklist to that phase.
  setLoadingStage(loading, 1);
  return fetchJson("/api/viewers/open", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

// Ask the user's injected EVM wallet (MetaMask, etc.) to EIP-191 `personal_sign` the canonical
// delegation. Ensures the signing account matches the delegation owner the gateway bound (so the
// node recovers the expected address). Returns the 0x-hex signature, or null if no wallet / declined
// (the caller then falls back to the legacy path).
async function walletPersonalSign(canonical, ownerAddress) {
  const eth = typeof window !== "undefined" ? window.ethereum : null;
  if (!eth || typeof eth.request !== "function") {
    return null;
  }
  try {
    const accounts = await eth.request({ method: "eth_requestAccounts" });
    const want = String(ownerAddress || "").toLowerCase();
    let from = Array.isArray(accounts) && accounts.length ? String(accounts[0]) : "";
    if (want) {
      const match = (accounts || []).find((a) => String(a).toLowerCase() === want);
      if (match) {
        from = match;
      } else {
        throw new Error(
          "the connected wallet (" + from + ") is not the account linked to this content (" + ownerAddress + ")"
        );
      }
    }
    // personal_sign params are [message, address]; the wallet applies the EIP-191 prefix the gateway
    // and the dKMS nodes recompute. We sign the canonical UTF-8 string verbatim.
    const sig = await eth.request({ method: "personal_sign", params: [canonical, from] });
    return typeof sig === "string" && sig ? sig : null;
  } catch (error) {
    // A user rejection or a missing wallet is not fatal here — surface it for visibility and let the
    // open fall back. A wrong-account error is rethrown so the user sees why it cannot proceed.
    if (error && /not the account linked/.test(String(error.message || ""))) {
      throw error;
    }
    console.warn("wallet personal_sign unavailable or declined:", error);
    return null;
  }
}

// A 403 whose body is the rights-provider's denial (no access token yet) — distinct
// from an auth/lock 403, which we leave to the unlock prompt. The buy-and-retry loop
// only triggers on the former.
function isRightsDeniedError(error) {
  const status = Number(error && error.status);
  if (status !== 403) {
    return false;
  }
  const message = String((error && error.message) || "");
  return message.includes("rights provider denied") || message.includes("no valid access token");
}

// Owned non-media: ask the gateway to stand up an OBJECT decrypt session through the
// local key-authority + a SEPARATE decrypt-provider boundary, then open ddrm-viewer
// at the returned view URL (session + scoped launch token baked in). The CEK never
// reaches the browser — only the already-decrypted object bytes are loaded.
async function launchOwnedObjectWindow(options = {}, loading = null) {
  // Single server call does the recover + decrypt, so jump the checklist to the recover phase.
  setLoadingStage(loading, 1);
  const opened = await fetchJson("/api/viewers/ddrm-viewer/object/open", {
    method: "POST",
  });
  if (typeof opened.play_url !== "string" || opened.play_url === "") {
    throw new Error("object open did not return a view URL");
  }
  setLoadingStage(loading, 2);
  const launched = {
    target: "ddrm-viewer",
    title: "Owned asset",
    route: opened.play_url,
    attach_kind: "iframe",
    launch_status: "launched",
  };
  return placeLaunched(launched, options, loading);
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

function releaseFrameRuntimePage(node) {
  const frame = node.querySelector(".window-frame");
  if (!frame) {
    return;
  }
  try {
    const release = frame.contentWindow?.__elastosBrowserReleaseRuntimePage;
    if (typeof release === "function") {
      release();
    }
  } catch (_error) {
    // Cross-origin or failed frames cannot expose the Browser cleanup hook.
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
  if (node.dataset.maximized === "true") {
    restoreWindowFromSpecialState(node);
    fitLaunchedWindow(entry);
    focusWindow(id);
    persistBrowserSession();
    return;
  }
  rememberWindowRestoreBounds(node);
  node.dataset.snap = "";
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
      const entry = await launchBrowserTargetWindow(restoredWindow.target, {
        restoredPlacement: restoredWindow,
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
    releaseFrameRuntimePage(entry.node);
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
