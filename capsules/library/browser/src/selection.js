export function createLibrarySelection({
  content,
  renderFooter,
  state,
  visibleObjects,
}) {
  let visibleUriSource = null;
  let visibleUriSet = new Set();

  function objectByUri(uri) {
    return state.objectsByUri.get(uri);
  }

  function visibleUris() {
    const visible = visibleObjects();
    if (visible !== visibleUriSource) {
      visibleUriSource = visible;
      visibleUriSet = new Set(visible.map((object) => object.uri));
    }
    return visibleUriSet;
  }

  function allVisible(uris) {
    const visible = visibleUris();
    return uris.every((uri) => visible.has(uri));
  }

  // Only items the person can see are selected or acted on (#441).
  function selectedObjects() {
    const visible = visibleUris();
    return Array.from(state.selectedUris)
      .filter((uri) => visible.has(uri))
      .map((uri) => state.objectsByUri.get(uri))
      .filter(Boolean);
  }

  function pruneSelectionToVisible() {
    const visible = visibleUris();
    for (const uri of state.selectedUris) {
      if (!visible.has(uri)) state.selectedUris.delete(uri);
    }
    if (state.selectionAnchorUri && !visible.has(state.selectionAnchorUri)) {
      state.selectionAnchorUri = "";
    }
  }

  function isSelected(uri) {
    return state.selectedUris.has(uri);
  }

  function selectOnly(uri) {
    state.selectedUris.clear();
    if (uri) state.selectedUris.add(uri);
    state.selectionAnchorUri = uri || "";
    syncSelectionDom();
  }

  function toggleSelected(uri) {
    if (!uri) return;
    if (state.selectedUris.has(uri)) {
      state.selectedUris.delete(uri);
    } else {
      state.selectedUris.add(uri);
    }
    state.selectionAnchorUri = uri;
    syncSelectionDom();
  }

  function selectRangeTo(uri, extend = false) {
    if (!uri) return;
    const visible = visibleObjects();
    const anchorUri = state.selectionAnchorUri || Array.from(state.selectedUris)[0] || uri;
    const anchorIndex = visible.findIndex((object) => object.uri === anchorUri);
    const targetIndex = visible.findIndex((object) => object.uri === uri);
    if (anchorIndex < 0 || targetIndex < 0) {
      selectOnly(uri);
      return;
    }
    if (!extend) state.selectedUris.clear();
    const start = Math.min(anchorIndex, targetIndex);
    const end = Math.max(anchorIndex, targetIndex);
    for (const object of visible.slice(start, end + 1)) {
      state.selectedUris.add(object.uri);
    }
    state.selectionAnchorUri = anchorUri;
    syncSelectionDom();
  }

  function selectAllVisible() {
    const visible = visibleObjects();
    state.selectedUris = new Set(visible.map((object) => object.uri));
    state.selectionAnchorUri = visible[0]?.uri || "";
    syncSelectionDom();
  }

  function clearSelection(render = true) {
    state.selectedUris.clear();
    state.selectionAnchorUri = "";
    if (render) {
      syncSelectionDom();
    }
  }

  function syncSelectionDom() {
    for (const item of content.querySelectorAll(".item[data-uri]")) {
      item.dataset.selected = state.selectedUris.has(item.dataset.uri) ? "true" : "false";
    }
    renderFooter();
  }

  function prepareDragSelection(uri, item) {
    if (!isSelected(uri)) {
      state.selectedUris.clear();
      state.selectedUris.add(uri);
      state.selectionAnchorUri = uri;
      if (item) item.dataset.selected = "true";
      syncSelectionDom();
    }
  }

  return {
    allVisible,
    clearSelection,
    isSelected,
    objectByUri,
    prepareDragSelection,
    pruneSelectionToVisible,
    selectAllVisible,
    selectedObjects,
    selectOnly,
    selectRangeTo,
    syncSelectionDom,
    toggleSelected,
  };
}
