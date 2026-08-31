export const MUTATING_PROVIDER_OPS = new Set([
  "write",
  "mkdir",
  "rename",
  "move",
  "copy",
  "trash",
  "restore",
  "delete_permanently",
  "empty_trash",
  "extract_archive",
  "compress_archive",
  "publish",
  "unpublish",
  "repair",
  "share",
]);

export function createLibraryState({ queryParams, storage, perfTarget }) {
  const rawMode = queryParams.get("mode") || "";
  const state = {
    homeToken: new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "",
    mode: ["attach", "archive-open", "archive-create"].includes(rawMode) ? rawMode : "browse",
    returnTarget: queryParams.get("returnTarget") || "",
    initialUri: queryParams.get("uri") || "",
    initialObjectUri: queryParams.get("objectUri") || "",
    initialAction: queryParams.get("action") || "",
    initialActionHandled: false,
    roots: [],
    currentUri: "",
    currentObject: null,
    objects: [],
    objectsByUri: new Map(),
    objectsVersion: 0,
    visibleCacheKey: "",
    visibleCache: [],
    folderCache: new Map(),
    folderPrefetches: new Map(),
    rootPrefetchStarted: false,
    contentRenderFrame: 0,
    contentRenderJob: 0,
    objectNodeCache: new Map(),
    selectedUris: new Set(),
    selectionAnchorUri: "",
    view: storage.getItem("library.view") || "list",
    sort: storage.getItem("library.sort") || "name",
    sortOrder: storage.getItem("library.sortOrder") || "asc",
    sidebarOrder: readStoredStringArray(storage.getItem("library.sidebarOrder")),
    showHidden: storage.getItem("library.showHidden") === "true",
    query: "",
    loading: false,
    loadSeq: 0,
    eventSource: null,
    eventReconnectTimer: null,
    eventRefreshTimer: null,
    uploads: [],
    previewUrl: "",
    draftCounter: 0,
    backStack: [],
    forwardStack: [],
    historyEntries: [],
    historyIndex: -1,
    historyKeyCounter: 0,
    clipboard: {
      op: "",
      uris: [],
    },
  };
  const perf = perfTarget || {};
  Object.assign(perf, {
    iconFetchCount: 0,
    renderPlacesCount: 0,
    contentRenderCount: 0,
    menuRenderCount: 0,
    uploadRenderCount: 0,
    uploadRenderScheduledCount: 0,
    lastContentRender: null,
    lastMenuRender: null,
    folderCacheHits: 0,
    folderCacheSize: 0,
    objectNodeCacheHits: 0,
    objectNodeCacheMisses: 0,
  });
  return { state, perf };
}

function readStoredStringArray(value) {
  try {
    const parsed = JSON.parse(value || "[]");
    return Array.isArray(parsed)
      ? parsed.filter((entry) => typeof entry === "string" && entry)
      : [];
  } catch {
    return [];
  }
}

export function setLibraryObjects(state, objects) {
  state.objects = Array.isArray(objects) ? objects : [];
  state.objectsByUri = new Map(state.objects.map((object) => [object.uri, object]));
  state.objectsVersion += 1;
  invalidateVisibleCache(state);
}

export function cacheFolderListing(state, perf, uri, objects, object = null) {
  const safeObjects = Array.isArray(objects) ? objects : [];
  const cached = {
    object,
    objects: safeObjects,
    signature: `${folderListingSignature(safeObjects)}\u0000${folderObjectSignature(object)}`,
  };
  state.folderCache.set(uri, cached);
  perf.folderCacheSize = state.folderCache.size;
  return cached;
}

function folderObjectSignature(object) {
  if (!object) return "";
  const metadata = object.metadata || {};
  return [
    object.uri || "",
    metadata.readonly === false ? "writable" : "readonly",
    metadata.access_policy || "",
    metadata.webspace_kind || "",
  ].join("\u0001");
}

export function visibleObjectsForState(state) {
  const cacheKey = [
    state.objectsVersion,
    state.query,
    state.showHidden ? "hidden" : "visible",
    state.sort,
    state.sortOrder,
  ].join("\u0000");
  if (state.visibleCacheKey === cacheKey) return state.visibleCache;
  const query = state.query.trim().toLowerCase();
  const objects = state.objects.filter((object) => {
    if (!state.showHidden && String(object.name || "").startsWith(".")) return false;
    if (!query) return true;
    return [object.name, object.uri, object.mime, object.content_cid || "", object.published_cid || ""]
      .join("\n")
      .toLowerCase()
      .includes(query);
  });
  objects.sort((left, right) => {
    if (left.kind !== right.kind) {
      return left.kind === "directory" ? -1 : 1;
    }
    let result = 0;
    if (state.sort === "modified") result = Number(left.modified_at || 0) - Number(right.modified_at || 0);
    else if (state.sort === "size") result = Number(left.size || 0) - Number(right.size || 0);
    else if (state.sort === "type") result = String(left.mime || "").localeCompare(String(right.mime || ""));
    else result = String(left.name || "").localeCompare(String(right.name || ""), undefined, { sensitivity: "base" });
    return state.sortOrder === "desc" ? -result : result;
  });
  state.visibleCacheKey = cacheKey;
  state.visibleCache = objects;
  return objects;
}

function invalidateVisibleCache(state) {
  state.visibleCacheKey = "";
  state.visibleCache = [];
}

function folderListingSignature(objects) {
  return JSON.stringify((Array.isArray(objects) ? objects : []).map((object) => [
    object.uri,
    object.revision,
    object.kind,
    object.name,
    object.size,
    object.modified_at,
    object.content_cid || "",
    object.published_cid || "",
    object.availability || "",
    object.metadata?.visibility?.placement || "",
    object.metadata?.visibility?.effective_access || "",
    object.published ? 1 : 0,
    object.shared ? 1 : 0,
    object.blocked_reason || "",
  ]));
}
