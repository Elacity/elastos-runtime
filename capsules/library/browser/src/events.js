import {
  inTrash,
  isDirectory,
} from "./model.js";

export function bindLibraryEvents({
  bindDialogEvents,
  clearSelection,
  copySelectedObjectsTo,
  createFolder,
  elements,
  handleBrowserPopState,
  hideDialog,
  hideMenu,
  loadCurrentFolder,
  moveSelectedObjectsTo,
  navigate,
  navigateBack,
  navigateForward,
  navigateUp,
  objectByUri,
  openObject,
  prepareDragSelection,
  reorderPlace,
  scheduleContentRender,
  selectAllVisible,
  selectRangeTo,
  selectedObjects,
  selectOnly,
  setSort,
  setView,
  deleteSelectedObjects,
  showBackgroundMenu,
  showError,
  showMenuForObject,
  showPlaceMenu,
  startRename,
  state,
  stopLibraryEventStream,
  toggleSelected,
  trashSelectedObjects,
  uploadFiles,
}) {
  let draggingPlaceId = "";

  elements.sidebar?.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    event.stopPropagation();
    hideMenu();
  });
  elements.places.addEventListener("click", (event) => {
    const button = event.target.closest(".place");
    if (button?.dataset.uri) {
      navigate(button.dataset.uri).catch(showError);
    }
  });
  elements.places.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    event.stopPropagation();
    const button = event.target.closest(".place");
    if (button?.dataset.uri) {
      showPlaceMenu(button.dataset.uri, event.clientX, event.clientY);
      return;
    }
    hideMenu();
  });
  elements.places.addEventListener("dragstart", (event) => {
    const button = event.target.closest(".place");
    if (!button?.dataset.rootId) return;
    draggingPlaceId = button.dataset.rootId;
    button.classList.add("window-sidebar-item-dragging");
    elements.places.dataset.reordering = "true";
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("application/x-elastos-library-root-id", draggingPlaceId);
    event.dataTransfer.setData("text/plain", button.dataset.uri || "");
  });
  elements.places.addEventListener("dragover", (event) => {
    const button = event.target.closest(".place");
    if (button?.dataset.rootId && dataTransferHasType(event.dataTransfer, "application/x-elastos-library-root-id")) {
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      markPlaceDropTarget(elements, button, event);
      return;
    }
    if (button?.dataset.rootId === "trash" && selectedObjects().some((object) => !inTrash(object))) {
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      return;
    }
    if (button?.dataset.uri && selectedObjects().length) {
      event.preventDefault();
      event.dataTransfer.dropEffect = event.altKey ? "copy" : "move";
    }
  });
  elements.places.addEventListener("dragleave", (event) => {
    const button = event.target.closest(".place");
    if (button && !button.contains(event.relatedTarget)) {
      delete button.dataset.dropPosition;
    }
  });
  elements.places.addEventListener("drop", (event) => {
    const button = event.target.closest(".place");
    const sourceRootId = event.dataTransfer?.getData("application/x-elastos-library-root-id") || draggingPlaceId;
    if (button?.dataset.rootId && sourceRootId) {
      event.preventDefault();
      reorderPlace(sourceRootId, button.dataset.rootId, placeDropPosition(button, event));
      clearPlaceDropTargets(elements);
      draggingPlaceId = "";
      return;
    }
    if (button?.dataset.rootId === "trash" && selectedObjects().some((object) => !inTrash(object))) {
      event.preventDefault();
      trashSelectedObjects().catch(showError);
      return;
    }
    if (!button?.dataset.uri || !selectedObjects().length) return;
    event.preventDefault();
    const action = event.altKey ? copySelectedObjectsTo : moveSelectedObjectsTo;
    action(button.dataset.uri).catch(showError);
  });
  elements.places.addEventListener("dragend", () => {
    draggingPlaceId = "";
    clearPlaceDropTargets(elements);
  });
  elements.breadcrumbs.addEventListener("click", (event) => {
    const button = event.target.closest(".crumb");
    if (button?.dataset.uri) {
      navigate(button.dataset.uri).catch(showError);
    }
  });
  elements.search.addEventListener("input", () => {
    state.query = elements.search.value || "";
    scheduleContentRender();
  });
  elements.sortSelect.addEventListener("change", () => {
    setSort(elements.sortSelect.value || "name");
  });
  elements.backButton.addEventListener("click", () => navigateBack().catch(showError));
  elements.forwardButton.addEventListener("click", () => navigateForward().catch(showError));
  elements.upButton.addEventListener("click", () => navigateUp().catch(showError));
  elements.refreshButton.addEventListener("click", () => loadCurrentFolder().catch(showError));
  elements.gridButton.addEventListener("click", () => setView("grid"));
  elements.listButton.addEventListener("click", () => setView("list"));
  elements.uploadButton.addEventListener("click", () => elements.fileInput.click());
  elements.newFolderButton.addEventListener("click", () => createFolder().catch(showError));
  elements.fileInput.addEventListener("change", () => {
    uploadFiles(elements.fileInput.files).catch(showError);
    elements.fileInput.value = "";
  });
  elements.content.addEventListener("click", (event) => {
    if (isNameEditorTarget(event.target)) return;
    const item = event.target.closest(".item");
    if (!item?.dataset.uri) {
      clearSelection();
      return;
    }
    item.focus({ preventScroll: true });
    if (event.shiftKey) {
      selectRangeTo(item.dataset.uri, event.metaKey || event.ctrlKey);
    } else if (event.metaKey || event.ctrlKey) {
      toggleSelected(item.dataset.uri);
    } else {
      selectOnly(item.dataset.uri);
    }
  });
  elements.content.addEventListener("dblclick", (event) => {
    if (isNameEditorTarget(event.target)) return;
    const item = event.target.closest(".item");
    const object = item ? objectByUri(item.dataset.uri) : null;
    if (object) openObject(object).catch(showError);
  });
  elements.content.addEventListener("contextmenu", (event) => {
    if (isNameEditorTarget(event.target)) return;
    event.preventDefault();
    const item = event.target.closest(".item");
    const object = item ? objectByUri(item.dataset.uri) : null;
    if (object) {
      showMenuForObject(object, event.clientX, event.clientY);
    } else {
      clearSelection();
      showBackgroundMenu(event.clientX, event.clientY);
    }
  });
  elements.content.addEventListener("dragstart", (event) => {
    const item = event.target.closest(".item");
    if (!item?.dataset.uri) return;
    prepareDragSelection(item.dataset.uri, item);
    event.dataTransfer.effectAllowed = "copyMove";
    event.dataTransfer.setData("application/x-elastos-library-uris", JSON.stringify(Array.from(state.selectedUris)));
    event.dataTransfer.setData("text/plain", item.dataset.uri);
  });
  elements.content.addEventListener("dragover", (event) => {
    const item = event.target.closest(".item");
    const object = item ? objectByUri(item.dataset.uri) : null;
    const hasFiles = dataTransferHasType(event.dataTransfer, "Files");
    if (hasFiles || (object && isDirectory(object) && selectedObjects().length)) {
      event.preventDefault();
      event.dataTransfer.dropEffect = hasFiles || event.altKey ? "copy" : "move";
    }
  });
  elements.content.addEventListener("drop", (event) => {
    event.preventDefault();
    const files = event.dataTransfer?.files;
    if (files?.length) {
      uploadFiles(files).catch(showError);
      return;
    }
    const item = event.target.closest(".item");
    const object = item ? objectByUri(item.dataset.uri) : null;
    if (object && isDirectory(object) && selectedObjects().length) {
      const action = event.altKey ? copySelectedObjectsTo : moveSelectedObjectsTo;
      action(object.uri).catch(showError);
    }
  });
  document.addEventListener("click", (event) => {
    if (!event.target.closest(".context-menu")) {
      hideMenu();
    }
  });
  bindDialogEvents();
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      hideMenu();
      hideDialog();
      clearSelection();
      return;
    }
    const editable = event.target.closest?.("input, textarea, select, [contenteditable='true']");
    const itemShortcutTarget = isItemShortcutTarget(event.target, elements);
    const shortcutSurfaceReady = itemShortcutTarget && !editable && !isDialogOpen(elements) && !isMenuOpen(elements);
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a" && shortcutSurfaceReady) {
      event.preventDefault();
      selectAllVisible();
    }
    if (event.key === "F2" && !editable) {
      if (!itemShortcutTarget || isDialogOpen(elements) || isMenuOpen(elements)) return;
      const objects = selectedObjects();
      if (objects.length === 1) {
        event.preventDefault();
        startRename(objects[0]);
      }
    }
    if (event.key === "Enter" && shortcutSurfaceReady) {
      const objects = selectedObjects();
      if (objects.length) {
        event.preventDefault();
        openSelectedObjects(objects, openObject, showError);
      }
    }
    if (event.key === "Delete" && shortcutSurfaceReady) {
      const objects = selectedObjects();
      if (objects.length) {
        event.preventDefault();
        const hasTrash = objects.some(inTrash);
        const action = event.shiftKey || hasTrash ? deleteSelectedObjects : trashSelectedObjects;
        action().catch(showError);
      }
    }
    if ((event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) && shortcutSurfaceReady) {
      event.preventDefault();
      const object = selectedObjects()[0];
      if (object) {
        showKeyboardObjectMenu(elements, object, showMenuForObject);
      } else {
        showKeyboardBackgroundMenu(elements, showBackgroundMenu);
      }
    }
  });
  window.addEventListener("popstate", (event) => {
    handleBrowserPopState(event).catch(showError);
  });
  window.addEventListener("beforeunload", () => {
    stopLibraryEventStream();
  });
}

function dataTransferHasType(dataTransfer, type) {
  return Array.from(dataTransfer?.types || []).includes(type);
}

function markPlaceDropTarget(elements, button, event) {
  const position = placeDropPosition(button, event);
  if (button.dataset.dropPosition === position) return;
  for (const place of elements.places.querySelectorAll(".place[data-drop-position]")) {
    if (place !== button) delete place.dataset.dropPosition;
  }
  button.dataset.dropPosition = position;
}

function placeDropPosition(button, event) {
  const rect = button.getBoundingClientRect();
  return event.clientY > rect.top + rect.height / 2 ? "after" : "before";
}

function clearPlaceDropTargets(elements) {
  delete elements.places.dataset.reordering;
  for (const place of elements.places.querySelectorAll(".place")) {
    place.classList.remove("window-sidebar-item-dragging");
    delete place.dataset.dropPosition;
  }
}

function isNameEditorTarget(target) {
  return !!target?.closest?.(".rename-input");
}

function isItemShortcutTarget(target, elements) {
  return !!(target && elements.content && (target === elements.content || elements.content.contains?.(target)));
}

function isDialogOpen(elements) {
  return elements.dialog && !elements.dialog.classList.contains("hidden");
}

function isMenuOpen(elements) {
  return elements.contextMenu && !elements.contextMenu.classList.contains("hidden");
}

function showKeyboardObjectMenu(elements, object, showMenuForObject) {
  const item = elements.content.querySelector(`[data-uri="${CSS.escape(object.uri)}"]`);
  const rect = item?.getBoundingClientRect();
  showMenuForObject(object, rect ? rect.left + 16 : window.innerWidth / 2, rect ? rect.top + 16 : 120);
}

function showKeyboardBackgroundMenu(elements, showBackgroundMenu) {
  const rect = elements.content.getBoundingClientRect();
  showBackgroundMenu(rect.left + Math.min(48, rect.width / 2), rect.top + Math.min(48, rect.height / 2));
}

async function openSelectedObjects(objects, openObject, showError) {
  try {
    for (const object of objects) {
      await openObject(object);
    }
  } catch (error) {
    showError(error);
  }
}
