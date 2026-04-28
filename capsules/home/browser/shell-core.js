export const desktop = document.querySelector("#desktop");
export const desktopBackdrop = document.querySelector(".desktop-backdrop");
export const desktopWorkspace = document.querySelector(".desktop-workspace");
export const desktopShortcuts = document.querySelector("#desktop-shortcuts");
export const desktopContextMenu = document.querySelector("#desktop-context-menu");
export const launcher = document.querySelector("#launcher");
export const launcherGrid = document.querySelector("#launcher-grid");
export const launcherEmptyState = document.querySelector("#launcher-empty-state");
export const launcherSearch = document.querySelector("#launcher-search");
export const launcherToggleButton = document.querySelector("#launcher-toggle");
export const closeLauncherButton = document.querySelector("#close-launcher");
export const toolbarHomeButton = document.querySelector("#toolbar-home");
export const toolbarInboxButton = document.querySelector("#toolbar-inbox");
export const toolbarInboxCount = document.querySelector("#toolbar-inbox-count");
export const toolbarFullscreenButton = document.querySelector("#toolbar-fullscreen");
export const taskbarTargets = document.querySelector("#taskbar-targets");
export const clockNode = document.querySelector("#clock");
export const windowSnapPreview = document.querySelector("#window-snap-preview");
export const windowTemplate = document.querySelector("#window-template");
export const shortcutTemplate = document.querySelector("#shortcut-template");
export const launcherItemTemplate = document.querySelector("#launcher-item-template");
export const windowErrorTemplate = document.querySelector("#window-error-template");
export const taskbarItemTemplate = document.querySelector("#taskbar-item-template");

export const SHELL_APP_ID = "home";
export const SYSTEM_APP_ID = "system";
const SHELL_LAYOUT_STORAGE_KEY = "home-layout";
const SHELL_SESSION_STORAGE_KEY = "home-session";
const RECENT_TARGETS_STORAGE_KEY = "home-recent-targets";
const MAX_RECENT_TARGETS = 10;
export const ICON_DRAG_THRESHOLD = 6;
const DESKTOP_ICON_WIDTH = 92;
const DESKTOP_ICON_HEIGHT = 98;
const DESKTOP_ICON_MARGIN = 12;
const DESKTOP_ICON_GAP_X = 96;
const DESKTOP_ICON_GAP_Y = 104;
export const WINDOW_MIN_WIDTH = 320;
export const WINDOW_MIN_HEIGHT = 220;
export const WINDOW_SNAP_THRESHOLD = 28;
export const WINDOW_SIDE_INSET = 8;
export const WINDOW_TOP_INSET = 8;
export const WINDOW_BOTTOM_INSET = 72;
export const CONTEXT_MENU_IGNORE_OUTSIDE_MS = 220;

export const shellState = {
  windows: new Map(),
  frameAutoFitCleanup: new WeakMap(),
  zIndexCounter: 100,
  browserWindowSerial: 0,
  activeWindowId: null,
  clockTimer: null,
  contextMenuOpen: false,
  currentSummary: null,
  shellLayoutState: {
    taskbar: [],
    desktop: {},
    desktopLabels: {},
    desktopHidden: [],
    desktopIconsVisible: true,
  },
  dragState: null,
  contextMenuTarget: { kind: "desktop" },
  contextMenuIgnoreOutsideUntil: 0,
  selectedDesktopTargetId: null,
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
};

export async function fetchJson(url, init) {
  const response = await fetch(url, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init && init.headers ? init.headers : {}),
    },
  });
  if (!response.ok) {
    throw new Error(`request failed: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

export function allVisibleTargets(summary) {
  return (summary && Array.isArray(summary.targets)) ? summary.targets : [];
}

export function targetById(summary, targetId) {
  return allVisibleTargets(summary).find((target) => target.target === targetId) || null;
}

export function shellAppId(summary) {
  return normalizeText(summary && summary.app && summary.app.id) || SHELL_APP_ID;
}

export function targetTitle(summary, targetId) {
  const target = targetById(summary, targetId);
  return target ? target.title : targetId;
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
  const stored = loadShellLayoutState();
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
  for (const [index, app] of allVisibleTargets(summary).entries()) {
    const defaultPosition = defaultDesktopPosition(index);
    const storedPosition = stored && stored.desktop ? stored.desktop[app.target] : null;
    const position = clampDesktopPosition(normalizeDesktopPosition(storedPosition, defaultPosition));
    shellState.shellLayoutState.desktop[app.target] = position;
    if (!storedPosition || !positionsEqual(storedPosition, position)) {
      changed = true;
    }
  }

  if (changed) {
    saveShellLayoutState();
  }
}

function loadShellLayoutState() {
  try {
    const raw = window.localStorage.getItem(SHELL_LAYOUT_STORAGE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") {
      return null;
    }
    return parsed;
  } catch (_error) {
    return null;
  }
}

export function saveShellLayoutState() {
  try {
    window.localStorage.setItem(
      SHELL_LAYOUT_STORAGE_KEY,
      JSON.stringify(shellState.shellLayoutState),
    );
  } catch (_error) {
    // Storage can be unavailable in private or transient contexts.
  }
}

export function loadShellSessionState() {
  try {
    const raw = window.localStorage.getItem(SHELL_SESSION_STORAGE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") {
      return null;
    }
    return parsed;
  } catch (_error) {
    return null;
  }
}

export function saveShellSessionState(session) {
  try {
    window.localStorage.setItem(
      SHELL_SESSION_STORAGE_KEY,
      JSON.stringify(session),
    );
  } catch (_error) {
    // Storage can be unavailable in private or transient contexts.
  }
}

export function clearShellSessionState() {
  try {
    window.localStorage.removeItem(SHELL_SESSION_STORAGE_KEY);
  } catch (_error) {
    // Storage can be unavailable in private or transient contexts.
  }
}

export function initializeRecentTargets(summary) {
  const stored = loadRecentTargets();
  const normalized = normalizeRecentTargets(stored, summary);
  shellState.recentTargetIds = normalized;
  if (!arrayEquals(stored, normalized)) {
    saveRecentTargets();
  }
}

function loadRecentTargets() {
  try {
    const raw = window.localStorage.getItem(RECENT_TARGETS_STORAGE_KEY);
    if (!raw) {
      return [];
    }
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch (_error) {
    return [];
  }
}

function saveRecentTargets() {
  try {
    window.localStorage.setItem(
      RECENT_TARGETS_STORAGE_KEY,
      JSON.stringify(shellState.recentTargetIds),
    );
  } catch (_error) {
    // Storage can be unavailable in private or transient contexts.
  }
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

function defaultDesktopPosition(index) {
  const rect = desktop.getBoundingClientRect();
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

export function clampDesktopPosition(position) {
  const rect = desktop.getBoundingClientRect();
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

export function desktopPositionForTarget(targetId, defaultIndex) {
  const stored = shellState.shellLayoutState.desktop[targetId];
  if (stored) {
    return clampDesktopPosition(stored);
  }
  const defaultPosition = defaultDesktopPosition(defaultIndex);
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
    .filter((target) => isTargetOnDesktop(target.target));
  for (const [index, target] of desktopTargets.entries()) {
    changed = setDesktopPosition(target.target, defaultDesktopPosition(index)) || changed;
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
  for (const [index, app] of allVisibleTargets(shellState.currentSummary).entries()) {
    const next = clampDesktopPosition(desktopPositionForTarget(app.target, index));
    if (!positionsEqual(shellState.shellLayoutState.desktop[app.target], next)) {
      shellState.shellLayoutState.desktop[app.target] = next;
      changed = true;
    }
  }
  return changed;
}

export function mountGlyph(container, targetId, forcedTone) {
  const tone = forcedTone || glyphTone(targetId);
  container.dataset.tone = tone;
  container.innerHTML = glyphSvg(targetId);
}

export function glyphTone(targetId) {
  if (targetId === SYSTEM_APP_ID) {
    return "system";
  }
  if (targetId === "inbox") {
    return "docs";
  }
  if (targetId.includes("file")) {
    return "docs";
  }
  if (targetId.includes("room")) {
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
