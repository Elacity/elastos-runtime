export let desktop = document.querySelector("#desktop");
export let desktopBackdrop = document.querySelector(".desktop-backdrop");
export let desktopWorkspace = document.querySelector(".desktop-workspace");
export let desktopShortcuts = document.querySelector("#desktop-shortcuts");
export let desktopContextMenu = document.querySelector("#desktop-context-menu");
export let launcher = document.querySelector("#launcher");
export let launcherGrid = document.querySelector("#launcher-grid");
export let launcherEmptyState = document.querySelector("#launcher-empty-state");
export let launcherSearch = document.querySelector("#launcher-search");
export let launcherToggleButton = document.querySelector("#launcher-toggle");
export let launcherViewToggle = document.querySelector("#launcher-view-toggle");
export let toolbarActiveTitleNode = document.querySelector("#toolbar-active-title");
export let toolbarHomeButton = document.querySelector("#toolbar-home");
export let toolbarIdentityAvatar = document.querySelector("#toolbar-identity-avatar");
export let toolbarIdentityMonogram = document.querySelector("#toolbar-identity-monogram");
export let toolbarIdentityAvatarImage = document.querySelector("#toolbar-identity-avatar-image");
export let toolbarInboxButton = document.querySelector("#toolbar-inbox");
export let toolbarInboxCount = document.querySelector("#toolbar-inbox-count");
export let toolbarSpotlightButton = document.querySelector("#toolbar-spotlight");
export let toolbarFullscreenButton = document.querySelector("#control-centre-fullscreen");
export let toolbarSignOutButton = document.querySelector("#identity-menu-sign-out");
export let toolbarSystem = document.querySelector("#toolbar-system");
export let toolbarIdentityMenu = document.querySelector("#toolbar-identity-menu");
export let toolbarIdentityMenuName = document.querySelector("#toolbar-identity-menu-name");
export let identityMenuSystemButton = document.querySelector("#identity-menu-system");
export let identityMenuShowDesktopButton = document.querySelector("#identity-menu-show-desktop");
export let homeNotificationToast = document.querySelector("#home-notification-toast");
export let homeNotificationTitle = document.querySelector("#home-notification-title");
export let homeNotificationBody = document.querySelector("#home-notification-body");
export let homeNotificationAction = document.querySelector("#home-notification-action");
export let homeNotificationDismiss = document.querySelector("#home-notification-dismiss");
export let taskbarTargets = document.querySelector("#taskbar-targets");
export let clockNode = document.querySelector("#clock");
export let windowSnapPreview = document.querySelector("#window-snap-preview");
export let windowTemplate = document.querySelector("#window-template");
export let shortcutTemplate = document.querySelector("#shortcut-template");
export let launcherItemTemplate = document.querySelector("#launcher-item-template");
export let windowErrorTemplate = document.querySelector("#window-error-template");
export let taskbarItemTemplate = document.querySelector("#taskbar-item-template");

export const HOME_SHELL_HOST_ID = "home-shell-host";
export const HOME_GUI_SHELL_ID = "home-gui";
export const SYSTEM_APP_ID = "system";
export const PEOPLE_TARGET_ID = "people";
const MAX_RECENT_TARGETS = 10;
export const ICON_DRAG_THRESHOLD = 6;
const DESKTOP_ICON_WIDTH = 92;
const DESKTOP_ICON_HEIGHT = 98;
const DESKTOP_ICON_MARGIN = 12;
const DESKTOP_ICON_GAP_X = 96;
const DESKTOP_ICON_GAP_Y = 104;
const DEFAULT_DESKTOP_LAYOUT_WIDTH = 1280;
const DEFAULT_DESKTOP_LAYOUT_HEIGHT = 720;
const HOME_BROWSER_CONTEXT_PATTERN = /^browser:[0-9a-f]{32}$/;
export const WINDOW_MIN_WIDTH = 320;
export const WINDOW_MIN_HEIGHT = 220;
export const WINDOW_SNAP_THRESHOLD = 28;
export const WINDOW_SIDE_INSET = 8;
export const WINDOW_TOP_INSET = 8;
export const WINDOW_BOTTOM_INSET = 72;
export const CONTEXT_MENU_IGNORE_OUTSIDE_MS = 220;
const HOME_GUI_TEMPLATE_ID = "home-gui-template";
const HOME_GUI_TEMPLATE_URL = new URL("./home-gui-template.html?v=home-20260814a", import.meta.url).href;
const HOME_GUI_STYLESHEET_ID = "home-gui-stylesheet";
const HOME_GUI_STYLESHEET_URL = new URL("./style.css?v=home-20260814a", import.meta.url).href;
const HOME_GUI_AGENT_STYLESHEET_ID = "home-gui-agent-stylesheet";
const HOME_GUI_AGENT_STYLESHEET_URL = new URL(
  "./agent-harness.css?v=home-20260814a",
  import.meta.url,
).href;
let homeGuiTemplateHtmlPromise = null;
let homeGuiLaunchToken = "";

export function setHomeGuiLaunchToken(token) {
  const normalized = typeof token === "string" ? token.trim() : "";
  if (!normalized) {
    throw new Error("Home GUI launch token is required");
  }
  homeGuiLaunchToken = normalized;
}

export function getHomeGuiLaunchToken() {
  if (homeGuiLaunchToken) {
    return homeGuiLaunchToken;
  }
  try {
    const fromGlobal = globalThis.__elastosHomeGuiLaunchToken;
    if (typeof fromGlobal === "string" && fromGlobal.trim()) {
      homeGuiLaunchToken = fromGlobal.trim();
      return homeGuiLaunchToken;
    }
  } catch {
    /* ignore */
  }
  try {
    const fromSession = sessionStorage.getItem("elastos.home-gui.launch-token");
    if (typeof fromSession === "string" && fromSession.trim()) {
      homeGuiLaunchToken = fromSession.trim();
      return homeGuiLaunchToken;
    }
  } catch {
    /* ignore */
  }
  return "";
}

export const shellState = {
  windows: new Map(),
  frameAutoFitCleanup: new WeakMap(),
  zIndexCounter: 100,
  browserWindowSerial: 0,
  activeWindowId: null,
  activeStageId: "desktop",
  extraDesktops: [],
  spaceOrder: [],
  clockTimer: null,
  summaryRefreshDebounceTimer: null,
  summaryRefreshInFlight: false,
  summaryVisibilityRefreshBound: false,
  homeEventsCursor: "",
  homeEventsTimer: null,
  homeEventsInFlight: false,
  homeEventsSource: null,
  homeEventsStreamFailed: false,
  sessionRefreshTimer: null,
  browserContextId: "",
  contextMenuOpen: false,
  currentSummary: null,
  homeBrowserState: {
    principalId: "",
    layout: null,
    session: null,
    recentTargets: [],
  },
  shellLayoutState: {
    taskbar: [],
    desktop: {},
    desktopLabels: {},
    desktopHidden: [],
    desktopIconsVisible: true,
  },
  dragState: null,
  geometryInteractionCount: 0,
  lastInteractionEndedAt: 0,
  contextMenuTarget: { kind: "desktop" },
  contextMenuIgnoreOutsideUntil: 0,
  selectedDesktopTargetId: null,
  marqueeSelection: new Set(),
  recentTargetIds: [],
  selectedLauncherTargetId: null,
  launcherIgnoreOutsideUntil: 0,
  recentUiAction: { key: "", at: 0 },
  lastPointer: { x: 0, y: 0, at: 0 },
  lastPointerMove: { x: 0, y: 0, at: 0 },
  editingDesktopTargetId: null,
  longPressState: null,
  restoringSession: false,
  requestSummaryRefresh: null,
  homeGuiMounted: false,
};

export function retireHomeGuiWindowState() {
  if (shellState.windows.size === 0) {
    return;
  }
  for (const entry of shellState.windows.values()) {
    entry?.node?.remove?.();
  }
  shellState.windows.clear();
  shellState.activeWindowId = null;
}

function bindHomeGuiDomRefs() {
  desktop = document.querySelector("#desktop");
  desktopBackdrop = document.querySelector(".desktop-backdrop");
  desktopWorkspace = document.querySelector(".desktop-workspace");
  desktopShortcuts = document.querySelector("#desktop-shortcuts");
  desktopContextMenu = document.querySelector("#desktop-context-menu");
  launcher = document.querySelector("#launcher");
  launcherGrid = document.querySelector("#launcher-grid");
  launcherEmptyState = document.querySelector("#launcher-empty-state");
  launcherSearch = document.querySelector("#launcher-search");
  launcherToggleButton = document.querySelector("#launcher-toggle");
  launcherViewToggle = document.querySelector("#launcher-view-toggle");
  toolbarActiveTitleNode = document.querySelector("#toolbar-active-title");
  toolbarHomeButton = document.querySelector("#toolbar-home");
  toolbarIdentityAvatar = document.querySelector("#toolbar-identity-avatar");
  toolbarIdentityMonogram = document.querySelector("#toolbar-identity-monogram");
  toolbarIdentityAvatarImage = document.querySelector("#toolbar-identity-avatar-image");
  toolbarInboxButton = document.querySelector("#toolbar-inbox");
  toolbarInboxCount = document.querySelector("#toolbar-inbox-count");
  toolbarSpotlightButton = document.querySelector("#toolbar-spotlight");
  toolbarFullscreenButton = document.querySelector("#control-centre-fullscreen");
  toolbarSignOutButton = document.querySelector("#identity-menu-sign-out");
  toolbarSystem = document.querySelector("#toolbar-system");
  toolbarIdentityMenu = document.querySelector("#toolbar-identity-menu");
  toolbarIdentityMenuName = document.querySelector("#toolbar-identity-menu-name");
  identityMenuSystemButton = document.querySelector("#identity-menu-system");
  identityMenuShowDesktopButton = document.querySelector("#identity-menu-show-desktop");
  homeNotificationToast = document.querySelector("#home-notification-toast");
  homeNotificationTitle = document.querySelector("#home-notification-title");
  homeNotificationBody = document.querySelector("#home-notification-body");
  homeNotificationAction = document.querySelector("#home-notification-action");
  homeNotificationDismiss = document.querySelector("#home-notification-dismiss");
  taskbarTargets = document.querySelector("#taskbar-targets");
  clockNode = document.querySelector("#clock");
  windowSnapPreview = document.querySelector("#window-snap-preview");
  windowTemplate = document.querySelector("#window-template");
  shortcutTemplate = document.querySelector("#shortcut-template");
  launcherItemTemplate = document.querySelector("#launcher-item-template");
  windowErrorTemplate = document.querySelector("#window-error-template");
  taskbarItemTemplate = document.querySelector("#taskbar-item-template");
}

async function loadHomeGuiTemplateHtml() {
  if (!homeGuiTemplateHtmlPromise) {
    homeGuiTemplateHtmlPromise = fetch(HOME_GUI_TEMPLATE_URL)
      .then((response) => {
        if (!response.ok) {
          throw new Error(`Home GUI template request failed: ${response.status}`);
        }
        return response.text();
      })
      .catch((error) => {
        homeGuiTemplateHtmlPromise = null;
        throw error;
      });
  }
  return homeGuiTemplateHtmlPromise;
}

export async function ensureHomeGuiDom() {
  ensureHomeGuiStylesheet();
  if (desktop && desktopBackdrop && desktopWorkspace && taskbarTargets && launcher) {
    return;
  }
  let template = document.querySelector(`#${HOME_GUI_TEMPLATE_ID}`);
  if (!template?.content) {
    template = document.createElement("template");
    template.id = HOME_GUI_TEMPLATE_ID;
    template.innerHTML = await loadHomeGuiTemplateHtml();
  }
  const hostRoot = document.querySelector(".home-gui-shell, .home-host-shell");
  const activeShellRoot = document.querySelector("#active-shell-root");
  if (!template?.content || !hostRoot) {
    throw new Error("Home GUI template is unavailable");
  }
  if (activeShellRoot) {
    hostRoot.insertBefore(template.content.cloneNode(true), activeShellRoot);
  } else {
    hostRoot.appendChild(template.content.cloneNode(true));
  }
  bindHomeGuiDomRefs();
  if (
    !desktop ||
    !desktopBackdrop ||
    !desktopWorkspace ||
    !taskbarTargets ||
    !launcher ||
    !windowTemplate ||
    !shortcutTemplate ||
    !launcherItemTemplate ||
    !windowErrorTemplate ||
    !taskbarItemTemplate
  ) {
    throw new Error("Home GUI template did not create the required shell nodes");
  }
}

function ensureHomeGuiStylesheet() {
  const head = document.head || document.querySelector("head");
  if (!document.querySelector(`#${HOME_GUI_STYLESHEET_ID}`)) {
    const link = document.createElement("link");
    link.id = HOME_GUI_STYLESHEET_ID;
    link.rel = "stylesheet";
    link.href = HOME_GUI_STYLESHEET_URL;
    head?.appendChild?.(link);
  }
  if (!document.querySelector(`#${HOME_GUI_AGENT_STYLESHEET_ID}`)) {
    const agentLink = document.createElement("link");
    agentLink.id = HOME_GUI_AGENT_STYLESHEET_ID;
    agentLink.rel = "stylesheet";
    agentLink.href = HOME_GUI_AGENT_STYLESHEET_URL;
    head?.appendChild?.(agentLink);
  }
}

export function beginShellInteraction() {
  shellState.geometryInteractionCount += 1;
}

export function endShellInteraction() {
  shellState.geometryInteractionCount = Math.max(0, shellState.geometryInteractionCount - 1);
  shellState.lastInteractionEndedAt = Date.now();
}

export function shellInteractionActive(graceMs = 250) {
  return Boolean(shellState.dragState)
    || shellState.geometryInteractionCount > 0
    || Date.now() - shellState.lastInteractionEndedAt < graceMs;
}

export async function fetchJson(url, init) {
  const response = await fetch(url, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(homeGuiLaunchToken ? { "x-elastos-home-token": homeGuiLaunchToken } : {}),
      ...(init && init.headers ? init.headers : {}),
    },
  });
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const error = new Error(
      `request failed: ${response.status} ${response.statusText}${detail ? ` ${detail}` : ""}`,
    );
    error.status = response.status;
    throw error;
  }
  return response.json();
}

export async function mutateDesktopObject(op, payload = {}) {
  return fetchJson("/api/apps/home/desktop/objects", {
    method: "POST",
    body: JSON.stringify({ op, ...payload }),
  });
}

export function allVisibleTargets(summary) {
  if (!summary || !Array.isArray(summary.targets)) {
    return [];
  }
  return summary.targets
    .filter((target) => target?.role !== "shell")
    .map((target) => ({
      ...target,
      title: normalizeText(target?.title) || target?.target,
    }));
}

export function desktopObjects(summary) {
  const objects = summary &&
    summary.desktop_objects &&
    Array.isArray(summary.desktop_objects.objects)
    ? summary.desktop_objects.objects
    : [];
  return objects.filter((object) => (
    object &&
    typeof object.uri === "string" &&
    object.uri.trim() !== "" &&
    typeof object.name === "string" &&
    object.name.trim() !== ""
  ));
}

export function desktopObjectEntryId(object) {
  return `object:${object.uri}`;
}

export function desktopObjectByEntryId(summary, entryId) {
  if (typeof entryId !== "string" || !entryId.startsWith("object:")) {
    return null;
  }
  const uri = entryId.slice("object:".length);
  return desktopObjects(summary).find((object) => object.uri === uri) || null;
}

export function desktopEntryExists(summary, entryId) {
  if (!summary || typeof entryId !== "string" || entryId.trim() === "") {
    return false;
  }
  if (entryId.startsWith("object:")) {
    return Boolean(desktopObjectByEntryId(summary, entryId));
  }
  return Boolean(targetById(summary, entryId) && isTargetOnDesktop(entryId));
}

export function desktopLayoutEntries(summary) {
  const appEntries = allVisibleTargets(summary).map((target) => ({
    id: target.target,
    kind: "target",
    target,
  }));
  const objectEntries = desktopObjects(summary).map((object) => ({
    id: desktopObjectEntryId(object),
    kind: "object",
    object,
  }));
  return [...appEntries, ...objectEntries];
}

function syncHomeBrowserState(summary) {
  const state = summary && summary.browser_state && typeof summary.browser_state === "object"
    ? summary.browser_state
    : {};
  shellState.homeBrowserState = {
    principalId: normalizeText(state.principal_id),
    layout: state.layout && typeof state.layout === "object" ? state.layout : null,
    session: state.session && typeof state.session === "object" ? state.session : null,
    recentTargets: Array.isArray(state.recent_targets) ? state.recent_targets : [],
  };
}

function saveHomeBrowserState(patch) {
  if (!shellState.homeBrowserState.principalId) {
    return;
  }
  const body = JSON.stringify(patch || {});
  fetchJson("/api/apps/home/state", {
    method: "POST",
    keepalive: body.length < 60_000,
    body,
  }).catch((error) => {
    console.warn("home state save failed", error);
  });
}

export function targetById(summary, targetId) {
  return allVisibleTargets(summary).find((target) => target.target === targetId) || null;
}

export function shellAppId(summary) {
  return normalizeText(summary && summary.app && summary.app.id) || HOME_GUI_SHELL_ID;
}

export function targetTitle(summary, targetId) {
  const target = targetById(summary, targetId);
  return canonicalTargetTitle(targetId, target?.title);
}

export function canonicalTargetTitle(targetId, title) {
  const normalizedTitle = normalizeText(title);
  return normalizedTitle || targetId;
}

/* Window chrome modes are presentation-only. Fail closed to standard.
   unified-sidebar: split-view apps — lights over the leading column.
   unified-toolbar: Browser-style — lights over the tool row; menubar owns the
   focused app name. */
export const WINDOW_CHROME_STANDARD = "standard";
export const WINDOW_CHROME_UNIFIED_SIDEBAR = "unified-sidebar";
export const WINDOW_CHROME_UNIFIED_TOOLBAR = "unified-toolbar";

const WINDOW_CHROME_BY_TARGET = {
  marketplace: WINDOW_CHROME_UNIFIED_SIDEBAR,
  system: WINDOW_CHROME_UNIFIED_SIDEBAR,
  library: WINDOW_CHROME_UNIFIED_SIDEBAR,
  services: WINDOW_CHROME_UNIFIED_SIDEBAR,
  people: WINDOW_CHROME_UNIFIED_SIDEBAR,
  documents: WINDOW_CHROME_UNIFIED_SIDEBAR,
  inbox: WINDOW_CHROME_UNIFIED_SIDEBAR,
  chat: WINDOW_CHROME_UNIFIED_SIDEBAR,
  "chat-room": WINDOW_CHROME_UNIFIED_SIDEBAR,
  browser: WINDOW_CHROME_UNIFIED_TOOLBAR,
};

const WINDOW_CHROME_CONTINUOUS_TARGETS = new Set([
  "wallet",
  "archive-manager",
  "gba-emulator",
  "wallet-metamask",
  "wallet-unisat",
  "wallet-walletconnect",
]);

export function parseWindowChromeMode(value) {
  if (value === WINDOW_CHROME_UNIFIED_SIDEBAR) {
    return WINDOW_CHROME_UNIFIED_SIDEBAR;
  }
  if (value === WINDOW_CHROME_UNIFIED_TOOLBAR) {
    return WINDOW_CHROME_UNIFIED_TOOLBAR;
  }
  return WINDOW_CHROME_STANDARD;
}

function windowChromeModeForTarget(targetId) {
  return parseWindowChromeMode(
    WINDOW_CHROME_BY_TARGET[normalizeText(targetId).toLowerCase()] || WINDOW_CHROME_STANDARD,
  );
}

export function applyWindowChrome(windowNode, targetId) {
  if (!windowNode) {
    return WINDOW_CHROME_STANDARD;
  }
  windowNode.classList.remove(
    "window-chrome-unified-sidebar",
    "window-chrome-unified-toolbar",
    "window-chrome-continuous",
  );
  const mode = windowChromeModeForTarget(targetId);
  if (mode === WINDOW_CHROME_UNIFIED_SIDEBAR) {
    windowNode.classList.add("window-chrome-unified-sidebar");
  } else if (mode === WINDOW_CHROME_UNIFIED_TOOLBAR) {
    windowNode.classList.add("window-chrome-unified-toolbar");
  } else if (WINDOW_CHROME_CONTINUOUS_TARGETS.has(normalizeText(targetId).toLowerCase())) {
    windowNode.classList.add("window-chrome-continuous");
  }
  windowNode.dataset.windowChromeMode = mode;
  return mode;
}

export function desktopLabelForTarget(summary, targetId) {
  const custom = shellState.shellLayoutState.desktopLabels[targetId];
  return normalizeText(custom) || targetTitle(summary, targetId);
}

function sortedDesktopTargets(summary) {
  return [...allVisibleTargets(summary)].sort((left, right) => {
    const leftTitle = normalizeText(left.title) || left.target;
    const rightTitle = normalizeText(right.title) || right.target;
    const titleOrder = leftTitle.localeCompare(rightTitle, undefined, {
      sensitivity: "base",
      numeric: true,
    });
    if (titleOrder !== 0) {
      return titleOrder;
    }
    return left.target.localeCompare(right.target, undefined, {
      sensitivity: "base",
      numeric: true,
    });
  });
}

export function initializeShellLayout(summary) {
  syncHomeBrowserState(summary);
  const stored = shellState.homeBrowserState.layout;
  const normalizedDesktopHidden = normalizeDesktopHiddenTargets(
    stored ? stored.desktopHidden : null,
    summary,
  );
  shellState.shellLayoutState = {
    taskbar: normalizeTaskbarLayout(stored ? stored.taskbar : null, summary),
    desktop: {},
    desktopLabels: normalizeDesktopLabels(stored ? stored.desktopLabels : null, summary),
    desktopHidden: normalizedDesktopHidden,
    desktopIconsVisible: normalizeDesktopIconsVisible(stored ? stored.desktopIconsVisible : null),
  };

  let changed =
    !stored ||
    !arrayEquals(
      Array.isArray(stored.desktopHidden) ? stored.desktopHidden : [],
      normalizedDesktopHidden,
    ) ||
    typeof stored.desktopIconsVisible !== "boolean";
  const storedDesktop = stored && stored.desktop && typeof stored.desktop === "object"
    ? stored.desktop
    : {};
  const occupiedPositions = [];
  for (const [index, entry] of desktopLayoutEntries(summary).entries()) {
    const defaultPosition = defaultDesktopPosition(index);
    const storedPosition = storedDesktop[entry.id];
    let position = clampDesktopPosition(normalizeDesktopPosition(storedPosition, defaultPosition));
    if (desktopPositionOverlapsAny(position, occupiedPositions)) {
      position = nextAvailableDesktopPosition(occupiedPositions, index);
      changed = true;
    }
    shellState.shellLayoutState.desktop[entry.id] = position;
    occupiedPositions.push(position);
    if (!storedPosition || !positionsEqual(storedPosition, position)) {
      changed = true;
    }
  }
  const currentDesktopIds = new Set(Object.keys(shellState.shellLayoutState.desktop));
  if (Object.keys(storedDesktop).some((id) => !currentDesktopIds.has(id))) {
    changed = true;
  }

  if (changed) {
    saveShellLayoutState();
  }
}

export function saveShellLayoutState() {
  shellState.homeBrowserState.layout = shellState.shellLayoutState;
  saveHomeBrowserState({ layout: shellState.shellLayoutState });
}

export function loadShellSessionState() {
  if (!hasHomeBrowserContextId()) {
    return null;
  }
  const session = shellState.homeBrowserState.session;
  if (!session || typeof session !== "object") {
    return null;
  }
  return session.browser_context_id === shellState.browserContextId ? session : null;
}

export function saveShellSessionState(session) {
  if (!hasHomeBrowserContextId()) {
    return false;
  }
  shellState.homeBrowserState.session = session && typeof session === "object"
    ? { ...session, browser_context_id: shellState.browserContextId }
    : null;
  saveHomeBrowserState({ session: shellState.homeBrowserState.session });
  return true;
}

export function initializeRecentTargets(summary) {
  syncHomeBrowserState(summary);
  const stored = shellState.homeBrowserState.recentTargets;
  const normalized = normalizeRecentTargets(stored, summary);
  shellState.recentTargetIds = normalized;
  if (!arrayEquals(stored, normalized)) {
    saveRecentTargets();
  }
}

function saveRecentTargets() {
  shellState.homeBrowserState.recentTargets = shellState.recentTargetIds;
  saveHomeBrowserState({ recent_targets: shellState.recentTargetIds });
}

function normalizeRecentTargets(targetIds, summary) {
  const knownTargets = new Set(allVisibleTargets(summary).map((target) => target.target));
  const normalized = [];
  for (const targetId of Array.isArray(targetIds) ? targetIds : []) {
    if (
      typeof targetId !== "string" ||
      !knownTargets.has(targetId) ||
      normalized.includes(targetId)
    ) {
      continue;
    }
    normalized.push(targetId);
    if (normalized.length >= MAX_RECENT_TARGETS) {
      break;
    }
  }
  return normalized;
}

export function isHomeBrowserContextId(value) {
  return typeof value === "string" && HOME_BROWSER_CONTEXT_PATTERN.test(value);
}

export function acceptHomeBrowserContextId(value) {
  if (!isHomeBrowserContextId(value)) {
    return false;
  }
  if (shellState.browserContextId && shellState.browserContextId !== value) {
    return false;
  }
  shellState.browserContextId = value;
  return true;
}

export function hasHomeBrowserContextId() {
  return isHomeBrowserContextId(shellState.browserContextId);
}

export function rememberRecentTarget(targetId) {
  if (!shellState.currentSummary || !targetById(shellState.currentSummary, targetId)) {
    return;
  }
  const next = [
    targetId,
    ...shellState.recentTargetIds.filter((candidate) => candidate !== targetId),
  ].slice(0, MAX_RECENT_TARGETS);
  if (arrayEquals(next, shellState.recentTargetIds)) {
    return;
  }
  shellState.recentTargetIds = next;
  saveRecentTargets();
}

function normalizeTaskbarLayout(taskbar, summary) {
  const knownTargets = new Set(allVisibleTargets(summary).map((target) => target.target));
  if (!Array.isArray(taskbar)) {
    return [];
  }
  const normalized = [];
  for (const targetId of taskbar) {
    if (
      typeof targetId !== "string" ||
      !knownTargets.has(targetId) ||
      normalized.includes(targetId)
    ) {
      continue;
    }
    normalized.push(targetId);
  }
  return normalized;
}

function normalizeDesktopLabels(labels, summary) {
  const knownTargets = new Set(allVisibleTargets(summary).map((target) => target.target));
  const normalized = {};
  if (!labels || typeof labels !== "object") {
    return normalized;
  }
  for (const [targetId, label] of Object.entries(labels)) {
    const nextLabel = normalizeText(label);
    if (!knownTargets.has(targetId) || nextLabel === "") {
      continue;
    }
    normalized[targetId] = nextLabel;
  }
  return normalized;
}

function normalizeDesktopHiddenTargets(targetIds, summary) {
  const knownTargets = new Set(allVisibleTargets(summary).map((target) => target.target));
  const normalized = [];
  for (const targetId of Array.isArray(targetIds) ? targetIds : []) {
    if (
      typeof targetId !== "string" ||
      !knownTargets.has(targetId) ||
      normalized.includes(targetId)
    ) {
      continue;
    }
    normalized.push(targetId);
  }
  return normalized;
}

function normalizeDesktopIconsVisible(value) {
  return typeof value === "boolean" ? value : true;
}

function normalizeDesktopPosition(position, defaultPosition) {
  const x = Number.isFinite(position && position.x) ? position.x : defaultPosition.x;
  const y = Number.isFinite(position && position.y) ? position.y : defaultPosition.y;
  return { x, y };
}

function desktopLayoutBounds() {
  const rect = desktop?.getBoundingClientRect?.();
  const width = Number.isFinite(rect?.width) && rect.width > 0
    ? rect.width
    : DEFAULT_DESKTOP_LAYOUT_WIDTH;
  const height = Number.isFinite(rect?.height) && rect.height > 0
    ? rect.height
    : DEFAULT_DESKTOP_LAYOUT_HEIGHT;
  return { width, height };
}

function defaultDesktopPosition(index) {
  const rect = desktopLayoutBounds();
  const usableHeight = Math.max(
    DESKTOP_ICON_GAP_Y,
    rect.height - DESKTOP_ICON_MARGIN * 2,
  );
  const rows = Math.max(1, Math.floor(usableHeight / DESKTOP_ICON_GAP_Y));
  const row = index % rows;
  const column = Math.floor(index / rows);
  return {
    x: DESKTOP_ICON_MARGIN + column * DESKTOP_ICON_GAP_X,
    y: DESKTOP_ICON_MARGIN + row * DESKTOP_ICON_GAP_Y,
  };
}

function desktopPositionsOverlap(left, right) {
  return (
    Math.abs(left.x - right.x) < DESKTOP_ICON_WIDTH &&
    Math.abs(left.y - right.y) < DESKTOP_ICON_HEIGHT
  );
}

function desktopPositionOverlapsAny(position, positions) {
  return positions.some((candidate) => desktopPositionsOverlap(position, candidate));
}

function nextAvailableDesktopPosition(occupiedPositions, preferredIndex) {
  for (let offset = 0; offset < 200; offset += 1) {
    const candidate = clampDesktopPosition(defaultDesktopPosition(preferredIndex + offset));
    if (!desktopPositionOverlapsAny(candidate, occupiedPositions)) {
      return candidate;
    }
  }
  return clampDesktopPosition(defaultDesktopPosition(preferredIndex));
}

function occupiedDesktopPositionsExcept(targetId) {
  return Object.entries(shellState.shellLayoutState.desktop)
    .filter(([entryId]) => entryId !== targetId)
    .map(([, position]) => clampDesktopPosition(position));
}

export function clampDesktopPosition(position) {
  const rect = desktopLayoutBounds();
  const maxX = Math.max(
    DESKTOP_ICON_MARGIN,
    rect.width - DESKTOP_ICON_WIDTH - DESKTOP_ICON_MARGIN,
  );
  const maxY = Math.max(
    DESKTOP_ICON_MARGIN,
    rect.height - DESKTOP_ICON_HEIGHT - DESKTOP_ICON_MARGIN,
  );
  return {
    x: clamp(position.x, DESKTOP_ICON_MARGIN, maxX),
    y: clamp(position.y, DESKTOP_ICON_MARGIN, maxY),
  };
}

export function snapDesktopPosition(entryId, position) {
  const clamped = clampDesktopPosition(position);
  const snapped = clampDesktopPosition({
    x: DESKTOP_ICON_MARGIN +
      Math.round((clamped.x - DESKTOP_ICON_MARGIN) / DESKTOP_ICON_GAP_X) * DESKTOP_ICON_GAP_X,
    y: DESKTOP_ICON_MARGIN +
      Math.round((clamped.y - DESKTOP_ICON_MARGIN) / DESKTOP_ICON_GAP_Y) * DESKTOP_ICON_GAP_Y,
  });
  if (desktopPositionOverlapsAny(snapped, occupiedDesktopPositionsExcept(entryId))) {
    return clamped;
  }
  return snapped;
}

export function desktopPositionForTarget(targetId, defaultIndex) {
  const stored = shellState.shellLayoutState.desktop[targetId];
  if (stored) {
    return clampDesktopPosition(stored);
  }
  const defaultPosition = nextAvailableDesktopPosition(
    occupiedDesktopPositionsExcept(targetId),
    defaultIndex,
  );
  shellState.shellLayoutState.desktop[targetId] = defaultPosition;
  saveShellLayoutState();
  return defaultPosition;
}

export function setDesktopPosition(targetId, position) {
  const next = clampDesktopPosition(position);
  if (positionsEqual(shellState.shellLayoutState.desktop[targetId], next)) {
    return false;
  }
  shellState.shellLayoutState.desktop[targetId] = next;
  return true;
}

export function setDesktopIconsVisible(visible) {
  const next = visible !== false;
  if (shellState.shellLayoutState.desktopIconsVisible === next) {
    return false;
  }
  shellState.shellLayoutState.desktopIconsVisible = next;
  saveShellLayoutState();
  return true;
}

export function isTargetOnDesktop(targetId) {
  return !shellState.shellLayoutState.desktopHidden.includes(targetId);
}

export function addTargetToDesktop(targetId) {
  const next = shellState.shellLayoutState.desktopHidden.filter(
    (candidate) => candidate !== targetId,
  );
  if (arrayEquals(next, shellState.shellLayoutState.desktopHidden)) {
    return false;
  }
  shellState.shellLayoutState.desktopHidden = next;
  return true;
}

export function removeTargetFromDesktop(targetId) {
  if (!shellState.currentSummary || !targetById(shellState.currentSummary, targetId)) {
    return false;
  }
  if (shellState.shellLayoutState.desktopHidden.includes(targetId)) {
    return false;
  }
  shellState.shellLayoutState.desktopHidden = [
    ...shellState.shellLayoutState.desktopHidden,
    targetId,
  ];
  return true;
}

export function setDesktopLabel(targetId, label, summary = shellState.currentSummary) {
  const canonicalTitle = targetTitle(summary, targetId);
  const nextLabel = normalizeText(label);
  const currentLabel = normalizeText(shellState.shellLayoutState.desktopLabels[targetId]);
  const normalizedNext = nextLabel === canonicalTitle ? "" : nextLabel;
  if (currentLabel === normalizedNext) {
    return false;
  }
  if (normalizedNext === "") {
    delete shellState.shellLayoutState.desktopLabels[targetId];
    return true;
  }
  shellState.shellLayoutState.desktopLabels[targetId] = normalizedNext;
  return true;
}

export function autoArrangeDesktopIcons(summary = shellState.currentSummary) {
  if (!summary) {
    return false;
  }
  let changed = false;
  const desktopTargets = sortedDesktopTargets(summary)
    .filter((target) => isTargetOnDesktop(target.target))
    .map((target) => ({ id: target.target }));
  const desktopObjectEntries = desktopObjects(summary).map((object) => ({
    id: desktopObjectEntryId(object),
  }));
  for (const [index, entry] of [...desktopTargets, ...desktopObjectEntries].entries()) {
    changed = setDesktopPosition(entry.id, defaultDesktopPosition(index)) || changed;
  }
  if (!changed) {
    return false;
  }
  saveShellLayoutState();
  return true;
}

export function pinTargetToTaskbar(targetId, index) {
  const next = shellState.shellLayoutState.taskbar.filter(
    (candidate) => candidate !== targetId,
  );
  const insertionIndex = clamp(index, 0, next.length);
  next.splice(insertionIndex, 0, targetId);
  if (arrayEquals(next, shellState.shellLayoutState.taskbar)) {
    return false;
  }
  shellState.shellLayoutState.taskbar = next;
  return true;
}

export function unpinTargetFromTaskbar(targetId) {
  const next = shellState.shellLayoutState.taskbar.filter(
    (candidate) => candidate !== targetId,
  );
  if (arrayEquals(next, shellState.shellLayoutState.taskbar)) {
    return false;
  }
  shellState.shellLayoutState.taskbar = next;
  return true;
}

export function isTargetPinnedToTaskbar(targetId) {
  return shellState.shellLayoutState.taskbar.includes(targetId);
}

export function clampDesktopLayoutToViewport() {
  if (!shellState.currentSummary) {
    return false;
  }
  let changed = false;
  for (const [index, entry] of desktopLayoutEntries(shellState.currentSummary).entries()) {
    const next = clampDesktopPosition(desktopPositionForTarget(entry.id, index));
    if (!positionsEqual(shellState.shellLayoutState.desktop[entry.id], next)) {
      shellState.shellLayoutState.desktop[entry.id] = next;
      changed = true;
    }
  }
  return changed;
}

export function trapTabWithin(container, event) {
  if (event.key !== "Tab" || !container) {
    return false;
  }
  const focusables = Array.from(
    container.querySelectorAll(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) => !element.hidden && !element.disabled && element.offsetParent !== null,
  );
  if (focusables.length === 0) {
    event.preventDefault();
    return true;
  }
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  const active = document.activeElement;
  if (event.shiftKey && (active === first || !container.contains(active))) {
    event.preventDefault();
    last.focus();
    return true;
  }
  if (!event.shiftKey && (active === last || !container.contains(active))) {
    event.preventDefault();
    first.focus();
    return true;
  }
  return false;
}

export function mountGlyph(container, targetId, forcedTone) {
  const tone = forcedTone || glyphTone(targetId);
  container.dataset.tone = tone;
  const capsuleIcon = capsuleIconMarkup(targetId);
  if (capsuleIcon) {
    // "raster" tells the stylesheet to drop the synthetic tile it draws behind
    // the built-in glyphs, so the capsule's own artwork stands alone.
    container.dataset.tone = "raster";
    container.innerHTML = capsuleIcon;
    return;
  }
  container.innerHTML = glyphSvg(targetId);
}

export function formatBadgeCount(count, cap = 99) {
  const n = Math.max(0, Number(count) || 0);
  if (n === 0) {
    return "";
  }
  return n > cap ? `${cap}+` : String(n);
}

let focusModeMemory = "";

export function focusModeEnabled() {
  return focusModeMemory === "on";
}

export function setFocusModeEnabled(on) {
  focusModeMemory = on ? "on" : "off";
}

// Capsule asset routes the Runtime resolved from each capsule's declared icon
// directory. The shell keeps no icon table of its own: an app that ships an
// icon owns it, and one that ships none gets the built-in glyph below.
const CAPSULE_ICON_ROUTE = /^\/apps\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_./-]+\.png$/;

function capsuleIconVariants(targetId) {
  const target = targetById(shellState.currentSummary, targetId);
  const variants = Array.isArray(target?.icon) ? target.icon : [];
  return variants.filter((variant) => (
    Number.isFinite(variant?.size) &&
    typeof variant?.route === "string" &&
    CAPSULE_ICON_ROUTE.test(variant.route)
  ));
}

function capsuleIconMarkup(targetId) {
  const variants = capsuleIconVariants(targetId);
  if (variants.length === 0) {
    return "";
  }
  const bySize = new Map(variants.map((variant) => [variant.size, variant.route]));
  const preferred = bySize.get(128) || variants[variants.length - 1].route;
  const srcset = variants
    .map((variant) => `${variant.route} ${variant.size}w`)
    .join(", ");
  return `<img class="app-icon-img" src="${preferred}" srcset="${srcset}" sizes="(max-width: 640px) 48px, 64px" alt="" draggable="false" />`;
}

export function glyphTone(targetId) {
  if (targetId === "trash" || targetId === "trash-full") {
    return "system";
  }
  if (targetId === SYSTEM_APP_ID) {
    return "system";
  }
  if (targetId === "inbox") {
    return "docs";
  }
  if (targetId === "marketplace") {
    return "market";
  }
  if (targetId.includes("wallet")) {
    return "wallet";
  }
  if (targetId === "browser") {
    return "browser";
  }
  if (targetId.includes("file")) {
    return "docs";
  }
  if (targetId.includes("room")) {
    return "room";
  }
  if (targetId === PEOPLE_TARGET_ID) {
    return "room";
  }
  if (targetId.includes("md") || targetId.includes("doc") || targetId.includes("viewer")) {
    return "docs";
  }
  if (targetId.includes("gba") || targetId.includes("emu") || targetId.includes("game")) {
    return "games";
  }
  return "default";
}

function glyphSvg(targetId) {
  if (targetId === "trash" || targetId === "trash-full") {
    const full = targetId === "trash-full";
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M8 7.25h8" />
        <path d="M10 7.25V5.5h4v1.75" />
        <path d="M6.75 9.25h10.5l-.75 9.25a2 2 0 0 1-2 1.82h-5a2 2 0 0 1-2-1.82Z" />
        ${full ? '<path d="M9.25 12.25h5.5" /><path d="M9.75 15.25h4.5" />' : '<path d="M10 12.25v4.5" /><path d="M14 12.25v4.5" />'}
      </svg>
    `;
  }
  if (targetId === SYSTEM_APP_ID) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 3v3" />
        <path d="M12 18v3" />
        <path d="M4.93 4.93l2.12 2.12" />
        <path d="M16.95 16.95l2.12 2.12" />
        <path d="M3 12h3" />
        <path d="M18 12h3" />
        <path d="M4.93 19.07l2.12-2.12" />
        <path d="M16.95 7.05l2.12-2.12" />
        <circle cx="12" cy="12" r="4.2" />
      </svg>
    `;
  }
  if (targetId === "inbox") {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 6.75A2.25 2.25 0 0 1 6.25 4.5h11.5A2.25 2.25 0 0 1 20 6.75v10.5a2.25 2.25 0 0 1-2.25 2.25H6.25A2.25 2.25 0 0 1 4 17.25Z" />
        <path d="m5.25 7.25 5.48 4.12a2.1 2.1 0 0 0 2.54 0l5.48-4.12" />
        <path d="M8 15.5h8" />
      </svg>
    `;
  }
  if (targetId === "marketplace") {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M5 9.25 6.3 4.5h11.4L19 9.25" />
        <path d="M5 9.25h14" />
        <path d="M6 9.25V18a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 18 18V9.25" />
        <path d="M9 13.25h6" />
        <path d="M9 16.25h4" />
      </svg>
    `;
  }
  if (targetId.includes("room")) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="3.5" y="4.5" width="17" height="15" rx="3" />
        <path d="M8 9h8" />
        <path d="M8 13h4.5" />
        <circle cx="16.75" cy="13.25" r="1.25" fill="currentColor" stroke="none" />
      </svg>
    `;
  }
  if (targetId === PEOPLE_TARGET_ID) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="9" cy="8" r="3" />
        <circle cx="16.5" cy="9.25" r="2.25" />
        <path d="M4.5 19.25a4.5 4.5 0 0 1 9 0" />
        <path d="M13.25 17.75a3.5 3.5 0 0 1 6.25 1.5" />
      </svg>
    `;
  }
  if (targetId.includes("file")) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4.5 7.25A2.25 2.25 0 0 1 6.75 5h3.35a2 2 0 0 1 1.4.57l1.18 1.18a2 2 0 0 0 1.41.58h3.16A2.25 2.25 0 0 1 19.5 9.6v7.65a2.25 2.25 0 0 1-2.25 2.25H6.75a2.25 2.25 0 0 1-2.25-2.25Z" />
        <path d="M7.75 12.25h8.5" />
        <path d="M7.75 15.25h5.75" />
      </svg>
    `;
  }
  if (targetId.includes("gba") || targetId.includes("emu") || targetId.includes("game")) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="4" y="8" width="16" height="8" rx="4" />
        <path d="M8 12h4" />
        <path d="M10 10v4" />
        <circle cx="15.5" cy="11" r="0.9" fill="currentColor" stroke="none" />
        <circle cx="17.8" cy="13" r="0.9" fill="currentColor" stroke="none" />
      </svg>
    `;
  }
  if (targetId.includes("wallet")) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 8.2A2.2 2.2 0 0 1 6.2 6h11.6A2.2 2.2 0 0 1 20 8.2v7.6a2.2 2.2 0 0 1-2.2 2.2H6.2A2.2 2.2 0 0 1 4 15.8Z" />
        <path d="M17 10.25h3v3.5h-3a1.75 1.75 0 1 1 0-3.5Z" />
        <path d="M6.5 8.25h7" />
      </svg>
    `;
  }
  if (targetId === "browser") {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="7.75" />
        <path d="M4.6 10h14.8" />
        <path d="M4.6 14h14.8" />
        <path d="M12 4.25c2.15 2.1 3.2 4.68 3.2 7.75s-1.05 5.65-3.2 7.75" />
        <path d="M12 4.25C9.85 6.35 8.8 8.93 8.8 12s1.05 5.65 3.2 7.75" />
      </svg>
    `;
  }
  if (targetId.includes("md") || targetId.includes("doc") || targetId.includes("viewer")) {
    return `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M7 3.75h7l4 4V20a1.25 1.25 0 0 1-1.25 1.25h-9.5A1.25 1.25 0 0 1 6 20V5A1.25 1.25 0 0 1 7.25 3.75Z" />
        <path d="M14 3.75V8h4" />
        <path d="M9 12h6" />
        <path d="M9 15h6" />
      </svg>
    `;
  }
  return `
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="4" y="4" width="7" height="7" rx="1.5" />
      <rect x="13" y="4" width="7" height="7" rx="1.5" />
      <rect x="4" y="13" width="7" height="7" rx="1.5" />
      <rect x="13" y="13" width="7" height="7" rx="1.5" />
    </svg>
  `;
}

function normalizeText(value) {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : "";
}

export function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}

export function pointInRect(clientX, clientY, rect) {
  return (
    clientX >= rect.left &&
    clientX <= rect.right &&
    clientY >= rect.top &&
    clientY <= rect.bottom
  );
}

export function ignoreRepeatedAction(key, windowMs = 350) {
  const now = window.performance ? window.performance.now() : Date.now();
  if (
    shellState.recentUiAction.key === key &&
    now - shellState.recentUiAction.at < windowMs
  ) {
    return true;
  }
  shellState.recentUiAction = { key, at: now };
  return false;
}

function positionsEqual(left, right) {
  return Boolean(left) && Boolean(right) && left.x === right.x && left.y === right.y;
}

function arrayEquals(left, right) {
  if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) {
    return false;
  }
  return left.every((entry, index) => entry === right[index]);
}

export function shouldOpenMaximizedByDefault() {
  return window.innerWidth <= 640;
}

export function shouldIgnoreDesktopKeydown(event) {
  const target = event.target;
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return Boolean(
    target.closest("input, textarea, select, [contenteditable='true']") ||
    target.closest(".window") ||
    target.closest("#launcher") ||
    target.closest("#desktop-context-menu"),
  );
}

export function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll("\"", "&quot;")
    .replaceAll("'", "&#39;");
}

const sharedUiPreferences = {};

export function rememberSharedUiPreferences(preferences) {
  if (preferences && typeof preferences === "object") {
    Object.assign(sharedUiPreferences, preferences);
  }
}

export function pushUiPreferencesToFrameWindow(frameWindow) {
  if (!frameWindow || Object.keys(sharedUiPreferences).length === 0) {
    return;
  }
  try {
    frameWindow.postMessage(
      { type: "elastos:ui-preference", preferences: { ...sharedUiPreferences } },
      "*",
    );
  } catch (_error) {
    // Frame mid-teardown.
  }
}
