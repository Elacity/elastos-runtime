import {
  desktop,
  desktopShortcuts,
  desktopContextMenu,
  launcher,
  launcherGrid,
  launcherEmptyState,
  launcherSearch,
  launcherToggleButton,
  toolbarInboxButton,
  toolbarInboxCount,
  homeNotificationToast,
  homeNotificationTitle,
  homeNotificationBody,
  homeNotificationAction,
  homeNotificationDismiss,
  taskbarTargets,
  shortcutTemplate,
  launcherItemTemplate,
  taskbarItemTemplate,
  ICON_DRAG_THRESHOLD,
  shellState,
  allVisibleTargets,
  targetById,
  targetTitle,
  desktopLabelForTarget,
  desktopPositionForTarget,
  setDesktopPosition,
  setDesktopIconsVisible,
  isTargetOnDesktop,
  addTargetToDesktop,
  removeTargetFromDesktop,
  setDesktopLabel,
  autoArrangeDesktopIcons,
  pinTargetToTaskbar,
  unpinTargetFromTaskbar,
  isTargetPinnedToTaskbar,
  clampDesktopPosition,
  snapDesktopPosition,
  saveShellLayoutState,
  mountGlyph,
  clamp,
  pointInRect,
  CONTEXT_MENU_IGNORE_OUTSIDE_MS,
  desktopObjects,
  desktopObjectEntryId,
  desktopObjectByEntryId,
  desktopEntryExists,
  trapTabWithin,
} from "./shell-core.js?v=home-20260717b";
import {
  browserWindowEntries,
  sortWindowEntriesByZOrder,
  browserWindowEntriesForTarget,
  browserWindowCount,
  browserWindowDisplayTitle,
  activeBrowserTargetId,
  openTarget,
  handleTaskbarTargetClick,
  showAllTargetWindows,
  hideAllTargetWindows,
  closeAllTargetWindows,
  focusWindow,
} from "./shell-windows.js?v=home-20260717b";
import { playUiSound } from "./shell-sounds.js?v=home-20260717b";

const DESKTOP_LONG_PRESS_MS = 520;
const DESKTOP_RENAME_BLUR_GUARD_MS = 350;
const HOME_NOTIFICATION_TOAST_MS = 12000;
let homeNotificationToastTimer = null;
let lastHomeNotificationToastId = "";

export function renderDesktop(summary) {
  desktopShortcuts.replaceChildren();
  for (const [index, app] of allVisibleTargets(summary).entries()) {
    if (!isTargetOnDesktop(app.target)) {
      continue;
    }
    const button = shortcutTemplate.content.firstElementChild.cloneNode(true);
    const position = desktopPositionForTarget(app.target, index);
    const label = desktopLabelForTarget(summary, app.target);
    button.dataset.target = app.target;
    button.dataset.desktopEntryId = app.target;
    button.id = `desktop-shortcut-${app.target}`;
    button.style.left = `${position.x}px`;
    button.style.top = `${position.y}px`;
    button.setAttribute("aria-label", desktopShortcutAriaLabel(label));
    button.title = `${label}\nDouble-click or press Enter to open`;
    mountGlyph(button.querySelector(".desktop-shortcut-icon"), app.target);
    button.querySelector(".desktop-shortcut-title").textContent = label;
    attachTargetIconInteractions(button, app.target, "desktop");
    desktopShortcuts.appendChild(button);
  }
  const desktopObjectOffset = allVisibleTargets(summary).length;
  for (const [index, object] of desktopObjects(summary).entries()) {
    const entryId = desktopObjectEntryId(object);
    const button = shortcutTemplate.content.firstElementChild.cloneNode(true);
    const position = desktopPositionForTarget(entryId, desktopObjectOffset + index);
    const label = object.name;
    button.dataset.desktopEntryId = entryId;
    button.dataset.objectUri = object.uri;
    button.id = desktopShortcutIdForEntry(entryId);
    button.style.left = `${position.x}px`;
    button.style.top = `${position.y}px`;
    button.setAttribute("aria-label", desktopShortcutAriaLabel(label));
    button.title = `${label}\nDouble-click or press Enter to open`;
    mountGlyph(button.querySelector(".desktop-shortcut-icon"), desktopObjectGlyphId(object));
    button.querySelector(".desktop-shortcut-title").textContent = label;
    attachDesktopObjectInteractions(button, entryId);
    desktopShortcuts.appendChild(button);
  }
  syncDesktopIconsVisibility();
  updateDesktopSelectionState();
  syncDesktopFirstRunHint();
}

/* First-contact teaching (shown once per browser): after the desktop stopped
   carrying app icons, a fresh user must still get a visible answer to "where
   are my apps". The hint sits above the dock and retires forever the first
   time the launcher opens or the hint itself is clicked. */
const DESKTOP_HINT_KEY = "elastos.shell.desktopHintDone";
let desktopHintBound = false;

function desktopFirstRunHintNode() {
  return document.querySelector("#desktop-first-run-hint");
}

function syncDesktopFirstRunHint() {
  const hint = desktopFirstRunHintNode();
  if (!hint) {
    return;
  }
  let done = false;
  try {
    done = localStorage.getItem(DESKTOP_HINT_KEY) === "1";
  } catch (_error) {
    done = true;
  }
  const show = !done && shellState.shellLayoutState.desktopApps.length === 0;
  hint.hidden = !show;
  launcherToggleButton?.classList.toggle("launcher-first-run", show);
  if (show && !desktopHintBound) {
    desktopHintBound = true;
    hint.addEventListener("click", () => dismissDesktopFirstRunHint());
  }
}

function dismissDesktopFirstRunHint() {
  const hint = desktopFirstRunHintNode();
  if (!hint || hint.hidden) {
    return;
  }
  try {
    localStorage.setItem(DESKTOP_HINT_KEY, "1");
  } catch (_error) {
    // Session-only dismissal still applies below.
  }
  hint.hidden = true;
  launcherToggleButton?.classList.remove("launcher-first-run");
}

function desktopShortcutIdForEntry(entryId) {
  return `desktop-shortcut-${encodeURIComponent(entryId).replaceAll("%", "_")}`;
}

function isTrashDesktopObject(object) {
  return object?.metadata?.system_kind === "trash" || object?.uri?.endsWith("/.Trash");
}

function desktopObjectGlyphId(object) {
  if (isTrashDesktopObject(object)) {
    return object?.metadata?.empty === false ? "trash-full" : "trash";
  }
  return object.kind === "directory" ? "file-folder" : "documents";
}

function syncDesktopIconsVisibility() {
  const visible = shellState.shellLayoutState.desktopIconsVisible !== false;
  desktopShortcuts.hidden = !visible;
  desktopShortcuts.setAttribute("aria-hidden", visible ? "false" : "true");
}

function selectDesktopTarget(entryId) {
  shellState.marqueeSelection.clear();
  if (shellState.selectedDesktopTargetId === entryId) {
    updateDesktopSelectionState();
    focusDesktopSelectionSurface();
    return;
  }
  shellState.selectedDesktopTargetId = entryId;
  updateDesktopSelectionState();
  focusDesktopSelectionSurface();
}

export function clearDesktopSelection() {
  if (!shellState.selectedDesktopTargetId && shellState.marqueeSelection.size === 0) {
    return;
  }
  shellState.selectedDesktopTargetId = null;
  shellState.marqueeSelection.clear();
  updateDesktopSelectionState();
}

/* Multi-select (macOS): the full selection is the marquee set plus the
   primary. Cmd/Ctrl+click toggles membership; the last icon added becomes the
   primary so Enter / arrow keys / context menu keep an anchor. */
function selectedDesktopEntryIds() {
  const ids = new Set(shellState.marqueeSelection);
  if (shellState.selectedDesktopTargetId) {
    ids.add(shellState.selectedDesktopTargetId);
  }
  return ids;
}

function entryInDesktopSelection(entryId) {
  return selectedDesktopEntryIds().has(entryId);
}

function toggleDesktopSelection(entryId) {
  const ids = selectedDesktopEntryIds();
  if (ids.has(entryId)) {
    ids.delete(entryId);
  } else {
    ids.add(entryId);
  }
  const list = [...ids];
  shellState.selectedDesktopTargetId = ids.has(entryId)
    ? entryId
    : list[list.length - 1] || null;
  shellState.marqueeSelection.clear();
  for (const id of list) {
    if (id !== shellState.selectedDesktopTargetId) {
      shellState.marqueeSelection.add(id);
    }
  }
  updateDesktopSelectionState();
  focusDesktopSelectionSurface();
}

export function selectAllDesktopIcons() {
  shellState.marqueeSelection.clear();
  let last = null;
  for (const shortcut of desktopShortcuts.querySelectorAll(".desktop-shortcut")) {
    const entryId = shortcut.dataset.desktopEntryId || shortcut.dataset.target || "";
    if (!entryId) {
      continue;
    }
    shellState.marqueeSelection.add(entryId);
    last = entryId;
  }
  if (last) {
    shellState.marqueeSelection.delete(last);
    shellState.selectedDesktopTargetId = last;
  }
  updateDesktopSelectionState();
  focusDesktopSelectionSurface();
}

function focusDesktopSelectionSurface() {
  if (document.activeElement === desktopShortcuts) {
    return;
  }
  desktopShortcuts.focus({ preventScroll: true });
}

function updateDesktopSelectionState() {
  let activeDescendant = "";
  if (
    shellState.selectedDesktopTargetId &&
    shellState.currentSummary &&
    !desktopEntryExists(shellState.currentSummary, shellState.selectedDesktopTargetId)
  ) {
    shellState.selectedDesktopTargetId = null;
  }
  for (const shortcut of desktopShortcuts.querySelectorAll(".desktop-shortcut")) {
    const entryId = shortcut.dataset.desktopEntryId || shortcut.dataset.target || "";
    const selected =
      entryId === shellState.selectedDesktopTargetId ||
      shellState.marqueeSelection.has(entryId);
    shortcut.classList.toggle("selected", selected);
    shortcut.setAttribute("aria-selected", selected ? "true" : "false");
    if (entryId === shellState.selectedDesktopTargetId) {
      activeDescendant = shortcut.id;
    }
  }
  if (activeDescendant) {
    desktopShortcuts.setAttribute("aria-activedescendant", activeDescendant);
    desktopShortcuts.dataset.selectedTarget = shellState.selectedDesktopTargetId;
    return;
  }
  desktopShortcuts.removeAttribute("aria-activedescendant");
  delete desktopShortcuts.dataset.selectedTarget;
}

// Spatial (nearest-in-direction) selection so arrows behave sensibly on a
// free-form icon grid, not just DOM order.
export function moveDesktopSelection(direction) {
  const shortcuts = Array.from(desktopShortcuts.querySelectorAll(".desktop-shortcut")).filter(
    (node) => !node.hidden,
  );
  if (shortcuts.length === 0) {
    return false;
  }
  const entryIdOf = (node) => node.dataset.desktopEntryId || node.dataset.target || "";
  const centerOf = (node) => {
    const rect = node.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  };
  const current = shortcuts.find((node) => entryIdOf(node) === shellState.selectedDesktopTargetId);
  if (!current) {
    const first = shortcuts
      .map((node) => ({ node, center: centerOf(node) }))
      .sort((a, b) => a.center.y - b.center.y || a.center.x - b.center.x)[0];
    selectDesktopTarget(entryIdOf(first.node));
    return true;
  }
  const origin = centerOf(current);
  let best = null;
  let bestScore = Infinity;
  for (const node of shortcuts) {
    if (node === current) {
      continue;
    }
    const center = centerOf(node);
    const dx = center.x - origin.x;
    const dy = center.y - origin.y;
    const along =
      direction === "left" ? -dx : direction === "right" ? dx : direction === "up" ? -dy : dy;
    if (along <= 1) {
      continue;
    }
    const cross = direction === "left" || direction === "right" ? Math.abs(dy) : Math.abs(dx);
    // Weight cross-axis drift heavier so arrows track rows/columns.
    const score = along + cross * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = node;
    }
  }
  if (!best) {
    return false;
  }
  selectDesktopTarget(entryIdOf(best));
  return true;
}

export function renderTaskbar(summary) {
  taskbarTargets.replaceChildren();
  const pinnedIds = new Set(shellState.shellLayoutState.taskbar);
  let separatorInserted = false;
  for (const targetId of visibleTaskbarTargets(summary)) {
    const app = targetById(summary, targetId);
    if (!app) {
      continue;
    }
    if (
      !pinnedIds.has(targetId) &&
      !separatorInserted &&
      taskbarTargets.childElementCount > 0
    ) {
      const separator = document.createElement("span");
      separator.className = "taskbar-separator";
      separator.setAttribute("aria-hidden", "true");
      taskbarTargets.appendChild(separator);
      separatorInserted = true;
    }
    const entry = taskbarItemTemplate.content.firstElementChild.cloneNode(true);
    const button = entry.querySelector(".taskbar-item");
    const openCount = browserWindowCount(app.target);
    button.dataset.target = app.target;
    button.dataset.label = app.title;
    mountGlyph(button.querySelector(".taskbar-item-icon"), app.target);
    button.dataset.openWindows = String(openCount);
    attachTargetIconInteractions(button, app.target, "taskbar");
    syncTaskbarGroupButton(entry, app.target, app.title, openCount);
    taskbarTargets.appendChild(entry);
  }
  appendTaskbarTrash(summary);
  updateTaskbarState();
}

/* Trash anchors the right end of the dock, past its own divider (the macOS
   position). It is the same desktop Trash object: the glyph fills when it
   holds items, click opens it in Library, right-click offers Empty Trash. */
function appendTaskbarTrash(summary) {
  const trashObject = desktopObjects(summary).find(isTrashDesktopObject);
  if (!trashObject) {
    return;
  }
  if (taskbarTargets.childElementCount > 0) {
    const separator = document.createElement("span");
    separator.className = "taskbar-separator";
    separator.setAttribute("aria-hidden", "true");
    taskbarTargets.appendChild(separator);
  }
  const entryId = desktopObjectEntryId(trashObject);
  const entry = taskbarItemTemplate.content.firstElementChild.cloneNode(true);
  const button = entry.querySelector(".taskbar-item");
  const empty = trashObject.metadata?.empty !== false;
  button.dataset.label = "Trash";
  button.setAttribute("aria-label", empty ? "Trash. Empty." : "Trash. Contains items.");
  mountGlyph(button.querySelector(".taskbar-item-icon"), empty ? "trash" : "trash-full");
  button.addEventListener("click", () => {
    openDesktopObject(entryId);
  });
  button.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    event.stopPropagation();
    const anchor = contextMenuAnchorPoint(event, button);
    openDesktopContextMenu(anchor.x, anchor.y, {
      kind: "desktop-object",
      entryId,
      source: "taskbar",
    });
  });
  taskbarTargets.appendChild(entry);
}

function visibleTaskbarTargets(summary) {
  const pinned = shellState.shellLayoutState.taskbar.filter(
    (targetId) => Boolean(targetById(summary, targetId)),
  );
  const openUnpinned = [];
  for (const entry of sortWindowEntriesByZOrder(browserWindowEntries())) {
    if (
      pinned.includes(entry.targetId) ||
      openUnpinned.includes(entry.targetId) ||
      !targetById(summary, entry.targetId)
    ) {
      continue;
    }
    openUnpinned.push(entry.targetId);
  }
  return [...pinned, ...openUnpinned];
}

export function updateTaskbarState() {
  for (const button of taskbarTargets.querySelectorAll(".taskbar-item[data-target]")) {
    updateTaskbarButton(button, button.dataset.target);
  }
}

function commitTaskbarLayoutChange() {
  saveShellLayoutState();
  if (!shellState.currentSummary) {
    return;
  }
  renderTaskbar(shellState.currentSummary);
}

function rerenderShellLayout() {
  if (!shellState.currentSummary) {
    return;
  }
  renderDesktop(shellState.currentSummary);
  renderTaskbar(shellState.currentSummary);
}

function updateTaskbarButton(button, targetId) {
  const openCount = browserWindowCount(targetId);
  const isActive = activeBrowserTargetId() === targetId;
  const appInfo = shellState.currentSummary ? targetById(shellState.currentSummary, targetId) : null;
  button.classList.toggle("open", openCount > 0);
  button.classList.toggle("active", isActive);
  button.dataset.open = openCount > 0 ? "true" : "false";
  button.dataset.active = isActive ? "true" : "false";
  button.dataset.openWindows = String(openCount);
  if (appInfo && shellState.currentSummary) {
    button.dataset.label = appInfo.title;
    button.setAttribute("aria-label", taskbarItemAriaLabel(appInfo.title, openCount, isActive));
  }
  const entry = button.closest(".taskbar-entry");
  if (entry && appInfo) {
    syncTaskbarGroupButton(entry, targetId, appInfo.title, openCount);
  }
}

function syncTaskbarGroupButton(entry, targetId, title, openCount) {
  const countButton = entry.querySelector(".taskbar-window-count");
  if (!countButton) {
    return;
  }
  countButton.hidden = openCount <= 1;
  countButton.textContent = String(openCount);
  countButton.title = `Manage ${title} windows`;
  countButton.setAttribute("aria-label", `Manage ${title}. ${openCount} windows open.`);
  const openGroupMenu = (event) => {
    event.preventDefault();
    event.stopPropagation();
    const rect = countButton.getBoundingClientRect();
    openDesktopContextMenu(rect.right, rect.bottom, {
      kind: "target",
      targetId,
      source: "taskbar",
    });
  };
  countButton.onpointerdown = (event) => {
    if (event.button !== 0) {
      return;
    }
    openGroupMenu(event);
  };
  countButton.onclick = (event) => {
    if (event.detail !== 0) {
      return;
    }
    openGroupMenu(event);
  };
}

export function renderLauncher(summary) {
  const query = launcherSearch.value;
  launcherGrid.replaceChildren();
  /* One continuous grid (macOS Apps panel) — section order still puts
     running, then recent, then the rest first, but without header breaks. */
  const grid = document.createElement("div");
  grid.className = "launcher-group-grid";
  for (const section of launcherSections(summary)) {
    for (const target of section.targets) {
      grid.appendChild(createLauncherCard(target));
    }
  }
  launcherGrid.appendChild(grid);
  filterLauncherItems(query);
}

function launcherSections(summary) {
  const runningIds = runningLauncherTargetIds(summary);
  const runningSet = new Set(runningIds);
  const recentIds = shellState.recentTargetIds.filter(
    (targetId) => !runningSet.has(targetId) && targetById(summary, targetId),
  );
  const recentSet = new Set(recentIds);
  const allIds = allVisibleTargets(summary)
    .map((app) => app.target)
    .filter((targetId) => !runningSet.has(targetId) && !recentSet.has(targetId));
  const allTargets = allIds.map((targetId) => targetById(summary, targetId)).filter(Boolean);
  return [
    {
      label: "Running",
      targets: runningIds.map((targetId) => targetById(summary, targetId)).filter(Boolean),
    },
    {
      label: "Recent",
      targets: recentIds.map((targetId) => targetById(summary, targetId)).filter(Boolean),
    },
    {
      label: "Apps",
      targets: allTargets.filter((app) => launchTargetKind(app) === "app"),
    },
    {
      label: "Library",
      targets: allTargets.filter((app) => launchTargetKind(app) === "object"),
    },
  ];
}

function runningLauncherTargetIds(summary) {
  const targetIds = [];
  for (const entry of sortWindowEntriesByZOrder(browserWindowEntries())) {
    if (!targetById(summary, entry.targetId) || targetIds.includes(entry.targetId)) {
      continue;
    }
    targetIds.push(entry.targetId);
  }
  return targetIds;
}

function createLauncherCard(app) {
  const card = launcherItemTemplate.content.firstElementChild.cloneNode(true);
  card.dataset.target = app.target;
  card.dataset.search = launcherSearchText(app);
  card.title = app.description;
  mountGlyph(card.querySelector(".launcher-item-icon"), app.target);
  card.querySelector(".launcher-card-title").textContent = app.title;
  card.setAttribute("aria-label", `Open ${app.title}`);
  card.setAttribute("aria-selected", "false");
  attachTargetIconInteractions(card, app.target, "launcher");
  return card;
}

function launchTargetKind(app) {
  return app && app.target_kind === "object" ? "object" : "app";
}

function desktopShortcutAriaLabel(title) {
  return `${title}. Click to select. Double-click or press Enter to open. On touch, tap to open and long-press for options.`;
}

function taskbarItemAriaLabel(title, openCount, isActive) {
  const countLabel = openCount === 1 ? "1 window open" : `${openCount} windows open`;
  if (isActive) {
    return `${title}. ${countLabel}. Active in taskbar.`;
  }
  return `${title}. ${countLabel}.`;
}

function launcherSearchText(app) {
  const title = typeof app.title === "string" ? app.title.trim() : "";
  const description = typeof app.description === "string" ? app.description.trim() : "";
  return `${title} ${description} ${app.target}`.toLowerCase();
}

function visibleLauncherItems() {
  return Array.from(launcherGrid.querySelectorAll(".launcher-card")).filter((item) => !item.hidden);
}

function setSelectedLauncherTarget(targetId) {
  shellState.selectedLauncherTargetId = targetId || null;
  updateLauncherSelectionState();
}

function updateLauncherSelectionState() {
  for (const card of launcherGrid.querySelectorAll(".launcher-card")) {
    const selected =
      Boolean(shellState.selectedLauncherTargetId) &&
      card.dataset.target === shellState.selectedLauncherTargetId &&
      !card.hidden;
    card.classList.toggle("selected", selected);
    card.setAttribute("aria-selected", selected ? "true" : "false");
  }
}

function ensureLauncherSelection(preferredTargetId) {
  const visible = visibleLauncherItems();
  if (visible.length === 0) {
    setSelectedLauncherTarget(null);
    return;
  }
  const preferred = preferredTargetId
    ? visible.find((item) => item.dataset.target === preferredTargetId)
    : null;
  if (preferred) {
    setSelectedLauncherTarget(preferred.dataset.target);
    return;
  }
  const existing = shellState.selectedLauncherTargetId
    ? visible.find((item) => item.dataset.target === shellState.selectedLauncherTargetId)
    : null;
  if (existing) {
    updateLauncherSelectionState();
    return;
  }
  const activeTargetId = activeBrowserTargetId();
  const active = activeTargetId
    ? visible.find((item) => item.dataset.target === activeTargetId)
    : null;
  setSelectedLauncherTarget((active || visible[0]).dataset.target);
}

export function moveLauncherSelection(delta) {
  const visible = visibleLauncherItems();
  if (visible.length === 0) {
    setSelectedLauncherTarget(null);
    return;
  }
  const currentIndex = visible.findIndex(
    (item) => item.dataset.target === shellState.selectedLauncherTargetId,
  );
  const nextIndex = currentIndex === -1
    ? (delta > 0 ? 0 : visible.length - 1)
    : clamp(currentIndex + delta, 0, visible.length - 1);
  setSelectedLauncherTarget(visible[nextIndex].dataset.target);
  visible[nextIndex].scrollIntoView({ block: "nearest" });
}

export function openSelectedLauncherTarget() {
  if (!shellState.selectedLauncherTargetId) {
    return;
  }
  hideLauncher();
  openTarget(shellState.selectedLauncherTargetId);
}

export function refreshLauncherIfVisible() {
  if (!shellState.currentSummary || launcher.hidden) {
    return;
  }
  renderLauncher(shellState.currentSummary);
}

function attachTargetIconInteractions(node, targetId, source) {
  node.addEventListener("click", (event) => {
    if (node.dataset.suppressClick === "true") {
      delete node.dataset.suppressClick;
      return;
    }
    if (source === "desktop") {
      if (event.metaKey || event.ctrlKey) {
        toggleDesktopSelection(targetId);
        return;
      }
      if (shouldOpenDesktopShortcutFromClick(node, event)) {
        openTarget(targetId);
        return;
      }
      // Slow click: a second click on an already-selected icon's label starts
      // rename, unless a double-click (open) lands first. Selection state is
      // sampled at pointerdown, before beginTargetDrag reselects the icon.
      const wasSelected = node.dataset.wasSelectedOnPointerdown === "true";
      delete node.dataset.wasSelectedOnPointerdown;
      if (
        wasSelected &&
        shellState.selectedDesktopTargetId === targetId &&
        !node.classList.contains("editing") &&
        event.target.closest(".desktop-shortcut-title")
      ) {
        clearSlowClickRename();
        slowClickRenameTimer = window.setTimeout(() => {
          slowClickRenameTimer = null;
          if (shellState.selectedDesktopTargetId === targetId) {
            startDesktopRename(targetId);
          }
        }, SLOW_CLICK_RENAME_MS);
        return;
      }
      selectDesktopTarget(targetId);
      return;
    }
    if (source === "taskbar") {
      if (event.target.closest(".taskbar-window-count")) {
        return;
      }
      if (browserWindowCount(targetId) === 0) {
        startDockLaunchBounce(node);
      }
      handleTaskbarTargetClick(targetId);
      return;
    }
    if (source === "launcher") {
      hideLauncher();
    }
    openTarget(targetId);
  });

  if (source === "desktop") {
    node.addEventListener("dblclick", () => {
      clearSlowClickRename();
      selectDesktopTarget(targetId);
      openTarget(targetId);
    });
    node.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      selectDesktopTarget(targetId);
      openTarget(targetId);
    });
  }

  if (source === "launcher") {
    node.addEventListener("pointerenter", () => {
      setSelectedLauncherTarget(targetId);
    });
    node.addEventListener("focus", () => {
      setSelectedLauncherTarget(targetId);
    });
  }

  if (source === "desktop") {
    node.addEventListener("focus", () => {
      selectDesktopTarget(targetId);
    });
  }

  if (source === "desktop" || source === "taskbar") {
    node.addEventListener("pointerdown", (event) => {
      if (node.classList.contains("editing")) {
        return;
      }
      node.dataset.lastPointerType = event.pointerType || "";
      maybeStartLongPressGesture(event, targetId, source, node);
      beginTargetDrag(event, targetId, source, node);
    });
  }

  node.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (
      source === "desktop" &&
      entryInDesktopSelection(targetId) &&
      selectedDesktopEntryIds().size > 1
    ) {
      const anchor = contextMenuAnchorPoint(event, node);
      openDesktopContextMenu(anchor.x, anchor.y, { kind: "desktop-group" });
      return;
    }
    if (source === "desktop") {
      selectDesktopTarget(targetId);
    }
    if (source === "launcher") {
      setSelectedLauncherTarget(targetId);
    }
    const anchor = contextMenuAnchorPoint(event, node);
    openDesktopContextMenu(anchor.x, anchor.y, {
      kind: "target",
      targetId,
      source,
      keepLauncherOpen: source === "launcher",
    });
  });
}

function attachDesktopObjectInteractions(node, entryId) {
  node.addEventListener("click", (event) => {
    if (event.metaKey || event.ctrlKey) {
      toggleDesktopSelection(entryId);
      return;
    }
    if (shouldOpenDesktopShortcutFromClick(node, event)) {
      openDesktopObject(entryId);
      return;
    }
    selectDesktopTarget(entryId);
  });
  node.addEventListener("dblclick", (event) => {
    if (event.metaKey || event.ctrlKey) {
      return;
    }
    selectDesktopTarget(entryId);
    openDesktopObject(entryId);
  });
  node.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    selectDesktopTarget(entryId);
    openDesktopObject(entryId);
  });
  node.addEventListener("focus", () => {
    selectDesktopTarget(entryId);
  });
  node.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) {
      return;
    }
    node.dataset.lastPointerType = event.pointerType || "";
    maybeStartLongPressGesture(event, entryId, "desktop-object", node);
    beginTargetDrag(event, entryId, "desktop-object", node);
    if (
      !isTouchLikePointer(event) &&
      !(event.metaKey || event.ctrlKey) &&
      !(entryInDesktopSelection(entryId) && selectedDesktopEntryIds().size > 1)
    ) {
      selectDesktopTarget(entryId);
    }
  });
  node.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (entryInDesktopSelection(entryId) && selectedDesktopEntryIds().size > 1) {
      const anchor = contextMenuAnchorPoint(event, node);
      openDesktopContextMenu(anchor.x, anchor.y, { kind: "desktop-group" });
      return;
    }
    selectDesktopTarget(entryId);
    const anchor = contextMenuAnchorPoint(event, node);
    openDesktopContextMenu(anchor.x, anchor.y, {
      kind: "desktop-object",
      entryId,
      source: "desktop",
    });
  });
}

// Keyboard-invoked contextmenu events (Shift+F10 / Menu key) arrive with
// (0,0) coordinates — anchor the menu to the element instead.
function contextMenuAnchorPoint(event, node) {
  if (event.clientX > 0 || event.clientY > 0) {
    return { x: event.clientX, y: event.clientY };
  }
  const rect = node.getBoundingClientRect();
  return {
    x: rect.left + rect.width / 2,
    y: rect.top + rect.height / 2,
  };
}

function openDesktopObject(entryId) {
  const object = desktopObjectByEntryId(shellState.currentSummary, entryId);
  if (!object || !canOpenDesktopObject(object)) {
    return false;
  }
  openFileObject(object);
  return true;
}

/* One canonical "open this file object" path — desktop double-click and
   Spotlight activation both land here. */
export function openFileObject(object) {
  if (object.kind === "directory") {
    openTarget("library", { query: { uri: object.uri } });
    return;
  }
  const viewer = desktopObjectViewer(object);
  openTarget(viewer, {
    query: {
      objectUri: object.uri,
      uri: object.uri,
      name: object.name || "",
      mime: object.mime || "application/octet-stream",
    },
  });
}

export function openSelectedDesktopEntry() {
  const entryId = shellState.selectedDesktopTargetId;
  if (!entryId) {
    return false;
  }
  if (entryId.startsWith("object:")) {
    return openDesktopObject(entryId);
  }
  openTarget(entryId);
  return true;
}

function desktopObjectViewer(object) {
  const viewers = Array.isArray(object.viewers) ? object.viewers : [];
  const preferred = viewers.find((viewer) => viewer && viewer.default) || viewers[0];
  return preferred && typeof preferred.id === "string" && preferred.id.trim() !== ""
    ? preferred.id
    : "documents";
}

function parentUri(uri) {
  const clean = String(uri || "").replace(/\/+$/, "");
  const index = clean.lastIndexOf("/");
  return index > "localhost://".length ? clean.slice(0, index) : clean;
}

function hasObjectCapability(object, capability) {
  const capabilities = object && object.capabilities;
  return Array.isArray(capabilities) && capabilities.includes(capability);
}

function canOpenDesktopObject(object) {
  if (!hasObjectCapability(object, "open")) {
    return false;
  }
  return object.kind !== "directory" || hasObjectCapability(object, "list");
}

function canRevealDesktopObject(object) {
  return canOpenDesktopObject(object) || hasObjectCapability(object, "properties");
}

function revealDesktopObject(entryId) {
  const object = desktopObjectByEntryId(shellState.currentSummary, entryId);
  if (!object || !canRevealDesktopObject(object)) {
    return;
  }
  const uri = object.kind === "directory" ? object.uri : parentUri(object.uri);
  openTarget("library", { query: { uri } });
}

function libraryActionForObject(object, action) {
  const uri = object.kind === "directory" ? object.uri : parentUri(object.uri);
  openTarget("library", {
    query: {
      uri,
      objectUri: object.uri,
      action,
    },
  });
}

function shouldOpenDesktopShortcutFromClick(node, event) {
  const pointerType = node.dataset.lastPointerType || "";
  delete node.dataset.lastPointerType;
  if (pointerType === "touch" || pointerType === "pen") {
    return true;
  }
  return (
    !pointerType &&
    event.detail > 0 &&
    window.matchMedia?.("(hover: none), (pointer: coarse)")?.matches
  );
}

function clearDragSelection() {
  window.getSelection?.()?.removeAllRanges();
}

const SLOW_CLICK_RENAME_MS = 620;
let slowClickRenameTimer = null;

function clearSlowClickRename() {
  if (slowClickRenameTimer !== null) {
    window.clearTimeout(slowClickRenameTimer);
    slowClickRenameTimer = null;
  }
}

function beginTargetDrag(event, targetId, source, sourceElement) {
  if (event.button !== 0 || !shellState.currentSummary) {
    return;
  }
  if (sourceElement.classList.contains("editing")) {
    return;
  }
  clearSlowClickRename();
  if (!isTouchLikePointer(event)) {
    clearDragSelection();
  }
  hideDesktopContextMenu();
  const onDesktop = source === "desktop" || source === "desktop-object";
  // Cmd/Ctrl+pointerdown is a selection toggle (handled on click), never a
  // drag, and must not disturb the current multi-selection.
  if (onDesktop && (event.metaKey || event.ctrlKey)) {
    return;
  }
  let groupEntryIds = null;
  if (onDesktop && !isTouchLikePointer(event)) {
    // Pressing an icon that is already part of a multi-selection keeps the
    // group (macOS): the drag moves all of them; a plain click on it later
    // collapses selection via the click handler.
    if (entryInDesktopSelection(targetId) && selectedDesktopEntryIds().size > 1) {
      groupEntryIds = [...selectedDesktopEntryIds()];
      sourceElement.dataset.wasSelectedOnPointerdown = "false";
    } else {
      sourceElement.dataset.wasSelectedOnPointerdown =
        shellState.selectedDesktopTargetId === targetId &&
        shellState.marqueeSelection.size === 0
          ? "true"
          : "false";
      selectDesktopTarget(targetId);
    }
  }
  const rect = sourceElement.getBoundingClientRect();
  shellState.dragState = {
    targetId,
    source,
    sourceElement,
    pointerId: event.pointerId,
    started: false,
    startClientX: event.clientX,
    startClientY: event.clientY,
    pointerType: event.pointerType || "",
    longPressReady: false,
    cancelled: false,
    offsetX: event.clientX - rect.left,
    offsetY: event.clientY - rect.top,
    dropTarget: null,
    ghost: null,
    groupEntryIds,
  };
}

function maybeStartLongPressGesture(event, targetId, source, sourceElement) {
  if ((source !== "desktop" && source !== "desktop-object") || !isTouchLikePointer(event)) {
    clearLongPressGesture();
    return;
  }
  clearLongPressGesture();
  const timeoutId = window.setTimeout(() => {
    const gesture = shellState.longPressState;
    if (
      !gesture ||
      gesture.pointerId !== event.pointerId ||
      gesture.sourceElement !== sourceElement
    ) {
      return;
    }
    sourceElement.dataset.suppressClick = "true";
    if (
      shellState.dragState &&
      shellState.dragState.pointerId === event.pointerId &&
      shellState.dragState.sourceElement === sourceElement
    ) {
      shellState.dragState.longPressReady = true;
    }
    shellState.longPressState = null;
    selectDesktopTarget(targetId);
    openDesktopContextMenu(gesture.clientX, gesture.clientY, {
      kind: source === "desktop-object" ? "desktop-object" : "target",
      targetId: source === "desktop-object" ? undefined : targetId,
      entryId: source === "desktop-object" ? targetId : undefined,
      source,
    });
  }, DESKTOP_LONG_PRESS_MS);
  shellState.longPressState = {
    pointerId: event.pointerId,
    sourceElement,
    startClientX: event.clientX,
    startClientY: event.clientY,
    clientX: event.clientX,
    clientY: event.clientY,
    timeoutId,
  };
}

function clearLongPressGesture() {
  if (!shellState.longPressState) {
    return;
  }
  window.clearTimeout(shellState.longPressState.timeoutId);
  shellState.longPressState = null;
}

function updateLongPressGesture(event) {
  const gesture = shellState.longPressState;
  if (!gesture || gesture.pointerId !== event.pointerId) {
    return;
  }
  if (
    Math.hypot(
      event.clientX - gesture.startClientX,
      event.clientY - gesture.startClientY,
    ) >= ICON_DRAG_THRESHOLD
  ) {
    if (
      shellState.dragState &&
      (shellState.dragState.source === "desktop" ||
        shellState.dragState.source === "desktop-object") &&
      isTouchLikeDragState(shellState.dragState) &&
      !shellState.dragState.longPressReady &&
      !shellState.dragState.started
    ) {
      shellState.dragState.cancelled = true;
      shellState.dragState.sourceElement.dataset.suppressClick = "true";
    }
    clearLongPressGesture();
  }
}

export function continueTargetDrag(event) {
  updateLongPressGesture(event);
  if (!shellState.dragState || event.pointerId !== shellState.dragState.pointerId) {
    return;
  }
  if (shellState.dragState.cancelled) {
    return;
  }
  if (
    (shellState.dragState.source === "desktop" ||
      shellState.dragState.source === "desktop-object") &&
    isTouchLikeDragState(shellState.dragState) &&
    !shellState.dragState.longPressReady
  ) {
    return;
  }

  if (!shellState.dragState.started) {
    const distance = Math.hypot(
      event.clientX - shellState.dragState.startClientX,
      event.clientY - shellState.dragState.startClientY,
    );
    if (distance < dragThresholdForSource(shellState.dragState.source)) {
      return;
    }
    startTargetDrag();
  }

  updateDragGhost(event.clientX, event.clientY);
  updateDragTarget(event.clientX, event.clientY);
}

function dragThresholdForSource(source) {
  return source === "taskbar" ? ICON_DRAG_THRESHOLD * 2 : ICON_DRAG_THRESHOLD;
}

function isTouchLikePointer(event) {
  return event.pointerType === "touch" || event.pointerType === "pen";
}

function isTouchLikeDragState(state) {
  return state.pointerType === "touch" || state.pointerType === "pen";
}

function startTargetDrag() {
  if (!shellState.dragState || shellState.dragState.started || !shellState.currentSummary) {
    return;
  }
  shellState.dragState.started = true;
  hideDesktopContextMenu();
  shellState.dragState.sourceElement.classList.add("drag-source");
  shellState.dragState.sourceElement.dataset.suppressClick = "true";
  try {
    shellState.dragState.sourceElement.setPointerCapture(shellState.dragState.pointerId);
  } catch (_error) {
    // Pointer capture can fail on browsers that do not support it here.
  }
  const dragEntry = dragEntryDescriptor(shellState.dragState.targetId);
  if (!dragEntry) {
    return;
  }
  document.body.classList.add("dragging-target");
  clearDragSelection();
  const ghost = shortcutTemplate.content.firstElementChild.cloneNode(true);
  ghost.classList.add("desktop-shortcut-ghost");
  mountGlyph(ghost.querySelector(".desktop-shortcut-icon"), dragEntry.glyphId);
  ghost.querySelector(".desktop-shortcut-title").textContent = dragEntry.title;
  const group = shellState.dragState.groupEntryIds;
  if (group && group.length > 1) {
    const badge = document.createElement("span");
    badge.className = "desktop-shortcut-ghost-count";
    badge.textContent = String(group.length);
    badge.setAttribute("aria-hidden", "true");
    ghost.appendChild(badge);
  }
  document.body.appendChild(ghost);
  shellState.dragState.ghost = ghost;
}

function dragEntryDescriptor(entryId) {
  const target = targetById(shellState.currentSummary, entryId);
  if (target) {
    return { glyphId: target.target, title: target.title };
  }
  const object = desktopObjectByEntryId(shellState.currentSummary, entryId);
  if (object) {
    return {
      glyphId: desktopObjectGlyphId(object),
      title: object.name,
    };
  }
  return null;
}

function updateDragGhost(clientX, clientY) {
  if (!shellState.dragState || !shellState.dragState.ghost) {
    return;
  }
  shellState.dragState.ghost.style.left = `${clientX - shellState.dragState.offsetX}px`;
  shellState.dragState.ghost.style.top = `${clientY - shellState.dragState.offsetY}px`;
}

function updateDragTarget(clientX, clientY) {
  if (!shellState.dragState) {
    return;
  }
  taskbarTargets.classList.remove("drop-active");
  if (shellState.dragState.source !== "desktop-object") {
    const taskbarTarget = taskbarDropTarget(clientX, clientY);
    if (taskbarTarget) {
      taskbarTargets.classList.add("drop-active");
      shellState.dragState.dropTarget = taskbarTarget;
      return;
    }
  }
  shellState.dragState.dropTarget = desktopDropTarget(clientX, clientY);
}

function taskbarDropTarget(clientX, clientY) {
  const taskbarRect = taskbarTargets.getBoundingClientRect();
  const launcherRect = launcherToggleButton.getBoundingClientRect();
  const left = launcherRect.right + 8;
  const right = Math.max(left + 24, taskbarRect.right + 24);
  if (!pointInRect(clientX, clientY, {
    left,
    top: launcherRect.top - 10,
    right,
    bottom: launcherRect.bottom + 10,
  })) {
    return null;
  }
  return {
    kind: "taskbar",
    index: taskbarInsertionIndex(clientX),
  };
}

function taskbarInsertionIndex(clientX) {
  const pinnedApps = shellState.shellLayoutState.taskbar.filter(
    (targetId) => Boolean(targetById(shellState.currentSummary, targetId)),
  );
  for (let index = 0; index < pinnedApps.length; index += 1) {
    const button = taskbarTargets.querySelector(`[data-target="${pinnedApps[index]}"]`);
    if (!button) {
      continue;
    }
    const rect = button.getBoundingClientRect();
    if (clientX < rect.left + rect.width / 2) {
      return index;
    }
  }
  return pinnedApps.length;
}

function desktopDropTarget(clientX, clientY) {
  const rect = desktop.getBoundingClientRect();
  if (!shellState.dragState || !pointInRect(clientX, clientY, rect)) {
    return null;
  }
  return {
    kind: "desktop",
    position: snapDesktopPosition(shellState.dragState.targetId, {
      x: clientX - rect.left - shellState.dragState.offsetX,
      y: clientY - rect.top - shellState.dragState.offsetY,
    }),
  };
}

export function finishTargetDrag(event) {
  if (shellState.longPressState && event.pointerId === shellState.longPressState.pointerId) {
    clearLongPressGesture();
  }
  if (!shellState.dragState || event.pointerId !== shellState.dragState.pointerId) {
    return;
  }

  const state = shellState.dragState;
  shellState.dragState = null;
  document.body.classList.remove("dragging-target");
  clearDragSelection();
  try {
    state.sourceElement.releasePointerCapture(event.pointerId);
  } catch (_error) {
    // Pointer capture may already be released.
  }

  if (!state.started) {
    return;
  }

  state.sourceElement.classList.remove("drag-source");
  let changed = false;
  if (
    state.dropTarget &&
    state.dropTarget.kind === "taskbar" &&
    state.source !== "desktop-object"
  ) {
    changed = pinTargetToTaskbar(state.targetId, state.dropTarget.index) || changed;
  } else if (state.dropTarget && state.dropTarget.kind === "desktop") {
    if (state.groupEntryIds && state.groupEntryIds.length > 1) {
      changed = moveDesktopGroup(state, state.dropTarget.position) || changed;
    } else {
      changed = setDesktopPosition(state.targetId, state.dropTarget.position) || changed;
    }
    if (state.source === "taskbar") {
      changed = unpinTargetFromTaskbar(state.targetId) || changed;
    }
  }

  if (state.ghost) {
    state.ghost.remove();
  }
  taskbarTargets.classList.remove("drop-active");

  if (changed) {
    saveShellLayoutState();
    rerenderShellLayout();
  }
}

/* Group drag: every selected icon moves by the dragged icon's delta, each
   snapping (or falling back to free-form) independently — the formation is
   preserved, honest to what macOS does. */
function moveDesktopGroup(state, droppedPosition) {
  const anchorNode = state.sourceElement;
  const anchorLeft = parseFloat(anchorNode.style.left) || 0;
  const anchorTop = parseFloat(anchorNode.style.top) || 0;
  const deltaX = droppedPosition.x - anchorLeft;
  const deltaY = droppedPosition.y - anchorTop;
  let changed = setDesktopPosition(state.targetId, droppedPosition);
  for (const entryId of state.groupEntryIds) {
    if (entryId === state.targetId) {
      continue;
    }
    const node = [...desktopShortcuts.querySelectorAll(".desktop-shortcut")].find(
      (candidate) =>
        (candidate.dataset.desktopEntryId || candidate.dataset.target || "") === entryId,
    );
    if (!node) {
      continue;
    }
    const next = snapDesktopPosition(entryId, {
      x: (parseFloat(node.style.left) || 0) + deltaX,
      y: (parseFloat(node.style.top) || 0) + deltaY,
    });
    changed = setDesktopPosition(entryId, next) || changed;
  }
  return changed;
}

/* Rubber-band (marquee) selection on empty desktop. Pointer-driven visual
   selection; the anchor icon (last one swept) becomes the primary selection so
   Enter/context-menu keep working unchanged. */

let marqueeState = null;

export function beginDesktopMarquee(event) {
  if (event.button !== 0 || isTouchLikePointer(event)) {
    return false;
  }
  const rect = desktop.getBoundingClientRect();
  marqueeState = {
    pointerId: event.pointerId,
    originX: event.clientX - rect.left,
    originY: event.clientY - rect.top,
    node: null,
    swept: false,
  };
  return true;
}

export function updateDesktopMarquee(event) {
  if (!marqueeState || event.pointerId !== marqueeState.pointerId) {
    return;
  }
  const rect = desktop.getBoundingClientRect();
  const currentX = clamp(event.clientX - rect.left, 0, rect.width);
  const currentY = clamp(event.clientY - rect.top, 0, rect.height);
  const left = Math.min(marqueeState.originX, currentX);
  const top = Math.min(marqueeState.originY, currentY);
  const width = Math.abs(currentX - marqueeState.originX);
  const height = Math.abs(currentY - marqueeState.originY);
  if (!marqueeState.node) {
    if (Math.hypot(width, height) < ICON_DRAG_THRESHOLD) {
      return;
    }
    const node = document.createElement("div");
    node.className = "desktop-marquee";
    node.setAttribute("aria-hidden", "true");
    desktop.appendChild(node);
    marqueeState.node = node;
    clearDragSelection();
  }
  const node = marqueeState.node;
  node.style.left = `${left}px`;
  node.style.top = `${top}px`;
  node.style.width = `${width}px`;
  node.style.height = `${height}px`;

  const band = {
    left: rect.left + left,
    top: rect.top + top,
    right: rect.left + left + width,
    bottom: rect.top + top + height,
  };
  marqueeState.swept = true;
  shellState.marqueeSelection.clear();
  let primary = null;
  for (const shortcut of desktopShortcuts.querySelectorAll(".desktop-shortcut")) {
    const iconRect = shortcut.getBoundingClientRect();
    const hit =
      iconRect.left < band.right &&
      iconRect.right > band.left &&
      iconRect.top < band.bottom &&
      iconRect.bottom > band.top;
    if (hit) {
      const entryId = shortcut.dataset.desktopEntryId || shortcut.dataset.target || "";
      shellState.marqueeSelection.add(entryId);
      primary = entryId;
    }
  }
  shellState.selectedDesktopTargetId = primary;
  updateDesktopSelectionState();
}

export function finishDesktopMarquee(event) {
  if (!marqueeState || event.pointerId !== marqueeState.pointerId) {
    return;
  }
  const state = marqueeState;
  marqueeState = null;
  state.node?.remove();
  if (state.swept && shellState.selectedDesktopTargetId) {
    focusDesktopSelectionSurface();
  }
}

export function desktopMarqueeActive() {
  return Boolean(marqueeState?.node);
}

export function toggleLauncher() {
  if (launcher.hidden) {
    showLauncher();
  } else {
    hideLauncher();
  }
}

export function showLauncher() {
  dismissDesktopFirstRunHint();
  if (shellState.currentSummary) {
    renderLauncher(shellState.currentSummary);
  }
  syncLauncherVisibility(true);
  ensureLauncherSelection(activeBrowserTargetId());
  if (shouldFocusLauncherSearch()) {
    launcherSearch.focus();
  }
}

export function hideLauncher() {
  const hadFocus = launcher.contains?.(document.activeElement) === true;
  syncLauncherVisibility(false);
  launcherSearch.value = "";
  shellState.selectedLauncherTargetId = null;
  filterLauncherItems("");
  // A closed modal must hand keyboard focus back to its invoker, never drop
  // it on <body>.
  if (hadFocus) {
    launcherToggleButton.focus();
  }
}

function syncLauncherVisibility(isVisible) {
  launcher.hidden = !isVisible;
  launcher.dataset.open = isVisible ? "true" : "false";
  launcher.setAttribute("aria-hidden", isVisible ? "false" : "true");
  shellState.launcherIgnoreOutsideUntil = isVisible
    ? (window.performance ? window.performance.now() : Date.now()) + 350
    : 0;
  launcherToggleButton.setAttribute("aria-expanded", isVisible ? "true" : "false");
}

function shouldFocusLauncherSearch() {
  if (navigator.maxTouchPoints > 0) {
    return false;
  }
  return !window.matchMedia?.("(hover: none), (pointer: coarse)")?.matches;
}

export function openDesktopContextMenu(clientX, clientY, target) {
  if (!target.keepLauncherOpen) {
    hideLauncher();
  }
  shellState.contextMenuTarget = target;
  shellState.contextMenuInvoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  renderContextMenu(target);
  desktopContextMenu.hidden = false;
  shellState.contextMenuOpen = true;
  shellState.contextMenuIgnoreOutsideUntil =
    (window.performance ? window.performance.now() : Date.now()) +
    CONTEXT_MENU_IGNORE_OUTSIDE_MS;

  const menuRect = desktopContextMenu.getBoundingClientRect();
  const left = clamp(clientX, 12, window.innerWidth - menuRect.width - 12);
  const top = clamp(clientY, 42, window.innerHeight - menuRect.height - 12);

  desktopContextMenu.style.left = `${left}px`;
  desktopContextMenu.style.top = `${top}px`;
  contextMenuFocusables()[0]?.focus();
}

export function hideDesktopContextMenu({ restoreFocus = false } = {}) {
  const hadFocus = desktopContextMenu.contains?.(document.activeElement) === true;
  desktopContextMenu.hidden = true;
  shellState.contextMenuOpen = false;
  if (restoreFocus && hadFocus) {
    shellState.contextMenuInvoker?.focus?.();
  }
  shellState.contextMenuInvoker = null;
}

function contextMenuFocusables() {
  return Array.from(desktopContextMenu.querySelectorAll('[role="menuitem"]'));
}

function moveContextMenuFocus(delta) {
  const items = contextMenuFocusables();
  if (items.length === 0) {
    return;
  }
  const index = items.indexOf(document.activeElement);
  const next = index < 0
    ? (delta > 0 ? 0 : items.length - 1)
    : (index + delta + items.length) % items.length;
  items[next].focus();
}

function handleContextMenuKeydown(event) {
  if (event.key === "ArrowDown") {
    event.preventDefault();
    moveContextMenuFocus(1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    moveContextMenuFocus(-1);
  } else if (event.key === "Home") {
    event.preventDefault();
    contextMenuFocusables()[0]?.focus();
  } else if (event.key === "End") {
    event.preventDefault();
    contextMenuFocusables().at(-1)?.focus();
  }
}

function renderContextMenu(target) {
  desktopContextMenu.replaceChildren();
  for (const item of contextMenuItems(target)) {
    if (item.kind === "divider") {
      const divider = document.createElement("div");
      divider.className = "context-menu-divider";
      divider.setAttribute("role", "separator");
      desktopContextMenu.appendChild(divider);
      continue;
    }
    const button = document.createElement("button");
    button.className = "context-menu-item";
    button.type = "button";
    button.dataset.contextAction = item.action;
    button.setAttribute("role", "menuitem");
    button.textContent = item.label;
    desktopContextMenu.appendChild(button);
  }
}

function taskbarPinMenuItem(targetId) {
  return isTargetPinnedToTaskbar(targetId)
    ? { action: "unpin-taskbar", label: "Remove from Taskbar" }
    : { action: "pin-taskbar", label: "Pin to Taskbar" };
}

function appendTargetGroupManagementItems(items, openWindows) {
  if (openWindows.length === 0) {
    return;
  }
  items.push({ kind: "divider" });
  items.push({ action: "show-all-windows", label: "Show All Windows" });
  items.push({ action: "hide-all-windows", label: "Hide All Windows" });
  items.push({ action: "close-all-windows", label: "Close All Windows" });
}

function contextMenuItems(target) {
  if (target.kind === "target") {
    return targetContextMenuItems(target);
  }
  if (target.kind === "desktop-object") {
    return desktopObjectContextMenuItems(target);
  }
  if (target.kind === "desktop-group") {
    const count = selectedDesktopEntryIds().size;
    return [
      { action: "open-desktop-group", label: `Open ${count} Items` },
      { kind: "divider" },
      { action: "clear-desktop-selection", label: "Deselect All" },
    ];
  }
  const iconsVisible = shellState.shellLayoutState.desktopIconsVisible !== false;
  const items = [
    {
      action: "toggle-desktop-icons",
      label: iconsVisible ? "Hide Desktop Icons" : "Show Desktop Icons",
    },
  ];
  if (iconsVisible) {
    items.push({ action: "auto-arrange", label: "Auto-arrange Icons" });
  }
  return items;
}

function desktopObjectContextMenuItems(target) {
  const object = desktopObjectByEntryId(shellState.currentSummary, target.entryId);
  if (!object) {
    return [];
  }
  if (isTrashDesktopObject(object)) {
    const items = [];
    if (canOpenDesktopObject(object)) {
      items.push({ action: "open-desktop-object", label: "Open Trash" });
      items.push({ action: "open-desktop-object-new-window", label: "Open in New Window" });
    }
    if (object.metadata?.empty === false && hasObjectCapability(object, "empty_trash")) {
      items.push({ action: "empty-trash", label: "Empty Trash" });
    }
    if (items.length > 0 && hasObjectCapability(object, "properties")) {
      items.push({ kind: "divider" });
    }
    if (hasObjectCapability(object, "properties")) {
      items.push({ action: "properties-desktop-object", label: "Properties" });
    }
    return items;
  }
  const items = [];
  if (canOpenDesktopObject(object)) {
    items.push({
      action: "open-desktop-object",
      label: object.kind === "directory" ? `Open ${object.name}` : "Open",
    });
  }
  if (object.kind === "directory" && canOpenDesktopObject(object)) {
    items.push({ action: "open-desktop-object-new-window", label: "Open in New Window" });
  }
  if (canRevealDesktopObject(object)) {
    items.push({ action: "reveal-desktop-object", label: "Show in Library" });
  }
  if (items.length > 0 && (hasObjectCapability(object, "download") || hasObjectCapability(object, "properties"))) {
    items.push({ kind: "divider" });
  }
  if (hasObjectCapability(object, "download")) {
    items.push({ action: "download-desktop-object", label: "Download" });
  }
  if (hasObjectCapability(object, "properties")) {
    items.push({ action: "properties-desktop-object", label: "Properties" });
  }
  return items;
}

function targetContextMenuItems(target) {
  const openWindows = sortWindowEntriesByZOrder(browserWindowEntriesForTarget(target.targetId));
  const items = [];
  if (target.source === "taskbar" && openWindows.length > 0) {
    for (const entry of openWindows) {
      items.push({ action: `focus-window:${entry.id}`, label: browserWindowDisplayTitle(entry) });
    }
    items.push({ kind: "divider" });
  }
  items.push({
    action: "open-target",
    label: openWindows.length === 0 && target.source !== "taskbar"
      ? `Open ${targetTitle(shellState.currentSummary, target.targetId)}`
      : "New Window",
  });
  if (target.source === "desktop") {
    items.push({ action: "rename-desktop-icon", label: "Rename" });
  }
  if (target.source === "desktop" || target.source === "launcher") {
    items.push(desktopPinMenuItem(target.targetId));
  }
  items.push(taskbarPinMenuItem(target.targetId));
  appendTargetGroupManagementItems(items, openWindows);
  return items;
}

function desktopPinMenuItem(targetId) {
  return isTargetOnDesktop(targetId)
    ? { action: "remove-desktop-icon", label: "Remove from Desktop" }
    : { action: "add-desktop-icon", label: "Add to Desktop" };
}

export function handleContextAction(action) {
  if (action === "open-desktop-group") {
    for (const entryId of selectedDesktopEntryIds()) {
      if (targetById(shellState.currentSummary, entryId)) {
        openTarget(entryId);
      } else if (desktopObjectByEntryId(shellState.currentSummary, entryId)) {
        openDesktopObject(entryId);
      }
    }
    return;
  }
  if (action === "clear-desktop-selection") {
    clearDesktopSelection();
    return;
  }
  if (shellState.contextMenuTarget.kind === "desktop-object") {
    if (action === "open-desktop-object" || action === "open-desktop-object-new-window") {
      openDesktopObject(shellState.contextMenuTarget.entryId);
      return;
    }
    if (action === "reveal-desktop-object") {
      revealDesktopObject(shellState.contextMenuTarget.entryId);
      return;
    }
    if (action === "download-desktop-object" || action === "properties-desktop-object") {
      const object = desktopObjectByEntryId(
        shellState.currentSummary,
        shellState.contextMenuTarget.entryId,
      );
      const requiredCapability = action === "download-desktop-object" ? "download" : "properties";
      if (object && hasObjectCapability(object, requiredCapability)) {
        libraryActionForObject(
          object,
          action === "download-desktop-object" ? "download" : "properties",
        );
      }
      return;
    }
    if (action === "empty-trash") {
      const object = desktopObjectByEntryId(
        shellState.currentSummary,
        shellState.contextMenuTarget.entryId,
      );
      if (object && isTrashDesktopObject(object) && hasObjectCapability(object, "empty_trash")) {
        playUiSound("trash");
        libraryActionForObject(object, "empty-trash");
      }
      return;
    }
  }
  if (action.startsWith("focus-window:")) {
    focusWindow(action.slice("focus-window:".length), { moveFocus: true });
    return;
  }
  if (action === "toggle-desktop-icons") {
    const iconsVisible = shellState.shellLayoutState.desktopIconsVisible !== false;
    if (setDesktopIconsVisible(!iconsVisible)) {
      if (iconsVisible) {
        clearDesktopSelection();
      }
      syncDesktopIconsVisibility();
    }
    return;
  }
  if (action === "auto-arrange") {
    if (autoArrangeDesktopIcons()) {
      renderDesktop(shellState.currentSummary);
    }
    return;
  }
  if (action === "show-all-windows" && shellState.contextMenuTarget.targetId) {
    showAllTargetWindows(shellState.contextMenuTarget.targetId);
    return;
  }
  if (action === "hide-all-windows" && shellState.contextMenuTarget.targetId) {
    hideAllTargetWindows(shellState.contextMenuTarget.targetId);
    return;
  }
  if (action === "close-all-windows" && shellState.contextMenuTarget.targetId) {
    closeAllTargetWindows(shellState.contextMenuTarget.targetId);
    return;
  }
  if (!shellState.contextMenuTarget.targetId) {
    return;
  }
  if (action === "open-target") {
    if (shellState.contextMenuTarget.source === "launcher") {
      hideLauncher();
    }
    openTarget(shellState.contextMenuTarget.targetId);
    return;
  }
  if (action === "rename-desktop-icon") {
    startDesktopRename(shellState.contextMenuTarget.targetId);
    return;
  }
  if (action === "remove-desktop-icon") {
    if (removeTargetFromDesktop(shellState.contextMenuTarget.targetId)) {
      if (shellState.selectedDesktopTargetId === shellState.contextMenuTarget.targetId) {
        clearDesktopSelection();
      }
      saveShellLayoutState();
      rerenderShellLayout();
      refreshLauncherIfVisible();
    }
    return;
  }
  if (action === "add-desktop-icon") {
    let changed = addTargetToDesktop(shellState.contextMenuTarget.targetId);
    changed = setDesktopIconsVisible(true) || changed;
    if (changed) {
      saveShellLayoutState();
      rerenderShellLayout();
      refreshLauncherIfVisible();
    }
    return;
  }
  if (action === "pin-taskbar") {
    if (
      pinTargetToTaskbar(
        shellState.contextMenuTarget.targetId,
        shellState.shellLayoutState.taskbar.length,
      )
    ) {
      commitTaskbarLayoutChange();
    }
    return;
  }
  if (action === "unpin-taskbar") {
    if (unpinTargetFromTaskbar(shellState.contextMenuTarget.targetId)) {
      commitTaskbarLayoutChange();
    }
  }
}

export function startDesktopRename(targetId) {
  if (!shellState.currentSummary || !targetById(shellState.currentSummary, targetId)) {
    return;
  }
  const shortcut = desktopShortcuts.querySelector(`.desktop-shortcut[data-target="${targetId}"]`);
  if (!shortcut) {
    return;
  }
  cancelDesktopRename();
  shellState.editingDesktopTargetId = targetId;
  shortcut.classList.add("editing");
  const titleNode = shortcut.querySelector(".desktop-shortcut-title");
  const input = document.createElement("input");
  input.className = "desktop-shortcut-rename";
  input.type = "text";
  input.spellcheck = false;
  input.maxLength = 48;
  input.value = desktopLabelForTarget(shellState.currentSummary, targetId);
  input.addEventListener("pointerdown", (event) => {
    event.stopPropagation();
  });
  input.addEventListener("click", (event) => {
    event.stopPropagation();
  });
  titleNode.replaceChildren(input);
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commitDesktopRename(targetId, input.value);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      cancelDesktopRename();
    }
  });
  input.addEventListener("blur", () => {
    const now = window.performance ? window.performance.now() : Date.now();
    const ignoreBlurUntil = Number.parseFloat(input.dataset.ignoreBlurUntil || "0");
    if (Number.isFinite(ignoreBlurUntil) && now < ignoreBlurUntil) {
      window.setTimeout(() => {
        if (shellState.editingDesktopTargetId === targetId) {
          input.focus();
          input.select();
        }
      }, 0);
      return;
    }
    if (shellState.editingDesktopTargetId === targetId) {
      commitDesktopRename(targetId, input.value);
    }
  });
  input.dataset.ignoreBlurUntil = String(
    (window.performance ? window.performance.now() : Date.now()) + DESKTOP_RENAME_BLUR_GUARD_MS,
  );
  input.focus();
  input.select();
}

function commitDesktopRename(targetId, value) {
  if (!shellState.currentSummary) {
    cancelDesktopRename();
    return;
  }
  setDesktopLabel(targetId, value, shellState.currentSummary);
  saveShellLayoutState();
  shellState.editingDesktopTargetId = null;
  renderDesktop(shellState.currentSummary);
  const shortcut = desktopShortcuts.querySelector(`.desktop-shortcut[data-target="${targetId}"]`);
  if (shortcut) {
    shortcut.focus();
  }
}

function cancelDesktopRename() {
  if (!shellState.editingDesktopTargetId || !shellState.currentSummary) {
    shellState.editingDesktopTargetId = null;
    return;
  }
  const targetId = shellState.editingDesktopTargetId;
  shellState.editingDesktopTargetId = null;
  renderDesktop(shellState.currentSummary);
  const shortcut = desktopShortcuts.querySelector(`.desktop-shortcut[data-target="${targetId}"]`);
  if (shortcut) {
    shortcut.focus();
  }
}

export function filterLauncherItems(query) {
  const normalized = query.trim().toLowerCase();
  let visibleCount = 0;
  for (const item of launcherGrid.querySelectorAll(".launcher-card")) {
    item.hidden = normalized !== "" && !item.dataset.search.includes(normalized);
    if (!item.hidden) {
      visibleCount += 1;
    }
  }
  launcherEmptyState.hidden = visibleCount !== 0;
  ensureLauncherSelection();
  if (launcher.hidden) {
    updateLauncherSelectionState();
  }
}

export function renderInboxBadge(summary) {
  const inboxTarget = targetById(summary, "inbox");
  toolbarInboxButton.hidden = !inboxTarget;
  toolbarInboxButton.disabled = !inboxTarget;
  if (!inboxTarget) {
    toolbarInboxCount.hidden = true;
    toolbarInboxCount.textContent = "";
    toolbarInboxButton.title = "";
    toolbarInboxButton.setAttribute("aria-label", "Inbox unavailable");
    return;
  }
  const notifications = summary && summary.notifications ? summary.notifications : {};
  const entries = Array.isArray(notifications.entries) ? notifications.entries : [];
  const semanticCount =
    Number(notifications.attention_count || 0) || Number(notifications.unread_count || 0);
  const badgeCount = Math.max(0, semanticCount || entries.length);
  toolbarInboxCount.hidden = badgeCount === 0;
  toolbarInboxCount.textContent = String(badgeCount);
  toolbarInboxButton.title = badgeCount === 0
    ? "Inbox"
    : `Inbox\n${badgeCount} pending items`;
  toolbarInboxButton.setAttribute(
    "aria-label",
    badgeCount === 0 ? "Open Inbox" : `Open Inbox. ${badgeCount} pending items.`,
  );
}

export function maybeShowWalletApprovalToast(previousSummary, summary) {
  if (!previousSummary || !targetById(summary, "inbox")) {
    return;
  }
  const previousIds = new Set(walletApprovalEntries(previousSummary).map(walletApprovalKey));
  const entry = walletApprovalEntries(summary)
    .find((item) => {
      const key = walletApprovalKey(item);
      return key && key !== lastHomeNotificationToastId && !previousIds.has(key);
    });
  if (!entry) {
    return;
  }
  showHomeNotificationToast(entry);
}

function walletApprovalEntries(summary) {
  const entries = Array.isArray(summary?.notifications?.entries)
    ? summary.notifications.entries
    : [];
  return entries.filter((entry) => {
    const actionId = entry?.action_ref?.action_id;
    return entry?.kind === "wallet_approval_request"
      && typeof actionId === "string"
      && actionId.startsWith("wallet-approve-request:");
  });
}

function walletApprovalKey(entry) {
  return String(entry?.id || entry?.action_ref?.action_id || "");
}

function showHomeNotificationToast(entry) {
  if (
    !homeNotificationToast ||
    !homeNotificationTitle ||
    !homeNotificationBody ||
    !homeNotificationAction ||
    !homeNotificationDismiss
  ) {
    return;
  }
  bindHomeNotificationToast();
  lastHomeNotificationToastId = walletApprovalKey(entry);
  homeNotificationTitle.textContent = entry.title || "Wallet approval request";
  homeNotificationBody.textContent = entry.body || "A capsule requests wallet approval.";
  homeNotificationToast.hidden = false;
  homeNotificationToast.setAttribute("aria-hidden", "false");
  playUiSound("notification");
  window.clearTimeout(homeNotificationToastTimer);
  homeNotificationToastTimer = window.setTimeout(hideHomeNotificationToast, HOME_NOTIFICATION_TOAST_MS);
}

function bindHomeNotificationToast() {
  if (!homeNotificationToast || homeNotificationToast.dataset.bound === "true") {
    return;
  }
  homeNotificationToast.dataset.bound = "true";
  homeNotificationAction.addEventListener("click", () => {
    hideHomeNotificationToast();
    openTarget("inbox");
  });
  homeNotificationDismiss.addEventListener("click", hideHomeNotificationToast);
}

function hideHomeNotificationToast() {
  if (!homeNotificationToast) {
    return;
  }
  window.clearTimeout(homeNotificationToastTimer);
  homeNotificationToast.setAttribute("aria-hidden", "true");
  homeNotificationToast.hidden = true;
}

/* ---- Dock behavior: magnification, tooltips, launch bounce ----
   Magnification is paint-only: layout slots stay fixed at 40px so hit targets
   never move; only the inner .taskbar-icon scales (origin bottom-center) with
   a cosine falloff around the pointer. Icon centers are cached on hover entry;
   pointermove only reads clientX and writes transforms inside one rAF per
   frame. Disabled while dragging and under prefers-reduced-motion. */
const DOCK_MAG_MAX_SCALE = 1.55;
const DOCK_MAG_RANGE_PX = 88;
const DOCK_MAG_LIFT_RATIO = 0.35;
/* Neighbors slide away from the cursor by a fraction of the magnified peers'
   width growth (macOS grows the whole dock; we spread within the pill). */
const DOCK_MAG_SPREAD = 0.3;
const DOCK_ICON_BASE_PX = 40;
const DOCK_TOOLTIP_SHOW_MS = 320;
const DOCK_TOOLTIP_HIDE_MS = 100;

// Queried lazily: matchMedia is unavailable in the host's DOM-stubbed smoke
// harnesses, and the GUI module graph must stay import-safe there.
function dockReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}
const dockState = {
  taskbar: null,
  icons: [],
  raf: 0,
  pointerX: null,
  tooltipNode: null,
  tooltipShowTimer: 0,
  tooltipHideTimer: 0,
  tooltipAnchor: null,
};

function dockMagnifyEnabled() {
  return (
    !dockReducedMotion() &&
    !document.body.classList.contains("dragging-target") &&
    window.matchMedia?.("(hover: hover) and (min-width: 641px)").matches === true
  );
}

function rebuildDockIconCache() {
  dockState.icons = Array.from(
    dockState.taskbar.querySelectorAll(".taskbar-item"),
  ).map((item) => {
    const rect = item.getBoundingClientRect();
    return {
      node: item.querySelector(".taskbar-icon"),
      item,
      center: rect.left + rect.width / 2,
    };
  });
  /* The wave should span ~2 icons each side of the cursor, whatever the
     current pitch (icon + gap) is — a fixed px range covers barely one. */
  let pitch = 0;
  for (let i = 1; i < dockState.icons.length; i += 1) {
    const gap = dockState.icons[i].center - dockState.icons[i - 1].center;
    if (gap > 0 && (pitch === 0 || gap < pitch)) {
      pitch = gap;
    }
  }
  dockState.range = Math.max(DOCK_MAG_RANGE_PX, pitch * 2.4);
}

function resetDockMagnification() {
  dockState.pointerX = null;
  for (const entry of dockState.icons) {
    if (entry.node) {
      entry.node.style.transform = "";
    }
    entry.item?.style.removeProperty("--dock-shift");
  }
}

function applyDockMagnification() {
  dockState.raf = 0;
  if (dockState.pointerX === null) {
    return;
  }
  const range = dockState.range || DOCK_MAG_RANGE_PX;
  const scales = dockState.icons.map((entry) => {
    const distance = Math.abs(dockState.pointerX - entry.center);
    if (distance >= range) {
      return 1;
    }
    const falloff = 0.5 * (1 + Math.cos((Math.PI * distance) / range));
    return 1 + (DOCK_MAG_MAX_SCALE - 1) * falloff;
  });
  for (let i = 0; i < dockState.icons.length; i += 1) {
    const entry = dockState.icons[i];
    if (!entry.node) {
      continue;
    }
    const scale = scales[i];
    /* Every magnified peer pushes this icon away from itself, so the row
       spreads around the cursor like the macOS dock wave. */
    let shift = 0;
    for (let j = 0; j < dockState.icons.length; j += 1) {
      if (j === i || scales[j] <= 1) {
        continue;
      }
      shift +=
        Math.sign(entry.center - dockState.icons[j].center) *
        (scales[j] - 1) *
        DOCK_ICON_BASE_PX *
        DOCK_MAG_SPREAD;
    }
    if (scale <= 1.004 && Math.abs(shift) < 0.5) {
      entry.node.style.transform = "";
      entry.item?.style.removeProperty("--dock-shift");
      continue;
    }
    const lift = -(scale - 1) * DOCK_ICON_BASE_PX * DOCK_MAG_LIFT_RATIO;
    entry.node.style.transform = `translate(${shift.toFixed(2)}px, ${lift.toFixed(2)}px) scale(${scale.toFixed(3)})`;
    entry.item?.style.setProperty("--dock-shift", `${shift.toFixed(2)}px`);
  }
  repositionDockTooltip();
}

/* Keep the visible label riding the magnified icon (macOS labels track). */
function repositionDockTooltip() {
  const tooltip = dockState.tooltipNode;
  const anchor = dockState.tooltipAnchor;
  if (!tooltip || tooltip.hidden || !anchor?.isConnected) {
    return;
  }
  const icon = anchor.querySelector(".taskbar-icon");
  const rect = (icon || anchor).getBoundingClientRect();
  tooltip.style.left = `${rect.left + rect.width / 2}px`;
  tooltip.style.top = `${rect.top - 11}px`;
}

function dockTooltipNode() {
  if (!dockState.tooltipNode) {
    const node = document.createElement("div");
    node.id = "dock-tooltip";
    node.className = "dock-tooltip";
    node.setAttribute("role", "tooltip");
    node.hidden = true;
    document.body.appendChild(node);
    dockState.tooltipNode = node;
  }
  return dockState.tooltipNode;
}

function scheduleDockTooltip(item, delay) {
  const label = item.dataset.label || "";
  if (!label) {
    return;
  }
  window.clearTimeout(dockState.tooltipHideTimer);
  window.clearTimeout(dockState.tooltipShowTimer);
  /* macOS timing: the first label waits, but while one is already up,
     sweeping across icons retargets it instantly — no stale label. */
  const tooltipUp = dockState.tooltipNode && !dockState.tooltipNode.hidden;
  if (tooltipUp || delay === 0) {
    showDockTooltip(item, label);
    return;
  }
  dockState.tooltipShowTimer = window.setTimeout(() => {
    showDockTooltip(item, label);
  }, delay);
}

function showDockTooltip(item, label) {
  if (!item.isConnected) {
    return;
  }
  const tooltip = dockTooltipNode();
  tooltip.textContent = label;
  tooltip.hidden = false;
  /* Anchor to the icon's live (possibly magnified) rect so the label floats
     clear above the grown icon, tail pointing at it — not glued to the bar. */
  const icon = item.querySelector(".taskbar-icon");
  const rect = (icon || item).getBoundingClientRect();
  tooltip.style.left = `${rect.left + rect.width / 2}px`;
  tooltip.style.top = `${rect.top - 11}px`;
  tooltip.dataset.visible = "true";
  if (dockState.tooltipAnchor && dockState.tooltipAnchor !== item) {
    dockState.tooltipAnchor.removeAttribute("aria-describedby");
  }
  dockState.tooltipAnchor = item;
  item.setAttribute("aria-describedby", "dock-tooltip");
}

function scheduleDockTooltipHide() {
  window.clearTimeout(dockState.tooltipShowTimer);
  window.clearTimeout(dockState.tooltipHideTimer);
  dockState.tooltipHideTimer = window.setTimeout(hideDockTooltip, DOCK_TOOLTIP_HIDE_MS);
}

function hideDockTooltip() {
  window.clearTimeout(dockState.tooltipShowTimer);
  if (dockState.tooltipNode) {
    dockState.tooltipNode.hidden = true;
    delete dockState.tooltipNode.dataset.visible;
  }
  if (dockState.tooltipAnchor) {
    dockState.tooltipAnchor.removeAttribute("aria-describedby");
    dockState.tooltipAnchor = null;
  }
}

function startDockLaunchBounce(item) {
  if (dockReducedMotion()) {
    return;
  }
  const icon = item.querySelector(".taskbar-icon");
  if (!icon) {
    return;
  }
  item.classList.remove("launching");
  // Restart the keyframe animation if a bounce is already mid-flight.
  void item.offsetWidth;
  item.classList.add("launching");
  icon.addEventListener(
    "animationend",
    () => {
      item.classList.remove("launching");
    },
    { once: true },
  );
}

function setupDock() {
  dockState.taskbar = document.querySelector(".taskbar");
  const taskbar = dockState.taskbar;
  if (!taskbar) {
    return;
  }
  taskbar.addEventListener("pointerenter", () => {
    if (dockMagnifyEnabled()) {
      rebuildDockIconCache();
    }
  });
  taskbar.addEventListener("pointermove", (event) => {
    if (!dockMagnifyEnabled()) {
      resetDockMagnification();
      return;
    }
    if (dockState.icons.length === 0) {
      rebuildDockIconCache();
    }
    dockState.pointerX = event.clientX;
    if (!dockState.raf) {
      dockState.raf = window.requestAnimationFrame(applyDockMagnification);
    }
  });
  taskbar.addEventListener("pointerleave", resetDockMagnification);
  window.addEventListener("resize", () => {
    dockState.icons = [];
  });

  // Tooltips by delegation so they survive taskbar re-renders. Shown on hover
  // (after a delay) and on keyboard focus (immediately); Escape dismisses.
  taskbar.addEventListener("pointerover", (event) => {
    const item = event.target.closest(".taskbar-item");
    if (!item || (event.relatedTarget && item.contains(event.relatedTarget))) {
      return;
    }
    scheduleDockTooltip(item, DOCK_TOOLTIP_SHOW_MS);
  });
  taskbar.addEventListener("pointerout", (event) => {
    const item = event.target.closest(".taskbar-item");
    if (!item || (event.relatedTarget && item.contains(event.relatedTarget))) {
      return;
    }
    scheduleDockTooltipHide();
  });
  taskbar.addEventListener("focusin", (event) => {
    const item = event.target.closest(".taskbar-item");
    if (item) {
      scheduleDockTooltip(item, 0);
    }
  });
  taskbar.addEventListener("focusout", scheduleDockTooltipHide);
  taskbar.addEventListener("click", hideDockTooltip);
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      hideDockTooltip();
    }
  });
}

/* Called by the home-gui facade once ensureHomeGuiDom() has instantiated the
   lazy GUI template — these nodes do not exist at module-evaluation time. */
let shellSurfaceDomBound = false;

export function bindShellSurfaceDom() {
  if (shellSurfaceDomBound) {
    return;
  }
  shellSurfaceDomBound = true;
  // The launcher is a modal dialog: Tab cycles inside the popover until it is
  // dismissed.
  launcher?.addEventListener("keydown", (event) => {
    if (!launcher.hidden) {
      trapTabWithin(launcher.querySelector(".launcher-popover"), event);
    }
  });
  desktopContextMenu?.addEventListener("keydown", handleContextMenuKeydown);
  setupDock();
  syncDockAutoHide();
  bindDockAutoHideReveal();
}

/* ---- Dock auto-hide (local preference, same store as theme/sounds) ---- */
const DOCK_AUTOHIDE_KEY = "elastos.ui.dockAutoHide";

export function dockAutoHideEnabled() {
  try {
    return localStorage.getItem(DOCK_AUTOHIDE_KEY) === "on";
  } catch (_error) {
    return false;
  }
}

export function setDockAutoHide(on) {
  try {
    localStorage.setItem(DOCK_AUTOHIDE_KEY, on ? "on" : "off");
  } catch (_error) {}
  syncDockAutoHide();
}

export function syncDockAutoHide() {
  const on = dockAutoHideEnabled();
  document.body.classList.toggle("dock-autohide", on);
  if (!on) {
    document.body.classList.remove("dock-revealed");
  }
}

function bindDockAutoHideReveal() {
  const revealBand = 28;
  window.addEventListener("pointermove", (event) => {
    if (!document.body.classList.contains("dock-autohide")) {
      return;
    }
    const nearBottom = event.clientY >= window.innerHeight - revealBand;
    const overDock = Boolean(event.target.closest?.(".taskbar"));
    document.body.classList.toggle("dock-revealed", nearBottom || overDock);
  });
  window.addEventListener("storage", (event) => {
    if (event.key === DOCK_AUTOHIDE_KEY || event.key === null) {
      syncDockAutoHide();
    }
  });
}
