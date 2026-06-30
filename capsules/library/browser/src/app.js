import {
  archiveLibraryObjectPayload,
  baseName,
  contentCid,
  escapeHtml,
  hasCapability,
  inTrash,
  isBlockedObject,
  isDirectory,
  isTrashRootUri,
  isTrashUri,
  isWebSpaceUri,
  parentUri,
  publishedCid,
  viewerOptions,
} from "./model.js";
import { createLibraryRuntime } from "./api.js";
import { createLibraryActions } from "./actions.js";
import { createLibraryDialog } from "./dialog.js";
import { createLibraryEditor } from "./editor.js";
import { bindLibraryEvents } from "./events.js";
import { createLibraryMenu } from "./menu.js";
import { createLibraryNavigation } from "./navigation.js";
import { createLibraryPreview } from "./preview.js";
import { createLibraryRealtime } from "./realtime.js";
import { createLibraryRenderer, iconPlaceholder } from "./render.js";
import { createLibrarySelection } from "./selection.js";
import {
  MUTATING_PROVIDER_OPS,
  cacheFolderListing,
  createLibraryState,
  setLibraryObjects,
  visibleObjectsForState,
} from "./state.js";
import { createLibraryUploads } from "./uploads.js";

    const queryParams = new URLSearchParams(window.location.search);
    const { state, perf } = createLibraryState({
      queryParams,
      storage: localStorage,
      perfTarget: (window.__libraryPerf = window.__libraryPerf || {}),
    });
    const {
      providerApi: runtimeProviderApi,
      uploadObject,
      downloadObjectRaw,
      openTarget,
      openPublishedUri,
      deliverToTarget,
      closeSelf,
    } = createLibraryRuntime({ getHomeToken: () => state.homeToken });

    const elements = {
      lockedShell: document.getElementById("locked-shell"),
      libraryShell: document.getElementById("library-shell"),
      places: document.getElementById("places"),
      backButton: document.getElementById("back-button"),
      forwardButton: document.getElementById("forward-button"),
      upButton: document.getElementById("up-button"),
      uploadButton: document.getElementById("upload-button"),
      newFolderButton: document.getElementById("new-folder-button"),
      pickerActionButton: document.getElementById("picker-action-button"),
      search: document.getElementById("search"),
      currentTitle: document.getElementById("current-title"),
      statusText: document.getElementById("status-text"),
      refreshButton: document.getElementById("refresh-button"),
      gridButton: document.getElementById("grid-button"),
      listButton: document.getElementById("list-button"),
      sortSelect: document.getElementById("sort-select"),
      breadcrumbs: document.getElementById("breadcrumbs"),
      content: document.getElementById("content"),
      footerLeft: document.getElementById("footer-left"),
      footerRight: document.getElementById("footer-right"),
      fileInput: document.getElementById("file-input"),
      uploadProgress: document.getElementById("upload-progress"),
      contextMenu: document.getElementById("context-menu"),
      dialog: document.getElementById("dialog"),
      sidebar: document.querySelector(".sidebar"),
    };
    let renderContent = () => {};
    let renderFooter = () => {};
    let scheduleContentRender = () => {};
    let syncContentViewMode = () => {};
    let syncViewButtons = () => {};
    let previewObject = async () => {};
    let revokePreviewUrl = () => {};
    let startCreateObject = () => {};
    let startRename = () => {};
    let startLibraryEventStream = () => {};
    let stopLibraryEventStream = () => {};
    let canPasteInto = () => false;
    let checkMyAccess = async () => {};
    let compressObjectToZip = async () => {};
    let compressSelectedObjectsToZip = async () => {};
    let copySelectedObjectsTo = async () => {};
    let copyText = async () => {};
    let createFolder = async () => {};
    let createTextDocument = async () => {};
    let deleteObject = async () => {};
    let deleteSelectedObjects = async () => {};
    let downloadObject = async () => {};
    let downloadObjectAsZip = async () => {};
    let downloadSelectedObjects = async () => {};
    let downloadSelectedObjectsAsZip = async () => {};
    let emptyTrash = async () => {};
    let extractArchiveObject = async () => {};
    let moveSelectedObjectsTo = async () => {};
    let openObject = async () => {};
    let openWithViewer = () => false;
    let pasteClipboardTo = async () => {};
    let publishObject = async () => {};
    let publishSelectedObjects = async () => {};
    let repairObject = async () => {};
    let restoreObject = async () => {};
    let restoreSelectedObjects = async () => {};
    let setClipboard = () => {};
    let shareObject = async () => {};
    let showStatusObject = async () => {};
    let trashObject = async () => {};
    let trashSelectedObjects = async () => {};
    let unpublishObject = async () => {};
    let unpublishSelectedObjects = async () => {};
    let uploadFiles = async () => {};
    const {
      clearSelection,
      isSelected,
      objectByUri,
      prepareDragSelection,
      selectAllVisible,
      selectedObjects,
      selectOnly,
      selectRangeTo,
      syncSelectionDom,
      toggleSelected,
    } = createLibrarySelection({
      content: elements.content,
      renderFooter: () => renderFooter(),
      state,
      visibleObjects,
    });
    const {
      handleBrowserPopState,
      installBrowserHistory,
      navigate,
      navigateBack,
      navigateForward,
      navigateUp,
      syncNavigationButtons,
    } = createLibraryNavigation({
      backButton: elements.backButton,
      forwardButton: elements.forwardButton,
      loadCurrentFolder,
      parentUri,
      rootForUri,
      searchInput: elements.search,
      setStatus,
      state,
      syncRouteChrome,
      upButton: elements.upButton,
    });
    const {
      renderUploads,
      setUploadProgress,
    } = createLibraryUploads({
      container: elements.uploadProgress,
      perf,
      state,
    });
    const {
      hideMenu,
      menuAction,
      renderMenu,
    } = createLibraryMenu({
      contextMenu: elements.contextMenu,
      perf,
      showError,
    });
    const {
      bindDialogEvents,
      confirmDestructive,
      hideDialog,
      showObjectStatus,
      showProperties,
      showShareDialog,
      showShareReceipt,
      showSharedAccessReceipt,
    } = createLibraryDialog({
      copyText: (...args) => copyText(...args),
      dialog: elements.dialog,
      hideMenu,
      objectByUri,
      onBeforeClose: () => revokePreviewUrl(),
    });
    ({
      previewObject,
      revokePreviewUrl,
    } = createLibraryPreview({
      dialog: elements.dialog,
      providerApi,
      setStatus,
      showProperties,
      state,
    }));
    ({
      renderContent,
      renderFooter,
      scheduleContentRender,
      syncContentViewMode,
      syncViewButtons,
    } = createLibraryRenderer({
      elements,
      isSelected,
      perf,
      selectedObjects,
      state,
      visibleObjects,
    }));
    ({
      startCreateObject,
      startRename,
    } = createLibraryEditor({
      content: elements.content,
      loadCurrentFolder,
      providerApi,
      renderContent,
      setObjects,
      setStatus,
      showError,
      state,
    }));
    ({
      startLibraryEventStream,
      stopLibraryEventStream,
    } = createLibraryRealtime({
      loadCurrentFolder,
      parentUri,
      showError,
      state,
    }));
    ({
      canPasteInto,
      checkMyAccess,
      compressObjectToZip,
      compressSelectedObjectsToZip,
      copySelectedObjectsTo,
      copyText,
      createFolder,
      createTextDocument,
      deleteObject,
      deleteSelectedObjects,
      downloadObject,
      downloadObjectAsZip,
      downloadSelectedObjects,
      downloadSelectedObjectsAsZip,
      emptyTrash,
      extractArchiveObject,
      moveSelectedObjectsTo,
      openObject,
      openWithViewer,
      pasteClipboardTo,
      publishObject,
      publishSelectedObjects,
      repairObject,
      restoreObject,
      restoreSelectedObjects,
      setClipboard,
      shareObject,
      showStatusObject,
      trashObject,
      trashSelectedObjects,
      unpublishObject,
      unpublishSelectedObjects,
      uploadFiles,
    } = createLibraryActions({
      clearSelection,
      closeSelf,
      confirmDestructive,
      currentFolderReadOnly,
      deliverToTarget,
      downloadObjectRaw,
      loadCurrentFolder,
      loadRoots,
      navigate,
      openPublishedUri,
      openTarget,
      previewObject,
      providerApi,
      renderUploads,
      selectedObjects,
      setStatus,
      setUploadProgress,
      showMenuForObject,
      showObjectStatus,
      showProperties,
      showShareDialog,
      showShareReceipt,
      showSharedAccessReceipt,
      startCreateObject,
      state,
      uploadObject,
    }));

    function isAttachMode() {
      return (
        state.mode === "attach" &&
        (state.returnTarget === "chat-room" || state.returnTarget === "browser")
      );
    }

    function isArchiveOpenMode() {
      return state.mode === "archive-open" && state.returnTarget === "archive-manager";
    }

    function isArchiveCreateMode() {
      return state.mode === "archive-create" && state.returnTarget === "archive-manager";
    }

    function isArchivePickerMode() {
      return isArchiveOpenMode() || isArchiveCreateMode();
    }

    function isPickerActionMode() {
      return isAttachMode() || isArchivePickerMode();
    }

    function setStatus(text) {
      elements.statusText.textContent = text;
      elements.statusText.classList.toggle("hidden", !text);
    }

    function attachStatusText() {
      return state.returnTarget === "browser"
        ? "Choose an object for Browser."
        : "Choose an object for Chat Room.";
    }

    async function providerApi(op, payload) {
      const result = await runtimeProviderApi(op, payload);
      if (MUTATING_PROVIDER_OPS.has(op)) {
        state.folderCache.clear();
        state.folderPrefetches.clear();
        perf.folderCacheSize = 0;
      }
      return result;
    }

    function setObjects(objects) {
      setLibraryObjects(state, objects);
    }

    function rootForUri(uri) {
      return activeRootForUri(uri) || state.roots[0] || null;
    }

    function activeRootForUri(uri) {
      const value = String(uri || "");
      return state.roots
        .filter((root) => value === root.uri || value.startsWith(root.uri + "/"))
        .sort((left, right) => right.uri.length - left.uri.length)[0] || null;
    }

    function currentFolderReadOnly() {
      if (isTrashRootUri(state.currentUri) || isTrashUri(state.currentUri)) return true;
      if (!isWebSpaceUri(state.currentUri)) return false;
      const folderObject = state.currentObject || objectByUri(state.currentUri);
      return folderObject?.metadata?.readonly !== false;
    }

    function setFolderStatus(text) {
      if (isArchivePickerMode()) {
        setStatus("");
        return;
      }
      if (isAttachMode()) {
        setStatus(attachStatusText());
        return;
      }
      setStatus(text);
    }

    function syncModeChrome() {
      elements.pickerActionButton.classList.toggle("hidden", !isPickerActionMode());
      if (isAttachMode()) {
        elements.pickerActionButton.textContent =
          state.returnTarget === "browser" ? "Select for Browser" : "Attach to Chat";
        setStatus(attachStatusText());
        elements.uploadButton.textContent = "Upload";
        return;
      }
      if (isArchiveOpenMode()) {
        elements.pickerActionButton.textContent = "Open in Archive";
        setStatus("");
        return;
      }
      if (isArchiveCreateMode()) {
        elements.pickerActionButton.textContent = "Create ZIP";
        setStatus("");
        return;
      }
      setStatus("Ready.");
    }

    async function completeAttachPicker() {
      if (!isAttachMode()) return;
      const selection = selectedObjects();
      if (selection.length !== 1) {
        setStatus(
          state.returnTarget === "browser"
            ? "Select one Library item for Browser."
            : "Select one Library item for Chat Room.",
        );
        return;
      }
      await openObject(selection[0]);
    }

    async function completeArchivePicker() {
      if (isArchiveOpenMode()) {
        const selection = selectedObjects();
        if (selection.length !== 1) {
          setStatus("Select one archive to open.");
          return;
        }
        await openObject(selection[0]);
        return;
      }
      if (!isArchiveCreateMode()) return;
      const objects = archiveCreateSelection();
      if (!objects.length) {
        setStatus("Select one compressible item, or several same-folder items.");
        return;
      }
      setStatus(objects.length === 1 ? `Creating ${objects[0].name}.zip...` : `Creating ZIP from ${objects.length} items...`);
      const response = objects.length === 1
        ? await providerApi("compress_archive", { uri: objects[0].uri, if_revision: objects[0].revision })
        : await providerApi("compress_archive", { uris: objects.map((object) => object.uri) });
      const archiveObject = response?.object;
      await loadCurrentFolder();
      if (archiveObject && deliverArchiveToArchive(archiveObject)) {
        setStatus(`Created ${archiveObject.name || "archive"} and opened it in Archive.`);
        return;
      }
      setStatus("ZIP created. Select it and press Open in Archive.");
    }

    function deliverArchiveToArchive(object) {
      const payload = {
        type: "archive:open-library-object",
        object: archiveLibraryObjectPayload(object),
      };
      if (deliverToTarget("archive-manager", payload) || openWithViewer(object, "archive-manager")) {
        window.setTimeout(closeSelf, 80);
        return true;
      }
      return false;
    }

    function archiveCreateSelection() {
      const objects = selectedObjects();
      const compressible = objects.filter((object) => (
        object &&
        !isBlockedObject(object) &&
        !inTrash(object) &&
        !isWebSpaceUri(object.uri) &&
        hasCapability(object, "compress_archive")
      ));
      if (compressible.length !== objects.length || !compressible.length) return [];
      if (compressible.length === 1) return compressible;
      const parent = parentUri(compressible[0].uri);
      return compressible.every((object) => parentUri(object.uri) === parent) ? compressible : [];
    }

    async function loadRoots() {
      const data = await providerApi("roots");
      state.roots = orderRoots(Array.isArray(data.roots) ? data.roots : []);
      if (!state.currentUri && state.roots.length) {
        const documents = state.roots.find((root) => root.id === "documents");
        const initialRoot = state.initialUri ? rootForUri(state.initialUri) : null;
        state.currentUri = initialRoot ? state.initialUri : (documents || state.roots[0]).uri;
      }
      renderPlaces();
    }

    function orderRoots(roots) {
      const rank = new Map(state.sidebarOrder.map((key, index) => [key, index]));
      return roots
        .map((root, index) => ({ root, index, key: rootKey(root) }))
        .sort((left, right) => {
          const leftRank = rank.has(left.key) ? rank.get(left.key) : Number.MAX_SAFE_INTEGER;
          const rightRank = rank.has(right.key) ? rank.get(right.key) : Number.MAX_SAFE_INTEGER;
          if (leftRank !== rightRank) return leftRank - rightRank;
          return left.index - right.index;
        })
        .map((entry) => entry.root);
    }

    function rootKey(root) {
      return root?.id || root?.uri || "";
    }

    function reorderPlace(sourceRootId, targetRootId, placement = "before") {
      if (!sourceRootId || !targetRootId || sourceRootId === targetRootId) return;
      const sourceIndex = state.roots.findIndex((root) => rootKey(root) === sourceRootId);
      const targetIndex = state.roots.findIndex((root) => rootKey(root) === targetRootId);
      if (sourceIndex < 0 || targetIndex < 0) return;
      const previousTops = capturePlaceTops();
      const [source] = state.roots.splice(sourceIndex, 1);
      const adjustedTargetIndex = state.roots.findIndex((root) => rootKey(root) === targetRootId);
      const insertIndex = placement === "after" ? adjustedTargetIndex + 1 : adjustedTargetIndex;
      state.roots.splice(insertIndex, 0, source);
      state.sidebarOrder = state.roots.map(rootKey).filter(Boolean);
      localStorage.setItem("library.sidebarOrder", JSON.stringify(state.sidebarOrder));
      renderPlaces({ animateFrom: previousTops });
      syncPlacesActive();
      setStatus("Sidebar order saved.");
    }

    async function loadCurrentFolder(options = {}) {
      if (!state.currentUri) return;
      const loadSeq = ++state.loadSeq;
      const uri = state.currentUri;
      const cached = options.useCache ? state.folderCache.get(uri) : null;
      let renderedCached = false;
      let renderAfterFetch = true;
      if (cached) {
        perf.folderCacheHits += 1;
        state.loading = false;
        state.currentObject = cached.object || null;
        setObjects(cached.objects);
        state.selectedUris.clear();
        setFolderStatus(`${state.objects.length} object${state.objects.length === 1 ? "" : "s"}.`);
        renderAll();
        await runInitialObjectAction();
        renderedCached = true;
      } else {
        state.loading = true;
        state.currentObject = null;
        setStatus("Loading...");
      }
      try {
        const data = await providerApi("list", { uri });
        if (loadSeq !== state.loadSeq || uri !== state.currentUri) {
          return;
        }
        const objects = Array.isArray(data.objects) ? data.objects : [];
        const currentObject = data.object || null;
        const nextCache = cacheFolderListing(state, perf, uri, objects, currentObject);
        state.currentObject = currentObject;
        if (renderedCached && cached.signature === nextCache.signature) {
          renderAfterFetch = false;
          setFolderStatus(`${state.objects.length} object${state.objects.length === 1 ? "" : "s"}.`);
          return;
        }
        setObjects(objects);
        state.selectedUris.clear();
        setFolderStatus(`${state.objects.length} object${state.objects.length === 1 ? "" : "s"}.`);
      } finally {
        if (loadSeq === state.loadSeq && uri === state.currentUri) {
          state.loading = false;
          if (renderAfterFetch) renderAll();
          if (renderAfterFetch) await runInitialObjectAction();
        }
      }
    }

    async function runInitialObjectAction() {
      if (state.initialActionHandled || !state.initialObjectUri || !state.initialAction) {
        return;
      }
      const object = objectByUri(state.initialObjectUri)
        || (state.currentObject?.uri === state.initialObjectUri ? state.currentObject : null);
      if (!object) {
        return;
      }
      state.initialActionHandled = true;
      selectOnly(object.uri);
      if (state.initialAction === "properties") {
        showProperties(object);
        return;
      }
      if (state.initialAction === "empty-trash" && isTrashRootUri(object.uri)) {
        await emptyTrash();
        return;
      }
      if (state.initialAction === "download" && hasCapability(object, "download")) {
        try {
          await downloadObject(object);
        } catch (error) {
          showError(error);
        }
      }
    }

    function prefetchFolder(uri) {
      if (!uri || state.folderCache.has(uri) || state.folderPrefetches.has(uri)) {
        return Promise.resolve();
      }
      const promise = providerApi("list", { uri })
        .then((data) => cacheFolderListing(state, perf, uri, data.objects, data.object || null))
        .catch(() => null)
        .finally(() => {
          state.folderPrefetches.delete(uri);
        });
      state.folderPrefetches.set(uri, promise);
      return promise;
    }

    function scheduleRootPrefetch() {
      if (state.rootPrefetchStarted || !state.roots.length) return;
      state.rootPrefetchStarted = true;
      window.setTimeout(() => {
        const roots = state.roots
          .filter((root) => root.uri !== state.currentUri)
          .map((root) => prefetchFolder(root.uri));
        Promise.all(roots).catch(() => null);
      }, 50);
    }

    function renderAll() {
      syncPlacesActive();
      renderBreadcrumbs();
      renderContent();
      renderFooter();
      renderUploads();
      syncViewButtons();
      syncNavigationButtons();
    }

    function renderPlaces(options = {}) {
      perf.renderPlacesCount += 1;
      elements.places.innerHTML = "";
      const activeRoot = activeRootForUri(state.currentUri);
      for (const root of state.roots) {
        const active = activeRoot?.uri === root.uri;
        const button = document.createElement("button");
        button.className = active ? "place window-sidebar-item window-sidebar-item-active" : "place window-sidebar-item";
        button.type = "button";
        button.dataset.uri = root.uri;
        button.dataset.rootId = rootKey(root);
        button.dataset.active = active ? "true" : "false";
        button.draggable = true;
        button.title = "Drag to reorder";
        button.innerHTML = `
          ${iconPlaceholder(placeIcon(root), "place-icon window-sidebar-item-icon")}
          <span class="place-label">${escapeHtml(root.label)}</span>
        `;
        elements.places.appendChild(button);
      }
      if (options.animateFrom) animatePlaceReorder(options.animateFrom);
    }

    function capturePlaceTops() {
      return new Map(Array.from(elements.places.querySelectorAll(".place[data-root-id]"))
        .map((button) => [button.dataset.rootId, button.getBoundingClientRect().top]));
    }

    function animatePlaceReorder(previousTops) {
      if (window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches) return;
      const animated = [];
      for (const button of elements.places.querySelectorAll(".place[data-root-id]")) {
        const previousTop = previousTops.get(button.dataset.rootId);
        if (typeof previousTop !== "number") continue;
        const delta = previousTop - button.getBoundingClientRect().top;
        if (Math.abs(delta) < 0.5) continue;
        button.style.transition = "none";
        button.style.transform = `translateY(${delta}px)`;
        button.style.willChange = "transform";
        animated.push(button);
      }
      if (!animated.length) return;
      perf.sidebarReorderAnimationCount = (perf.sidebarReorderAnimationCount || 0) + 1;
      window.requestAnimationFrame(() => {
        for (const button of animated) {
          button.style.transition = "transform 160ms cubic-bezier(0.2, 0.8, 0.2, 1)";
          button.style.transform = "";
          button.addEventListener("transitionend", () => {
            button.style.transition = "";
            button.style.willChange = "";
          }, { once: true });
        }
      });
    }

    function syncPlacesActive() {
      const activeRoot = activeRootForUri(state.currentUri);
      for (const button of elements.places.querySelectorAll(".place[data-uri]")) {
        const uri = button.dataset.uri || "";
        const active = activeRoot?.uri === uri;
        button.className = active ? "place window-sidebar-item window-sidebar-item-active" : "place window-sidebar-item";
        button.dataset.active = active ? "true" : "false";
      }
    }

    function syncRouteChrome() {
      syncPlacesActive();
      renderBreadcrumbs();
      renderFooter();
    }

    function placeIcon(root) {
      const id = typeof root === "string" ? root : root?.id;
      if (id === "trash") {
        return root?.metadata?.empty === false ? "icons/trash-full.svg" : "icons/trash.svg";
      }
      return {
        home: "icons/sidebar-folder-home.svg",
        desktop: "icons/sidebar-folder-desktop.svg",
        documents: "icons/sidebar-folder-documents.svg",
        pictures: "icons/sidebar-folder-pictures.svg",
        videos: "icons/sidebar-folder-videos.svg",
        downloads: "icons/sidebar-folder.svg",
        public: "icons/sidebar-folder-public.svg",
        webspaces: "icons/sidebar-folder.svg",
      }[id] || "icons/sidebar-folder.svg";
    }

    function renderBreadcrumbs() {
      elements.breadcrumbs.innerHTML = "";
      const root = rootForUri(state.currentUri);
      if (!root) return;
      const segments = state.currentUri === root.uri
        ? []
        : state.currentUri.slice(root.uri.length + 1).split("/").filter(Boolean);
      const rootButton = crumbButton(root.label, root.uri, segments.length === 0);
      elements.breadcrumbs.appendChild(rootButton);
      let cursor = root.uri;
      for (let index = 0; index < segments.length; index += 1) {
        cursor += "/" + segments[index];
        elements.breadcrumbs.appendChild(pathSeparator());
        elements.breadcrumbs.appendChild(crumbButton(decodeURIComponent(segments[index]), cursor, index === segments.length - 1));
      }
      elements.currentTitle.textContent = segments.length ? decodeURIComponent(segments[segments.length - 1]) : root.label;
    }

    function pathSeparator() {
      const separator = document.createElement("span");
      separator.className = "path-seperator";
      separator.textContent = "/";
      return separator;
    }

    function crumbButton(label, uri, current) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = current ? "crumb crumb-current" : "crumb";
      button.textContent = label;
      button.dataset.uri = uri;
      button.disabled = current;
      return button;
    }

    function visibleObjects() {
      return visibleObjectsForState(state);
    }

    function showMenuForObject(object, x, y) {
      if (!isSelected(object.uri)) {
        selectOnly(object.uri);
      }
      const selection = selectedObjects();
      if (selection.length > 1) {
        showMenuForSelection(x, y);
        return;
      }
      const actions = [];
      if (isBlockedObject(object)) {
        actions.push(menuAction("Properties", () => showProperties(object)));
        renderMenu(actions, x, y);
        return;
      }
      if (isDirectory(object)) {
        actions.push(menuAction("Open", () => navigate(object.uri)));
        if (!inTrash(object)) {
          actions.push(menuAction("Open in New Window", () => openTarget("library", { uri: object.uri })));
        }
      } else {
        actions.push(menuAction(
          isAttachMode()
            ? (state.returnTarget === "browser" ? "Select for Browser" : "Attach to Chat")
            : "Open",
          () => openObject(object),
        ));
        const viewers = viewerOptions(object);
        if (!isAttachMode() && viewers.length) {
          actions.push(menuAction("Open With", null, {
            children: viewers.map((viewer) => menuAction(viewer.label || viewer.id, () => openWithViewer(object, viewer.id))),
          }));
        }
      }
      if (hasCapability(object, "download")) {
        actions.push(menuAction("Download", () => downloadObject(object)));
        if (isDirectory(object) && !isWebSpaceUri(object.uri)) {
          actions.push(menuAction("Download as ZIP", () => downloadObjectAsZip(object)));
        }
      }
      if (!inTrash(object) && hasCapability(object, "compress_archive")) {
        actions.push(menuAction("Compress to ZIP", () => compressObjectToZip(object)));
      }
      if (!inTrash(object)) {
        if (!isDirectory(object)) {
          if (hasCapability(object, "extract_archive")) {
            actions.push(menuAction("Extract Here", () => extractArchiveObject(object)));
          } else if (isPolicyGatedArchive(object)) {
            actions.push(menuAction("Archive Support", () => showArchiveSupport(object)));
          }
            if (object.published) {
              actions.push(menuAction("Status", () => showStatusObject(object)));
              if (hasCapability(object, "repair")) actions.push(menuAction("Repair", () => repairObject(object)));
              if (object.shared) actions.push(menuAction("Check My Access", () => checkMyAccess(object)));
              if (hasCapability(object, "share")) actions.push(menuAction("Share", () => shareObject(object)));
              if (hasCapability(object, "unpublish")) actions.push(menuAction("Unpublish", () => unpublishObject(object)));
          } else if (hasCapability(object, "publish")) {
            actions.push(menuAction("Publish", () => publishObject(object)));
          }
        }
        actions.push("-");
        if (hasCapability(object, "move")) actions.push(menuAction("Cut", () => setClipboard("move", [object])));
        if (hasCapability(object, "copy")) actions.push(menuAction("Copy", () => setClipboard("copy", [object])));
        if (isDirectory(object) && canPasteInto(object.uri)) {
          actions.push(menuAction("Paste Into Folder", () => pasteClipboardTo(object.uri)));
        }
        actions.push("-");
        if (hasCapability(object, "trash")) actions.push(menuAction("Delete", () => trashObject(object)));
        if (hasCapability(object, "delete_permanently")) actions.push(menuAction("Delete Permanently", () => deleteObject(object)));
        if (hasCapability(object, "rename")) actions.push(menuAction("Rename", () => startRename(object)));
      }
      if (inTrash(object)) {
        if (hasCapability(object, "restore")) actions.push(menuAction("Restore", () => restoreObject(object)));
        if (hasCapability(object, "delete_permanently")) actions.push(menuAction("Delete Permanently", () => deleteObject(object)));
      }
      actions.push("-");
      const localContentCid = contentCid(object);
      const publicCid = publishedCid(object);
      if (localContentCid) {
        actions.push(menuAction("Copy Content CID", () => copyText(localContentCid, "content CID")));
      }
      if (object.published && publicCid) {
        actions.push(menuAction("Copy Published Link", () => copyText("elastos://" + publicCid, "published link")));
      }
      actions.push(menuAction("Properties", () => showProperties(object)));
      renderMenu(actions, x, y);
    }

    function isPolicyGatedArchive(object) {
      return object?.metadata?.archive_support?.status === "policy_gated_unsupported_archive_family";
    }

    function showArchiveSupport(object) {
      const support = object?.metadata?.archive_support || {};
      const family = support.family || "archive";
      const archiveViewer = viewerOptions(object).find((viewer) => viewer?.id === "archive-manager");
      if (archiveViewer && openWithViewer(object, archiveViewer.id)) {
        setStatus(`Opening ${object.name || "archive"} in Archive.`);
        return;
      }
      setStatus(`${object.name || "Archive"} is a ${family} archive. Extraction is disabled pending dependency and release-policy review.`);
      showProperties(object);
    }

    function showPlaceMenu(uri, x, y) {
      const root = state.roots.find((entry) => entry.uri === uri);
      if (!root) {
        hideMenu();
        return;
      }
      const actions = [
        menuAction("Open", () => navigate(root.uri)),
        menuAction("Open in New Window", () => openTarget("library", { uri: root.uri })),
      ];
      if (root.id === "trash" && root.metadata?.empty === false) {
        actions.push("-");
        actions.push(menuAction("Empty Trash", emptyTrash));
      }
      renderMenu(actions, x, y);
    }

    function showMenuForSelection(x, y) {
      const actions = [];
      const objects = selectedObjects();
      const files = objects.filter((object) => !isDirectory(object) && !inTrash(object) && !isBlockedObject(object));
      const unpublished = files.filter((object) => !object.published);
      const published = files.filter((object) => object.published);
      const trash = objects.filter(inTrash);
      const active = objects.filter((object) => !inTrash(object) && !isBlockedObject(object));
      const downloadable = active.filter((object) => !isWebSpaceUri(object.uri) && hasCapability(object, "download"));
      const compressible = active.filter((object) => !isWebSpaceUri(object.uri) && hasCapability(object, "compress_archive"));
      const permanentlyDeletable = trash.length || active.some((object) => hasCapability(object, "delete_permanently"));
      if (downloadable.length > 1 && downloadable.length === active.length) {
        actions.push(menuAction("Download Selected", downloadSelectedObjects));
        actions.push(menuAction("Download Selected as ZIP", downloadSelectedObjectsAsZip));
      }
      if (
        compressible.length > 1 &&
        compressible.length === active.length &&
        compressible.every((object) => parentUri(object.uri) === parentUri(compressible[0].uri))
      ) {
        actions.push(menuAction("Compress Selected to ZIP", compressSelectedObjectsToZip));
      }
      if (unpublished.some((object) => hasCapability(object, "publish"))) actions.push(menuAction("Publish Selected", publishSelectedObjects));
      if (published.some((object) => hasCapability(object, "unpublish"))) actions.push(menuAction("Unpublish Selected", unpublishSelectedObjects));
      if (active.length) actions.push("-");
      if (active.some((object) => hasCapability(object, "move"))) actions.push(menuAction("Cut", () => setClipboard("move", active)));
      if (active.some((object) => hasCapability(object, "copy"))) actions.push(menuAction("Copy", () => setClipboard("copy", active)));
      if (active.some((object) => hasCapability(object, "trash"))) actions.push(menuAction("Delete", trashSelectedObjects));
      if (trash.length) actions.push(menuAction("Restore", restoreSelectedObjects));
      if (permanentlyDeletable) actions.push(menuAction("Delete Permanently", deleteSelectedObjects));
      renderMenu(actions, x, y);
    }

    function showBackgroundMenu(x, y) {
      const readOnly = currentFolderReadOnly();
      const actions = [];
      actions.push(menuAction("Sort By", null, {
        children: [
          menuAction("Name", () => setSort("name"), { checked: state.sort === "name" }),
          menuAction("Date Modified", () => setSort("modified"), { checked: state.sort === "modified" }),
          menuAction("Type", () => setSort("type"), { checked: state.sort === "type" }),
          menuAction("Size", () => setSort("size"), { checked: state.sort === "size" }),
          "-",
          menuAction("Ascending", () => setSortOrder("asc"), { checked: state.sortOrder !== "desc" }),
          menuAction("Descending", () => setSortOrder("desc"), { checked: state.sortOrder === "desc" }),
        ],
      }));
      actions.push(menuAction("View", null, {
        children: [
          menuAction("Icons", () => setView("grid"), { checked: state.view !== "list" }),
          menuAction("Details", () => setView("list"), { checked: state.view === "list" }),
        ],
      }));
      actions.push("-");
      actions.push(menuAction("Refresh", loadCurrentFolder));
      actions.push(menuAction("Show Hidden", () => toggleShowHidden(), { checked: state.showHidden }));
      actions.push("-");
      if (!readOnly) {
        actions.push(menuAction("New", null, {
          children: [
            menuAction("Folder", createFolder),
            menuAction("Text Document", createTextDocument),
          ],
        }));
        actions.push("-");
      }
      if (canPasteInto(state.currentUri)) actions.push(menuAction("Paste", () => pasteClipboardTo(state.currentUri)));
      if (!readOnly) actions.push(menuAction("Upload Here", () => elements.fileInput.click()));
      actions.push("-");
      actions.push(menuAction("Properties", () => showFolderProperties()));
      renderMenu(actions, x, y);
    }

    function showFolderProperties() {
      const root = rootForUri(state.currentUri);
      showProperties({
        uri: state.currentUri,
        name: root?.uri === state.currentUri ? root.label : baseName(state.currentUri),
        kind: "directory",
        mime: "inode/directory",
        size: 0,
        modified_at: 0,
        revision: "-",
        availability: "local-only",
        viewers: [],
      });
    }

    function setSort(sort) {
      state.sort = sort || "name";
      elements.sortSelect.value = state.sort;
      localStorage.setItem("library.sort", state.sort);
      scheduleContentRender();
    }

    function setSortOrder(order) {
      state.sortOrder = order === "desc" ? "desc" : "asc";
      localStorage.setItem("library.sortOrder", state.sortOrder);
      scheduleContentRender();
    }

    function toggleShowHidden() {
      state.showHidden = !state.showHidden;
      localStorage.setItem("library.showHidden", String(state.showHidden));
      scheduleContentRender();
    }

    function setView(view) {
      state.view = view === "list" ? "list" : "grid";
      localStorage.setItem("library.view", state.view);
      syncContentViewMode();
      syncViewButtons();
      renderFooter();
    }

    function showError(error) {
      console.error(error);
      setStatus(error && error.message ? error.message : "Library action failed.");
    }

    function bindEvents() {
      elements.pickerActionButton.addEventListener("click", () => {
        const action = isAttachMode() ? completeAttachPicker : completeArchivePicker;
        action().catch(showError);
      });
      bindLibraryEvents({
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
        selectedObjects,
        selectOnly,
        selectRangeTo,
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
      });
    }

    async function boot() {
      if (!state.homeToken) {
        elements.lockedShell.classList.remove("hidden");
        return;
      }
      elements.libraryShell.classList.remove("hidden");
      elements.content.dataset.view = state.view;
      syncModeChrome();
      bindEvents();
      try {
        await loadRoots();
        installBrowserHistory();
        await loadCurrentFolder();
        scheduleRootPrefetch();
        startLibraryEventStream();
      } catch (error) {
        showError(error);
        elements.content.innerHTML = `<div class="empty"><div><h2>Could not load Library</h2><p>${escapeHtml(error.message || "Runtime object provider unavailable.")}</p></div></div>`;
      }
    }

    boot();
