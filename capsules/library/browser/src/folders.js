import { isDirectory } from "./model.js";
import { cacheFolderListing } from "./state.js";

export function createLibraryFolders({
  homeNavigation,
  perf,
  providerApi,
  renderAll,
  runInitialObjectAction,
  setFolderStatus,
  setObjects,
  setStatus,
  state,
}) {
  async function loadCurrentFolder(options = {}) {
    if (!state.currentUri) return;
    const loadSeq = ++state.loadSeq;
    const uri = state.currentUri;
    const cached = options.useCache ? state.folderCache.get(uri) : null;
    const sameFolder = state.objectsUri === uri;
    let renderedCached = false;
    let renderAfterFetch = true;
    let loaded = false;
    state.loadError = false;
    if (cached) {
      perf.folderCacheHits += 1;
      state.loading = false;
      state.currentObject = cached.object || null;
      state.objectsUri = uri;
      setObjects(cached.objects);
      state.selectedUris.clear();
      state.selectionAnchorUri = "";
      setFolderStatus(`${state.objects.length} item${state.objects.length === 1 ? "" : "s"}.`);
      renderAll();
      await runInitialObjectAction();
      renderedCached = true;
    } else {
      state.loading = true;
      setStatus("Loading...");
      if (!sameFolder) {
        state.currentObject = null;
        state.objectsUri = "";
        setObjects([]);
        state.selectedUris.clear();
        state.selectionAnchorUri = "";
        renderAll();
      }
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
      state.objectsUri = uri;
      loaded = true;
      if (state.mode === "browse" && currentObject?.uri === uri && isDirectory(currentObject)) {
        homeNavigation.setQuery({ uri });
      }
      if (renderedCached && cached.signature === nextCache.signature) {
        renderAfterFetch = false;
        setFolderStatus(`${state.objects.length} item${state.objects.length === 1 ? "" : "s"}.`);
        return;
      }
      setObjects(objects);
      state.selectedUris.clear();
      state.selectionAnchorUri = "";
      setFolderStatus(`${state.objects.length} item${state.objects.length === 1 ? "" : "s"}.`);
    } catch (error) {
      // A later navigation owns both the rows and any failure message.
      if (loadSeq !== state.loadSeq || uri !== state.currentUri) return;
      state.loadError = true;
      throw error;
    } finally {
      if (loadSeq === state.loadSeq && uri === state.currentUri) {
        state.loading = false;
        if (renderAfterFetch) renderAll();
        if (renderAfterFetch && loaded) await runInitialObjectAction();
      }
    }
  }

  return { loadCurrentFolder };
}
