import {
  escapeHtml,
  formatBytes,
  formatTime,
  iconFor,
  inTrash,
  isWebSpaceUri,
  isBlockedObject,
  isDirectory,
  visibilityContract,
} from "./model.js";
import { getTag, tagColor } from "./tags.js";

const LARGE_RENDER_THRESHOLD = 240;
const INITIAL_RENDER_LIMIT = 120;
const RENDER_CHUNK_SIZE = 180;
const OBJECT_NODE_CACHE_LIMIT = 3_000;

export function iconPlaceholder(src, className) {
  const safeSrc = escapeHtml(src);
  return `<span class="${className}" aria-hidden="true"><img src="${safeSrc}" alt="" loading="eager" decoding="async"></span>`;
}

export function createLibraryRenderer({
  elements,
  isSelected,
  perf,
  selectedObjects,
  state,
  visibleObjects,
}) {
  function renderContent() {
    const startedAt = performance.now();
    const job = ++state.contentRenderJob;
    elements.content.dataset.view = state.view === "list" ? "list" : "grid";
    elements.sortSelect.value = state.sort;
    const objects = visibleObjects();
    if (!objects.length) {
      elements.content.dataset.empty = "true";
      const copy = emptyStateCopy(state);
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.innerHTML = `<div class="empty-inner"><h2>${escapeHtml(copy.title)}</h2><p>${escapeHtml(copy.body)}</p></div>`;
      elements.content.replaceChildren(empty);
      perf.contentRenderCount += 1;
      perf.lastContentRender = {
        durationMs: performance.now() - startedAt,
        objectCount: 0,
        view: state.view,
      };
      return;
    }
    elements.content.dataset.empty = "false";
    const chunked = objects.length > LARGE_RENDER_THRESHOLD;
    const firstLimit = chunked ? Math.min(INITIAL_RENDER_LIMIT, objects.length) : objects.length;
    const fragment = document.createDocumentFragment();
    if (state.view === "list") {
      fragment.appendChild(renderListHeader());
    }
    for (const object of objects.slice(0, firstLimit)) {
      fragment.appendChild(renderObject(object));
    }
    elements.content.replaceChildren(fragment);
    pruneObjectNodeCache(objects);
    perf.contentRenderCount += 1;
    perf.lastContentRender = {
      durationMs: performance.now() - startedAt,
      objectCount: objects.length,
      initialRenderedCount: firstLimit,
      renderedCount: firstLimit,
      complete: !chunked,
      chunked,
      view: state.view,
    };
    if (chunked) {
      renderContentChunks(objects, firstLimit, job, startedAt);
    }
  }

  function emptyStateCopy(state) {
    if (state.query) {
      return { title: "No matching items", body: "Try another search." };
    }
    if (isWebSpaceUri(state.currentUri)) {
      return {
        title: "This space is empty",
        body: "Add files or folders to this space.",
      };
    }
    return { title: "This folder is empty", body: "Upload a file or create a folder." };
  }

  function renderContentChunks(objects, startIndex, job, startedAt) {
    window.requestAnimationFrame(() => {
      if (job !== state.contentRenderJob) return;
      const fragment = document.createDocumentFragment();
      const nextIndex = Math.min(startIndex + RENDER_CHUNK_SIZE, objects.length);
      for (const object of objects.slice(startIndex, nextIndex)) {
        fragment.appendChild(renderObject(object));
      }
      elements.content.appendChild(fragment);
      if (nextIndex < objects.length) {
        renderContentChunks(objects, nextIndex, job, startedAt);
        return;
      }
      perf.lastContentRender = {
        ...(perf.lastContentRender || {}),
        durationMs: perf.lastContentRender?.durationMs || 0,
        completeDurationMs: performance.now() - startedAt,
        renderedCount: objects.length,
        complete: true,
      };
    });
  }

  function renderListHeader() {
    const header = document.createElement("div");
    header.className = "explore-table-headers";
    header.innerHTML = `
      <span class="explore-table-headers-th explore-table-headers-th--name">Name</span>
      <span class="explore-table-headers-th explore-table-headers-th--modified">Modified</span>
      <span class="explore-table-headers-th explore-table-headers-th--size">Size</span>
      <span class="explore-table-headers-th explore-table-headers-th--type">Type</span>
    `;
    return header;
  }

  function renderObject(object) {
    const signature = objectRenderSignature(object);
    const cached = state.objectNodeCache.get(object.uri);
    if (cached && cached.signature === signature && !cached.node.querySelector(".rename-input")) {
      perf.objectNodeCacheHits += 1;
      cached.node.dataset.selected = isSelected(object.uri) ? "true" : "false";
      state.objectNodeCache.delete(object.uri);
      state.objectNodeCache.set(object.uri, cached);
      return cached.node;
    }
    perf.objectNodeCacheMisses += 1;
    const item = document.createElement("article");
    item.className = "item";
    item.dataset.uri = object.uri;
    item.dataset.kind = object.kind;
    item.dataset.blocked = isBlockedObject(object) ? "true" : "false";
    item.dataset.selected = isSelected(object.uri) ? "true" : "false";
    item.draggable = !isBlockedObject(object);
    item.tabIndex = 0;
    const visibility = visibilityContract(object);
    const inPublicFolder = visibility.placement === "public_folder";
    const badges = [
      isBlockedObject(object) ? '<span class="badge badge-blocked">Blocked</span>' : "",
      inPublicFolder && !object.published ? '<span class="badge badge-public-folder">Public folder</span>' : "",
      object.published ? '<span class="badge badge-published">Published</span>' : "",
      inTrash(object) ? '<span class="badge badge-trash">Trash</span>' : "",
    ].join("");
    const badgesMarkup = badges ? `<span class="badges">${badges}</span>` : "";
    const tag = getTag(object.uri);
    const tagHex = tag ? tagColor(tag)?.hex || "#9aa0a6" : "";
    // List view keeps the right-aligned badge dot (a separate cell). Grid view uses an inline dot
    // placed just left of the centered filename — desktop file-manager style — so it never displaces the icon.
    const tagMarkup = tag
      ? `<span class="item-tag" style="--tag-color:${escapeHtml(tagHex)}" title="Tag: ${escapeHtml(tag)}" aria-label="Tag: ${escapeHtml(tag)}"></span>`
      : "";
    const tagDotInline = tag
      ? `<span class="item-tag-inline" style="--tag-color:${escapeHtml(tagHex)}" aria-hidden="true"></span>`
      : "";
    item.innerHTML = `
      ${iconPlaceholder(iconFor(object), "file-icon")}
      <span class="item-name" data-name-uri="${escapeHtml(object.uri)}" title="${escapeHtml(object.name)}">${tagDotInline}<span class="item-name-text">${escapeHtml(object.name)}</span></span>
      ${tagMarkup}
      <span class="item-date">${escapeHtml(formatTime(object.modified_at))}</span>
      <span class="item-size">${escapeHtml(isDirectory(object) ? "-" : formatBytes(object.size))}</span>
      <span class="item-type">${escapeHtml(isDirectory(object) ? "Folder" : object.mime || "File")}</span>
      ${badgesMarkup}
    `;
    state.objectNodeCache.set(object.uri, { signature, node: item });
    return item;
  }

  function objectRenderSignature(object) {
    return [
      object.uri,
      getTag(object.uri),
      object.kind,
      object.name,
      object.mime || "",
      object.size || 0,
      object.modified_at || 0,
      object.revision || "",
      object.content_cid || "",
      object.published_cid || "",
      object.availability || "",
      object.metadata?.visibility?.placement || "",
      object.metadata?.visibility?.effective_access || "",
      object.published ? 1 : 0,
      object.shared ? 1 : 0,
      object.blocked_reason || "",
      inTrash(object) ? 1 : 0,
      isBlockedObject(object) ? 1 : 0,
    ].join("\u0000");
  }

  function pruneObjectNodeCache(visible) {
    if (state.objectNodeCache.size <= OBJECT_NODE_CACHE_LIMIT) return;
    const visibleUris = new Set(visible.map((object) => object.uri));
    for (const uri of state.objectNodeCache.keys()) {
      if (state.objectNodeCache.size <= OBJECT_NODE_CACHE_LIMIT) break;
      if (!visibleUris.has(uri)) state.objectNodeCache.delete(uri);
    }
  }

  function renderFooter() {
    const visible = visibleObjects().length;
    const selected = selectedObjects().length;
    const prefix = selected ? `${selected} selected · ` : "";
    elements.footerLeft.textContent = `${prefix}${visible} item${visible === 1 ? "" : "s"}`;
    elements.footerRight.textContent = elements.currentTitle.textContent || "Library";
  }

  function scheduleContentRender() {
    if (state.contentRenderFrame) return;
    state.contentRenderFrame = window.requestAnimationFrame(() => {
      state.contentRenderFrame = 0;
      renderContent();
      renderFooter();
    });
  }

  function syncViewButtons() {
    elements.gridButton.classList.toggle("layout-toggle-segment-active", state.view !== "list");
    elements.listButton.classList.toggle("layout-toggle-segment-active", state.view === "list");
    elements.gridButton.setAttribute("aria-pressed", state.view !== "list" ? "true" : "false");
    elements.listButton.setAttribute("aria-pressed", state.view === "list" ? "true" : "false");
  }

  function syncContentViewMode() {
    elements.content.dataset.view = state.view === "list" ? "list" : "grid";
    elements.content.querySelector(".explore-table-headers")?.remove();
    if (state.view === "list" && visibleObjects().length) {
      elements.content.prepend(renderListHeader());
    }
  }

  return {
    renderContent,
    renderFooter,
    scheduleContentRender,
    syncContentViewMode,
    syncViewButtons,
  };
}
