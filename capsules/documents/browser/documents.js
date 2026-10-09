import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";
import {
  createHomeClipboardClient,
} from "/apps/home/home-clipboard-client.js?v=home-20260726a";

const state = {
  homeToken: new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "",
  initialDocDid: new URLSearchParams(window.location.search).get("doc") || "",
  initialCid: new URLSearchParams(window.location.search).get("cid") || "",
  initialLibraryObjectUri:
    new URLSearchParams(window.location.search).get("objectUri") ||
    new URLSearchParams(window.location.search).get("object_uri") ||
    "",
  initialView: new URLSearchParams(window.location.search).get("view") || "",
  initialIntent: new URLSearchParams(window.location.search).get("intent") || "",
  mode: "share",
  documents: [],
  current: null,
  currentSessionId: 0,
  dirty: false,
  workspaceView: "write",
  inspectorView: "preview",
  sidebarCollapsed: false,
  filterQuery: "",
  pendingConfirmation: null,
  pendingChatAttachmentId: "",
};
const homeOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";
const homeNavigation = createHomeNavigationClient({ homeToken: state.homeToken, homeOrigin });
const DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE =
  "elastos.documents.window-close.request/v1";
const DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE =
  "elastos.documents.window-close.result/v1";
const homeClipboard = createHomeClipboardClient({
  targetId: "documents",
  homeOrigin,
  homeToken: state.homeToken,
});
homeClipboard.start();

const elements = {
  documentsShell: document.getElementById("documents-shell"),
  documentsList: document.getElementById("documents-list"),
  sidebarSearch: document.getElementById("sidebar-search"),
  titleInput: document.getElementById("title-input"),
  sidebarToggle: document.getElementById("sidebar-toggle"),
  saveButton: document.getElementById("save-button"),
  saveAsButton: document.getElementById("save-as-button"),
  publishButton: document.getElementById("publish-button"),
  unpublishButton: document.getElementById("unpublish-button"),
  deleteButton: document.getElementById("delete-button"),
  statusRow: document.getElementById("status-row"),
  statusText: document.getElementById("status-text"),
  findBar: document.getElementById("find-bar"),
  findInput: document.getElementById("find-input"),
  findCount: document.getElementById("find-count"),
  findPrev: document.getElementById("find-prev"),
  findNext: document.getElementById("find-next"),
  findClose: document.getElementById("find-close"),
  contextMenu: document.getElementById("context-menu"),
  copyPublishedLink: document.getElementById("copy-published-link"),
  editor: document.getElementById("editor"),
  preview: document.getElementById("preview"),
  previewPanel: document.getElementById("preview-panel"),
  outlinePanel: document.getElementById("outline-panel"),
  historyPanel: document.getElementById("history-panel"),
  newButton: document.getElementById("new-document"),
  modeWrite: document.getElementById("mode-write"),
  modeSplit: document.getElementById("mode-split"),
  modeRead: document.getElementById("mode-read"),
  panePreview: document.getElementById("pane-preview"),
  paneOutline: document.getElementById("pane-outline"),
  paneHistory: document.getElementById("pane-history"),
  shareShell: document.getElementById("share-shell"),
  shareSidebar: document.getElementById("share-sidebar"),
  shareSidebarMeta: document.getElementById("share-sidebar-meta"),
  shareDocList: document.getElementById("share-doc-list"),
  shareTopline: document.getElementById("share-topline"),
  shareTitle: document.getElementById("share-title"),
  shareSubtitle: document.getElementById("share-subtitle"),
  sharePreview: document.getElementById("share-preview"),
  saveAsModal: document.getElementById("save-as-modal"),
  saveAsForm: document.getElementById("save-as-form"),
  saveAsTitleInput: document.getElementById("save-as-title-input"),
  saveAsFileInput: document.getElementById("save-as-file-input"),
  saveAsCancel: document.getElementById("save-as-cancel"),
  confirmModal: document.getElementById("confirm-modal"),
  confirmTitle: document.getElementById("confirm-title"),
  confirmMessage: document.getElementById("confirm-message"),
  confirmCancel: document.getElementById("confirm-cancel"),
  confirmSecondaryAction: document.getElementById("confirm-secondary-action"),
  confirmAction: document.getElementById("confirm-action"),
};

let autosaveTimerId = 0;
let statusClearTimerId = 0;
let saveInFlight = null;
let autosaveQueued = false;
let queuedSaveTarget = null;
let savedWorkingCopy = null;
let saveRecovery = null;
let documentSelectionSequence = 0;
let pendingHomeWindowCloseTarget = null;
let homeChromeReady = false;
let lastHomeMenuManifestSignature = "";
let findMatches = [];
let findIndex = -1;
let previewFindMarks = [];
let contextMenuDocDid = "";

function escapeHtml(value) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;");
}

function hasExactKeys(value, expectedKeys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length &&
    actual.every((key, index) => key === expected[index]);
}

function renderInlineMarkdown(text) {
  const tokens = /`([^`]+)`|\*\*([^*]+)\*\*|\*([^*]+)\*|\[([^\]]+)\]\(([^)]+)\)/g;
  let html = "";
  let offset = 0;
  for (const match of text.matchAll(tokens)) {
    html += escapeHtml(text.slice(offset, match.index));
    if (match[1] !== undefined) {
      html += "<code>" + escapeHtml(match[1]) + "</code>";
    } else if (match[2] !== undefined) {
      html += "<strong>" + renderInlineMarkdown(match[2]) + "</strong>";
    } else if (match[3] !== undefined) {
      html += "<em>" + renderInlineMarkdown(match[3]) + "</em>";
    } else {
      html += '<a href="' + safeMarkdownHref(match[5]) + '" rel="noopener noreferrer">' + renderInlineMarkdown(match[4]) + "</a>";
    }
    offset = match.index + match[0].length;
  }
  return html + escapeHtml(text.slice(offset));
}

function safeMarkdownHref(value) {
  const href = String(value || "").trim();
  if (/^(https?:\/\/|elastos:\/\/|localhost:\/\/|#|\/(?!\/)|\.{1,2}\/)/i.test(href)) {
    return escapeHtml(href);
  }
  return "#";
}

function headingSlug(text, number) {
  const slug = String(text || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return (slug || "section") + "-" + number;
}

function renderMarkdownDocument(source, options) {
  const interactiveTasks = !!(options && options.interactiveTasks);
  const lines = String(source || "").replace(/\r\n/g, "\n").split("\n");
  const blocks = [];
  const outline = [];
  let index = 0;
  let headingIndex = 0;

  while (index < lines.length) {
    const line = lines[index];

    if (/^```/.test(line)) {
      const fenceInfo = line.slice(3).trim();
      const language = /^[A-Za-z0-9+#-]+$/.test(fenceInfo) ? fenceInfo : "";
      const code = [];
      index += 1;
      while (index < lines.length && !/^```/.test(lines[index])) {
        code.push(escapeHtml(lines[index]));
        index += 1;
      }
      index += 1;
      blocks.push('<pre><code' + (language ? ' class="language-' + language + '"' : "") + ">" + code.join("\n") + "</code></pre>");
      continue;
    }

    if (/^(\*{3,}|-{3,}|_{3,})\s*$/.test(line)) {
      blocks.push("<hr>");
      index += 1;
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) {
      headingIndex += 1;
      const title = heading[2].trim();
      const id = headingSlug(title, headingIndex);
      blocks.push("<h" + heading[1].length + ' id="' + id + '">' + renderInlineMarkdown(title) + "</h" + heading[1].length + ">");
      outline.push({ depth: heading[1].length, title: title, id: id });
      index += 1;
      continue;
    }

    if (/^>\s?/.test(line)) {
      const quote = [];
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(renderInlineMarkdown(lines[index].replace(/^>\s?/, "")));
        index += 1;
      }
      blocks.push("<blockquote><p>" + quote.join("<br>") + "</p></blockquote>");
      continue;
    }

    if (/^[-*]\s+\[( |x|X)\]\s+/.test(line)) {
      const items = [];
      while (index < lines.length && /^[-*]\s+\[( |x|X)\]\s+/.test(lines[index])) {
        const taskLine = lines[index];
        const taskMatch = /^[-*]\s+\[( |x|X)\]\s+([\s\S]+)$/.exec(taskLine);
        const checked = !!taskMatch && String(taskMatch[1]).toLowerCase() === "x";
        const label = taskMatch ? taskMatch[2] : taskLine.replace(/^[-*]\s+/, "");
        const checkbox = interactiveTasks
          ? '<label class="task-item-label"><input class="task-checkbox" type="checkbox" aria-label="Toggle task completion" data-source-line="' + String(index) + '"' + (checked ? " checked" : "") + '><span>' + renderInlineMarkdown(label) + "</span></label>"
          : '<span>' + renderInlineMarkdown((checked ? "[x] " : "[ ] ") + label) + "</span>";
        items.push('<li class="task-item">' + checkbox + "</li>");
        index += 1;
      }
      blocks.push("<ul>" + items.join("") + "</ul>");
      continue;
    }

    if (/^[-*]\s+/.test(line)) {
      const items = [];
      while (index < lines.length && /^[-*]\s+/.test(lines[index])) {
        items.push("<li>" + renderInlineMarkdown(lines[index].replace(/^[-*]\s+/, "")) + "</li>");
        index += 1;
      }
      blocks.push("<ul>" + items.join("") + "</ul>");
      continue;
    }

    if (/^\d+\.\s+/.test(line)) {
      const items = [];
      while (index < lines.length && /^\d+\.\s+/.test(lines[index])) {
        items.push("<li>" + renderInlineMarkdown(lines[index].replace(/^\d+\.\s+/, "")) + "</li>");
        index += 1;
      }
      blocks.push("<ol>" + items.join("") + "</ol>");
      continue;
    }

    if (/^\|.*\|$/.test(line) && index + 1 < lines.length && /^\|?[\s:-]+\|[\s|:-]*$/.test(lines[index + 1])) {
      const headerCells = line.split("|").slice(1, -1).map((cell) => "<th>" + renderInlineMarkdown(cell.trim()) + "</th>");
      index += 2;
      const rows = [];
      while (index < lines.length && /^\|.*\|$/.test(lines[index])) {
        const cells = lines[index].split("|").slice(1, -1).map((cell) => "<td>" + renderInlineMarkdown(cell.trim()) + "</td>");
        rows.push("<tr>" + cells.join("") + "</tr>");
        index += 1;
      }
      blocks.push("<table><thead><tr>" + headerCells.join("") + "</tr></thead><tbody>" + rows.join("") + "</tbody></table>");
      continue;
    }

    if (!line.trim()) {
      index += 1;
      continue;
    }

    const paragraph = [];
    while (index < lines.length && lines[index].trim() && !/^(#{1,6})\s+/.test(lines[index]) && !/^```/.test(lines[index]) && !/^>\s?/.test(lines[index]) && !/^[-*]\s+/.test(lines[index]) && !/^\d+\.\s+/.test(lines[index]) && !/^(\*{3,}|-{3,}|_{3,})\s*$/.test(lines[index])) {
      paragraph.push(renderInlineMarkdown(lines[index]));
      index += 1;
    }
    if (!paragraph.length) {
      blocks.push("<p>" + renderInlineMarkdown(line) + "</p>");
      index += 1;
      continue;
    }
    blocks.push("<p>" + paragraph.join("<br>") + "</p>");
  }

  return {
    html: blocks.join("\n"),
    outline: outline,
  };
}

function shortCid(cid) {
  if (!cid) {
    return "";
  }
  if (cid.length <= 18) {
    return cid;
  }
  return cid.slice(0, 10) + "…" + cid.slice(-6);
}

function documentUri(docDid) {
  return docDid ? "localhost://ElastOS/Documents/" + docDid : "";
}

function documentIdentity(document) {
  if (!document) {
    return "";
  }
  if (isLibraryFileProjection(document)) {
    return "library:" + String(document.library_object_uri || document.document_uri || document.working_copy_uri || "");
  }
  if (document.doc_did) {
    return "document:" + String(document.doc_did);
  }
  return "draft";
}

function baseName(uri) {
  const clean = String(uri || "").replace(/\/+$/, "");
  const name = clean.split("/").pop() || "Document";
  try {
    return decodeURIComponent(name);
  } catch (_error) {
    return name;
  }
}

function formatBytes(bytes) {
  const value = Number(bytes || 0);
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB"];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  return (unitIndex === 0 ? String(amount) : amount.toFixed(amount >= 10 ? 1 : 2)) + " " + units[unitIndex];
}

function looksLikeTextContent(contentType, buffer) {
  const lower = String(contentType || "").toLowerCase();
  if (
    lower.startsWith("text/") ||
    lower.includes("json") ||
    lower.includes("markdown") ||
    lower.includes("xml") ||
    lower.includes("javascript")
  ) {
    return true;
  }
  const bytes = new Uint8Array(buffer);
  if (bytes.length === 0) {
    return true;
  }
  const sample = bytes.subarray(0, Math.min(bytes.length, 4096));
  if (sample.includes(0)) {
    return false;
  }
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(sample);
    return true;
  } catch (_error) {
    return false;
  }
}

function isTextMimeType(mime) {
  const lower = String(mime || "").toLowerCase();
  return lower.startsWith("text/") ||
    lower.includes("json") ||
    lower.includes("markdown") ||
    lower.includes("xml") ||
    lower.includes("javascript");
}

function dataUrlToUtf8(dataUrl) {
  const value = String(dataUrl || "");
  const match = /^data:([^;,]*)(;base64)?,([\s\S]*)$/i.exec(value);
  if (!match) {
    throw new Error("Chat attachment payload is not a data URL.");
  }
  if (match[2]) {
    return base64ToUtf8(match[3] || "");
  }
  return decodeURIComponent(match[3] || "");
}

function isLibraryFileProjection(document) {
  return !!document && !!document.library_object_uri;
}

function isDraftDocument(document) {
  return !!document && !document.doc_did && !isLibraryFileProjection(document);
}

function createDraftDocument() {
  return {
    doc_did: "",
    document_uri: "",
    title: "",
    file_name: "",
    working_copy_uri: "",
    body: "",
    created_at: 0,
    updated_at: 0,
    latest_published_cid: null,
    publish_history: [],
  };
}

function formatTime(timestamp) {
  if (!timestamp) {
    return "";
  }
  return new Date(timestamp * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function preferredWorkspaceView(view) {
  return view === "read" ? "read" : view === "split" ? "split" : "write";
}

function clearStatusTimer() {
  if (statusClearTimerId) {
    window.clearTimeout(statusClearTimerId);
    statusClearTimerId = 0;
  }
}

function scheduleStatusClear(delay = 1600) {
  clearStatusTimer();
  statusClearTimerId = window.setTimeout(() => {
    if (!state.dirty) {
      clearStatus();
    }
  }, delay);
}

function clearStatus() {
  clearStatusTimer();
  elements.statusText.textContent = "";
  elements.statusText.className = "status-text";
  elements.statusRow.classList.add("hidden");
}

function clearAutosaveTimer() {
  if (autosaveTimerId) {
    window.clearTimeout(autosaveTimerId);
    autosaveTimerId = 0;
  }
}

function resetPendingSaveIntent() {
  clearAutosaveTimer();
  autosaveQueued = false;
}

function replaceCurrentDocument(document) {
  resetPendingSaveIntent();
  queuedSaveTarget = null;
  saveRecovery = null;
  savedWorkingCopy = document ? { ...document } : null;
  documentSelectionSequence += 1;
  state.current = document;
  state.currentSessionId += 1;
  updateDocumentsNavigation(document);
}

function updateDocumentsNavigation(document) {
  // A settled editor selection replaces the hint; initial loading stays null.
  let query = {};
  if (document?.library_object_uri && typeof document.revision === "string" && document.revision) {
    query = { objectUri: document.library_object_uri };
  } else if (document?.doc_did && /^[0-9a-f]{64}$/.test(document.revision || "")) {
    query = { doc: document.doc_did };
  } else if (document?.doc_did || document?.library_object_uri) {
    return; // An unverified result does not replace the last settled hint.
  }
  homeNavigation.setQuery(query);
}

function isCurrentSessionTarget(target) {
  return !!target && state.currentSessionId === target.sessionId;
}

function isCurrentDocumentTarget(target) {
  return isCurrentSessionTarget(target) && documentIdentity(state.current) === target.identity;
}

function scheduleAutosave() {
  clearAutosaveTimer();
  if (
    !state.current ||
    state.mode !== "shell" ||
    !state.dirty ||
    saveRecovery?.target.sessionId === state.currentSessionId ||
    pendingHomeWindowCloseTarget?.sessionId === state.currentSessionId
  ) {
    return;
  }
  autosaveTimerId = window.setTimeout(() => {
    void saveCurrent({ quiet: true, autosave: true }).catch((error) => {
      reportSaveFailure(error, "Autosave failed.");
    });
  }, 900);
}

function reportSaveFailure(error, fallback) {
  if (error?.documentsTarget && !isCurrentDocumentTarget(error.documentsTarget)) return;
  setStatus(readableDocumentsError(error, fallback), "warning");
}

function canAnnounceHomeChrome() {
  return !!state.homeToken && !!homeOrigin && window.top !== window;
}

function buildHomeMenuManifest() {
  return {
    menus: [
      {
        title: "File",
        items: [
          { label: "New Document", cmd: "file-new-document" },
          { label: "Save", cmd: "file-save" },
          { label: "Save As", cmd: "file-save-as" },
          { label: "Publish", cmd: "file-publish" },
          { label: "Delete Document", cmd: "file-delete-document" },
          { label: "Close Window", cmd: "__close-window" },
        ],
      },
      {
        title: "View",
        items: [
          { label: "Write", cmd: "view-write" },
          { label: "Split", cmd: "view-split" },
          { label: "Read", cmd: "view-read" },
          { label: "Find", cmd: "view-find" },
        ],
      },
    ],
  };
}

function syncHomeMenuManifest() {
  if (!canAnnounceHomeChrome() || !homeChromeReady) {
    return;
  }
  const manifest = buildHomeMenuManifest();
  const signature = JSON.stringify(manifest);
  if (signature === lastHomeMenuManifestSignature) {
    return;
  }
  lastHomeMenuManifestSignature = signature;
  window.top.postMessage({
    type: "home:menu-manifest",
    homeToken: state.homeToken,
    ...manifest,
  }, homeOrigin);
}

function announceHomeChrome() {
  if (!canAnnounceHomeChrome() || homeChromeReady) {
    return;
  }
  window.top.postMessage({ type: "home:app-ready", homeToken: state.homeToken }, homeOrigin);
  homeChromeReady = true;
  syncHomeMenuManifest();
}

function chooseInCapsule(options) {
  if (state.pendingConfirmation) {
    closeConfirmModal("cancel");
  }
  const title = options && Object.prototype.hasOwnProperty.call(options, "title")
    ? String(options.title || "")
    : "Continue?";
  const message = options && options.message ? options.message : "";
  const confirmLabel = options && options.confirmLabel ? options.confirmLabel : "Continue";
  const secondaryLabel = options && options.secondaryLabel ? String(options.secondaryLabel) : "";
  const danger = !!(options && options.danger);
  const hasTitle = title.length > 0;
  const dialog = elements.confirmModal.querySelector(".modal-card");
  elements.confirmTitle.textContent = title;
  elements.confirmTitle.hidden = !hasTitle;
  if (dialog) {
    if (hasTitle) {
      dialog.setAttribute("aria-labelledby", "confirm-title");
      dialog.removeAttribute("aria-label");
    } else {
      dialog.removeAttribute("aria-labelledby");
      dialog.setAttribute("aria-label", confirmLabel);
    }
  }
  elements.confirmMessage.textContent = message;
  elements.confirmAction.textContent = confirmLabel;
  elements.confirmAction.classList.toggle("action-danger-solid", danger);
  elements.confirmSecondaryAction.hidden = !secondaryLabel;
  elements.confirmSecondaryAction.textContent = secondaryLabel || "Discard";
  elements.confirmModal.classList.add("open");
  elements.confirmModal.setAttribute("aria-hidden", "false");
  const focusTarget =
    options && options.defaultFocus === "cancel"
      ? elements.confirmCancel
      : elements.confirmAction;
  window.setTimeout(() => focusTarget.focus(), 10);
  return new Promise((resolve) => {
    state.pendingConfirmation = { resolve };
  });
}

function closeContextMenu() {
  contextMenuDocDid = "";
  elements.contextMenu.classList.add("hidden");
  elements.contextMenu.setAttribute("aria-hidden", "true");
  elements.contextMenu.innerHTML = "";
}

function openContextMenu(x, y, docDid) {
  const documentItem = state.documents.find((item) => item.doc_did === docDid) || null;
  contextMenuDocDid = docDid || "";
  const actions = [];
  if (!documentItem) {
    actions.push({ action: "new", label: "New note" });
  } else {
    actions.push({ action: "duplicate", label: "Duplicate" });
    if (documentItem.latest_published_cid) {
      actions.push({ action: "unpublish", label: "Unpublish" });
      actions.push({ action: "copy-published-link", label: "Copy Published Link" });
    } else {
      actions.push({ action: "publish", label: "Publish" });
    }
    actions.push({ action: "delete", label: "Delete", danger: true });
  }
  elements.contextMenu.innerHTML = actions.map((item) =>
    '<button class="context-menu-item' + (item.danger ? ' danger' : '') + '" type="button" data-menu-action="' + item.action + '" role="menuitem">' + escapeHtml(item.label) + "</button>"
  ).join("");
  elements.contextMenu.style.left = "8px";
  elements.contextMenu.style.top = "8px";
  elements.contextMenu.classList.remove("hidden");
  elements.contextMenu.setAttribute("aria-hidden", "false");
  const menuWidth = elements.contextMenu.offsetWidth;
  const menuHeight = elements.contextMenu.offsetHeight;
  const maxLeft = Math.max(8, window.innerWidth - menuWidth - 8);
  const maxTop = Math.max(8, window.innerHeight - menuHeight - 8);
  const left = Math.min(Math.max(8, Math.round(x)), maxLeft);
  const top = Math.min(Math.max(8, Math.round(y)), maxTop);
  elements.contextMenu.style.left = left + "px";
  elements.contextMenu.style.top = top + "px";
}

function clearPreviewFindHighlights() {
  elements.preview.querySelectorAll("mark.find-highlight").forEach((mark) => {
    const parent = mark.parentNode;
    if (!parent) {
      return;
    }
    parent.replaceChild(document.createTextNode(mark.textContent || ""), mark);
    parent.normalize();
  });
  previewFindMarks = [];
}

function isEditorVisible() {
  return state.workspaceView !== "read";
}

function isPreviewVisible() {
  return state.workspaceView !== "write" && state.inspectorView === "preview";
}

function currentFindQuery() {
  return String(elements.findInput.value || "").trim();
}

function computeSourceFindMatches(query) {
  if (!query) {
    return [];
  }
  const source = String(elements.editor.value || "");
  const lowerSource = source.toLowerCase();
  const lowerQuery = query.toLowerCase();
  const matches = [];
  let offset = 0;
  while (offset <= lowerSource.length) {
    const matchIndex = lowerSource.indexOf(lowerQuery, offset);
    if (matchIndex === -1) {
      break;
    }
    matches.push({
      start: matchIndex,
      end: matchIndex + query.length,
    });
    offset = matchIndex + Math.max(1, query.length);
  }
  return matches;
}

function syncFindCount(query) {
  if (!query) {
    elements.findCount.textContent = "";
    return;
  }
  if (!findMatches.length || findIndex < 0) {
    elements.findCount.textContent = "0 / 0";
    return;
  }
  elements.findCount.textContent = String(findIndex + 1) + " / " + String(findMatches.length);
}

function selectEditorFindMatch(match) {
  if (!match || !isEditorVisible()) {
    return;
  }
  elements.editor.focus();
  elements.editor.setSelectionRange(match.start, match.end, "forward");
  const beforeMatch = String(elements.editor.value || "").slice(0, match.start);
  const lineNumber = beforeMatch.split("\n").length;
  const lineHeight = Number.parseFloat(window.getComputedStyle(elements.editor).lineHeight) || 26;
  elements.editor.scrollTop = Math.max(0, (lineNumber - 2) * lineHeight);
}

function applyPreviewFindHighlights() {
  clearPreviewFindHighlights();
  const query = currentFindQuery();
  if (!query || !isPreviewVisible()) {
    return;
  }
  const lowerQuery = query.toLowerCase();
  const walker = document.createTreeWalker(elements.preview, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (!node || !node.nodeValue || !node.nodeValue.trim()) {
        return NodeFilter.FILTER_REJECT;
      }
      if (node.parentElement && node.parentElement.closest("mark.find-highlight")) {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  const textNodes = [];
  while (walker.nextNode()) {
    textNodes.push(walker.currentNode);
  }
  textNodes.forEach((node) => {
    const text = node.nodeValue || "";
    const lowerText = text.toLowerCase();
    let searchIndex = 0;
    let matchIndex = lowerText.indexOf(lowerQuery, searchIndex);
    if (matchIndex === -1) {
      return;
    }
    const fragment = document.createDocumentFragment();
    while (matchIndex !== -1) {
      if (matchIndex > searchIndex) {
        fragment.appendChild(document.createTextNode(text.slice(searchIndex, matchIndex)));
      }
      const mark = document.createElement("mark");
      mark.className = "find-highlight";
      mark.textContent = text.slice(matchIndex, matchIndex + query.length);
      fragment.appendChild(mark);
      previewFindMarks.push(mark);
      searchIndex = matchIndex + query.length;
      matchIndex = lowerText.indexOf(lowerQuery, searchIndex);
    }
    if (searchIndex < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(searchIndex)));
    }
    node.parentNode.replaceChild(fragment, node);
  });
  previewFindMarks.forEach((match, matchNumber) => {
    match.classList.toggle("active", matchNumber === findIndex);
  });
  const activePreviewMatch = previewFindMarks[findIndex];
  if (activePreviewMatch) {
    activePreviewMatch.scrollIntoView({ block: "center", behavior: "smooth" });
  }
}

function activateFindMatch(index) {
  const query = currentFindQuery();
  if (!query) {
    findIndex = -1;
    syncFindCount(query);
    clearPreviewFindHighlights();
    return;
  }
  if (!findMatches.length) {
    findIndex = -1;
    syncFindCount(query);
    applyPreviewFindHighlights();
    return;
  }
  findIndex = ((index % findMatches.length) + findMatches.length) % findMatches.length;
  syncFindCount(query);
  selectEditorFindMatch(findMatches[findIndex]);
  applyPreviewFindHighlights();
}

function updateFindMatches(options) {
  const preserveIndex = !!(options && options.preserveIndex);
  const query = currentFindQuery();
  findMatches = computeSourceFindMatches(query);
  if (!query) {
    findIndex = -1;
    syncFindCount(query);
    clearPreviewFindHighlights();
    return;
  }
  if (!findMatches.length) {
    findIndex = -1;
    syncFindCount(query);
    applyPreviewFindHighlights();
    return;
  }
  const nextIndex = preserveIndex && findIndex >= 0
    ? Math.min(findIndex, findMatches.length - 1)
    : 0;
  activateFindMatch(nextIndex);
}

function openFindBar() {
  elements.findBar.classList.remove("hidden");
  elements.findInput.focus();
  elements.findInput.select();
  updateFindMatches();
}

function closeFindBar() {
  elements.findBar.classList.add("hidden");
  elements.findInput.value = "";
  findMatches = [];
  findIndex = -1;
  clearPreviewFindHighlights();
  elements.findCount.textContent = "";
}

function refreshPreviewFromEditor() {
  const rendered = renderMarkdownDocument(elements.editor.value || "", { interactiveTasks: true });
  elements.preview.innerHTML = rendered.html;
  renderOutline(rendered.outline);
  if (!elements.findBar.classList.contains("hidden")) {
    updateFindMatches({ preserveIndex: true });
  }
}

function syncEditorMutation() {
  if (state.current) {
    state.current.body = elements.editor.value;
  }
  refreshPreviewFromEditor();
  setDirty(true);
  scheduleAutosave();
}

function setTaskLineChecked(lineNumber, checked) {
  const lines = String(elements.editor.value || "").replace(/\r\n/g, "\n").split("\n");
  if (lineNumber < 0 || lineNumber >= lines.length) {
    return;
  }
  const updated = lines[lineNumber].replace(/^([-*]\s+\[)( |x|X)(\]\s+.*)$/, (_match, start, _mark, end) => {
    return start + (checked ? "x" : " ") + end;
  });
  if (updated === lines[lineNumber]) {
    return;
  }
  lines[lineNumber] = updated;
  elements.editor.value = lines.join("\n");
  syncEditorMutation();
}

function toggleTaskAtCursor() {
  const source = String(elements.editor.value || "");
  const selectionStart = elements.editor.selectionStart || 0;
  const selectionEnd = elements.editor.selectionEnd || selectionStart;
  const lineStart = source.lastIndexOf("\n", Math.max(0, selectionStart - 1)) + 1;
  const lineEndIndex = source.indexOf("\n", selectionEnd);
  const lineEnd = lineEndIndex === -1 ? source.length : lineEndIndex;
  const line = source.slice(lineStart, lineEnd);
  const taskLine = /^([-*]\s+\[)( |x|X)(\]\s+.*)$/.exec(line);
  let nextLine = line;
  let nextSelectionStart = selectionStart;
  let nextSelectionEnd = selectionEnd;
  if (taskLine) {
    nextLine = taskLine[1] + (String(taskLine[2]).toLowerCase() === "x" ? " " : "x") + taskLine[3];
  } else {
    const prefix = "- [ ] ";
    nextLine = prefix + line;
    nextSelectionStart += prefix.length;
    nextSelectionEnd += prefix.length;
  }
  if (nextLine === line) {
    return;
  }
  elements.editor.value = source.slice(0, lineStart) + nextLine + source.slice(lineEnd);
  elements.editor.focus();
  elements.editor.setSelectionRange(nextSelectionStart, nextSelectionEnd, "forward");
  syncEditorMutation();
}

function sortDocuments() {
  state.documents.sort((left, right) => {
    return (right.updated_at || 0) - (left.updated_at || 0) || left.title.localeCompare(right.title);
  });
}

function setWorkspaceView(view) {
  state.workspaceView = preferredWorkspaceView(view);
  if (state.workspaceView === "read") {
    setInspectorView("preview");
  }
  elements.documentsShell.dataset.workspaceView = state.workspaceView;
  elements.modeWrite.classList.toggle("active", state.workspaceView === "write");
  elements.modeSplit.classList.toggle("active", state.workspaceView === "split");
  elements.modeRead.classList.toggle("active", state.workspaceView === "read");
  elements.modeWrite.setAttribute("aria-pressed", String(state.workspaceView === "write"));
  elements.modeSplit.setAttribute("aria-pressed", String(state.workspaceView === "split"));
  elements.modeRead.setAttribute("aria-pressed", String(state.workspaceView === "read"));
  if (!elements.findBar.classList.contains("hidden")) {
    updateFindMatches({ preserveIndex: true });
  }
}

function setSidebarCollapsed(collapsed) {
  state.sidebarCollapsed = !!collapsed;
  elements.documentsShell.dataset.sidebarCollapsed = state.sidebarCollapsed ? "true" : "false";
  const label = state.sidebarCollapsed ? "Show list" : "Hide list";
  elements.sidebarToggle.setAttribute("aria-label", label);
  elements.sidebarToggle.title = label;
  elements.sidebarToggle.setAttribute("aria-expanded", String(!state.sidebarCollapsed));
}

function setInspectorView(view) {
  state.inspectorView = view === "outline" ? "outline" : view === "history" ? "history" : "preview";
  elements.previewPanel.classList.toggle("hidden", state.inspectorView !== "preview");
  elements.outlinePanel.classList.toggle("hidden", state.inspectorView !== "outline");
  elements.historyPanel.classList.toggle("hidden", state.inspectorView !== "history");
  elements.panePreview.classList.toggle("active", state.inspectorView === "preview");
  elements.paneOutline.classList.toggle("active", state.inspectorView === "outline");
  elements.paneHistory.classList.toggle("active", state.inspectorView === "history");
  if (!elements.findBar.classList.contains("hidden")) {
    updateFindMatches({ preserveIndex: true });
  }
}

function setStatus(message, type) {
  clearStatusTimer();
  const text = type === "warning"
    ? readableDocumentsError(message, "Documents action could not be completed.")
    : String(message || "");
  elements.statusText.textContent = text;
  elements.statusText.className = "status-text" + (type === "warning" ? " warning" : "");
  elements.statusRow.classList.toggle("hidden", !text);
}

function setDirty(dirty) {
  state.dirty = !!dirty;
  const hasCurrent = !!state.current;
  const hasPersistedCurrent = hasCurrent && !isDraftDocument(state.current);
  const isLibraryFile = isLibraryFileProjection(state.current);
  elements.saveButton.disabled = !hasCurrent || !state.dirty;
  elements.saveAsButton.disabled = !hasPersistedCurrent || isLibraryFile;
  elements.publishButton.disabled = !hasCurrent || isLibraryFile;
  elements.unpublishButton.disabled = !hasPersistedCurrent || isLibraryFile || !state.current.latest_published_cid;
  elements.deleteButton.disabled = !hasPersistedCurrent || isLibraryFile;
  if (hasCurrent && state.dirty) {
    setStatus("Unsaved local changes.", "warning");
  } else if (hasCurrent) {
    clearStatus();
  }
  renderDocumentsList();
}

function setPublishedUri(uri) {
  const cleanUri = typeof uri === "string" ? uri.trim() : "";
  if (!cleanUri) {
    elements.copyPublishedLink.classList.add("hidden");
    elements.copyPublishedLink.disabled = true;
    delete elements.copyPublishedLink.dataset.copyUri;
    return;
  }
  elements.copyPublishedLink.classList.remove("hidden");
  elements.copyPublishedLink.disabled = false;
  elements.copyPublishedLink.dataset.copyUri = cleanUri;
}

async function copyTextToClipboard(text) {
  await homeClipboard.writeText(text, { purpose: "resource.uri" });
}

async function copyPublishedUri(uri) {
  const cleanUri = typeof uri === "string" ? uri.trim() : "";
  if (!cleanUri.startsWith("elastos://")) {
    return;
  }
  await copyTextToClipboard(cleanUri);
  setStatus("Copied link.");
  scheduleStatusClear();
}

function renderHistory() {
  elements.historyPanel.innerHTML = "";
  const revisions = state.current && Array.isArray(state.current.publish_history)
    ? [...state.current.publish_history].reverse()
    : [];
  if (!revisions.length) {
    elements.historyPanel.innerHTML = '<div class="sidebar-note">No published revisions yet.</div>';
    return;
  }
  revisions.forEach((revision, index) => {
    const item = window.document.createElement("div");
    item.className = "history-item";
    const uri = "elastos://" + revision.cid;
    item.innerHTML =
      '<span class="history-kicker">Revision ' + escapeHtml(String(revisions.length - index)) + "</span>" +
      '<span class="history-title">' + escapeHtml(shortCid(revision.cid)) + "</span>" +
      '<span class="history-meta">' + escapeHtml(formatTime(revision.published_at)) + "</span>" +
      '<button class="history-link" type="button" data-copy-uri="' + escapeHtml(uri) + '">Copy link</button>';
    item.querySelector(".history-link").addEventListener("click", async (event) => {
      try {
        await copyPublishedUri(event.currentTarget.dataset.copyUri || "");
      } catch (error) {
        console.error("documents copy history link failed", error);
        setStatus(readableDocumentsError(error, "Copy failed."), "warning");
      }
    });
    elements.historyPanel.appendChild(item);
  });
}

function renderOutline(outline) {
  elements.outlinePanel.innerHTML = "";
  if (!outline.length) {
    elements.outlinePanel.innerHTML = '<div class="sidebar-note">No headings yet. Add `#` markdown headings to build an outline.</div>';
    return;
  }
  const list = window.document.createElement("div");
  list.className = "outline-list";
  outline.forEach((item) => {
    const wrapper = window.document.createElement("div");
    wrapper.className = "outline-item";
    wrapper.style.marginLeft = Math.max(0, item.depth - 1) * 12 + "px";
    wrapper.innerHTML =
      '<a class="outline-link" href="#' + escapeHtml(item.id) + '">' +
        '<span class="outline-kicker">H' + escapeHtml(String(item.depth)) + "</span>" +
        '<span class="outline-title">' + escapeHtml(item.title) + "</span>" +
      "</a>";
    wrapper.querySelector("a").addEventListener("click", (event) => {
      event.preventDefault();
      const heading = elements.preview.querySelector("#" + CSS.escape(item.id));
      if (heading) {
        heading.scrollIntoView({ block: "start", behavior: "smooth" });
      }
      setInspectorView("preview");
    });
    list.appendChild(wrapper);
  });
  elements.outlinePanel.appendChild(list);
}

function renderCurrentDocument() {
  if (!state.current) {
    clearAutosaveTimer();
    elements.titleInput.value = "";
    elements.titleInput.disabled = true;
    elements.editor.value = "";
    elements.editor.disabled = true;
    elements.preview.innerHTML = '<div class="empty-state">Create or open a document.</div>';
    setPublishedUri("");
    renderHistory();
    renderOutline([]);
    setStatus("No document selected.");
    setDirty(false);
    return;
  }

  elements.titleInput.disabled = isLibraryFileProjection(state.current);
  elements.editor.disabled = false;
  elements.titleInput.value = state.current.title;
  elements.editor.value = state.current.body;
  refreshPreviewFromEditor();

  if (state.current.latest_published_cid) {
    setPublishedUri("elastos://" + state.current.latest_published_cid);
  } else {
    setPublishedUri("");
  }

  renderHistory();
  clearStatus();
  setDirty(false);
}

function renderDocumentsList() {
  elements.documentsList.innerHTML = "";
  const query = state.filterQuery.trim().toLowerCase();
  const visibleDocuments = state.documents.filter((item) => {
    if (!query) {
      return true;
    }
    return item.title.toLowerCase().includes(query) || item.file_name.toLowerCase().includes(query);
  });
  if (!visibleDocuments.length) {
    elements.documentsList.innerHTML = '<div class="sidebar-note">' + (query ? "No documents match this search." : "No documents yet.") + "</div>";
    return;
  }

  visibleDocuments.forEach((item) => {
    const button = window.document.createElement("button");
    button.type = "button";
    const classes = ["document-list-item"];
    if (item.latest_published_cid) {
      classes.push("published");
    }
    if (state.current && state.current.doc_did === item.doc_did) {
      classes.push("active");
    }
    button.className = classes.join(" ");
    button.dataset.docDid = item.doc_did;
    const dirtyMarker = state.current && state.current.doc_did === item.doc_did && state.dirty ? " • unsaved" : "";
    const publishedIcon = item.latest_published_cid
      ? '<span class="document-list-published-icon" aria-label="Published" title="Published"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v10"></path><path d="m7 9 5-5 5 5"></path><path d="M5 16v4h14v-4"></path></svg></span>'
      : "";
    button.innerHTML =
      '<span class="document-list-title-row"><span class="document-list-title">' + escapeHtml(item.title) + "</span>" + publishedIcon + "</span>" +
      '<span class="document-list-meta">' + escapeHtml(item.file_name + dirtyMarker) + "</span>" +
      '<span class="document-list-row"><span class="document-list-updated">' + escapeHtml(formatTime(item.updated_at)) + '</span></span>';
    button.addEventListener("click", () => selectDocument(item.doc_did));
    elements.documentsList.appendChild(button);
  });
}

async function documentsApi(path, options) {
  const request = { method: "GET", ...(options || {}) };
  request.headers = {
    ...(request.headers || {}),
    "x-elastos-home-token": state.homeToken,
  };
  if (request.body && !request.headers["content-type"]) {
    request.headers["content-type"] = "application/json";
  }
  const response = await fetch(path, request);
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || "Documents request failed");
  }
  if (response.status === 204) {
    return null;
  }
  const contentType = response.headers.get("content-type") || "";
  if (contentType.includes("application/json")) {
    return response.json();
  }
  return response.text();
}

async function documentsProviderApi(op, payload) {
  const response = await documentsApi("/api/provider/documents/" + encodeURIComponent(op), {
    method: "POST",
    body: payload ? JSON.stringify(payload) : "",
  });
  if (response && response.status === "error") {
    throw new Error(response.message || "Documents request failed");
  }
  return response && typeof response === "object" && "data" in response ? response.data : response;
}

async function libraryObjectApi(op, payload) {
  const uri = payload && payload.uri ? String(payload.uri) : "";
  const endpoint = "/api/viewers/documents/library-object?uri=" + encodeURIComponent(uri);
  const response = op === "write"
    ? await documentsApi(endpoint, {
      method: "PUT",
      body: JSON.stringify({
        data: payload.data || "",
        mime: payload.mime || null,
        if_revision: payload.if_revision || null,
      }),
    })
    : await documentsApi(endpoint, { method: "GET" });
  if (response && response.status === "error") {
    throw new Error(response.message || "Library request failed");
  }
  return response && typeof response === "object" && "data" in response ? response.data : response;
}

function base64ToUtf8(data) {
  const binary = atob(data || "");
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new TextDecoder().decode(bytes);
}

function utf8ToBase64(text) {
  const bytes = new TextEncoder().encode(String(text || ""));
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function upsertDocumentListItem(document) {
  const listItem = {
    doc_did: document.doc_did,
    title: document.title,
    file_name: document.file_name,
    working_copy_uri: document.working_copy_uri,
    updated_at: document.updated_at,
    latest_published_cid: document.latest_published_cid || null,
  };
  const index = state.documents.findIndex((item) => item.doc_did === listItem.doc_did);
  if (index === -1) {
    state.documents.unshift(listItem);
  } else {
    state.documents[index] = listItem;
  }
  sortDocuments();
}

function startDraftDocument(options) {
  replaceCurrentDocument(createDraftDocument());
  renderDocumentsList();
  renderCurrentDocument();
  clearStatus();
  if (options && options.focusTitle) {
    window.setTimeout(() => elements.titleInput.focus(), 10);
  } else if (options && options.focusEditor) {
    window.setTimeout(() => elements.editor.focus(), 10);
  }
}

async function loadSummary() {
  const summary = await documentsProviderApi("summary");
  state.documents = summary.documents || [];
  sortDocuments();
  renderDocumentsList();
  if (state.initialIntent === "new") {
    state.initialIntent = "";
    await createDocument("");
    return;
  }
  if (state.initialLibraryObjectUri) {
    const requestedUri = state.initialLibraryObjectUri.trim();
    state.initialLibraryObjectUri = "";
    await loadLibraryObject(requestedUri);
    setWorkspaceView(preferredWorkspaceView(state.initialView));
    return;
  }
  if (state.initialCid) {
    const requestedCid = state.initialCid.trim();
    state.initialCid = "";
    const requested = state.documents.find((item) => item.latest_published_cid === requestedCid);
    if (requested) {
      await selectDocument(requested.doc_did);
      setWorkspaceView(preferredWorkspaceView(state.initialView));
    } else {
      elements.documentsShell.classList.add("hidden");
      await loadPublishedRevisionMode(requestedCid);
    }
    return;
  }
  if (state.initialDocDid) {
    const requested = state.documents.find((item) => item.doc_did === state.initialDocDid);
    if (requested) {
      await selectDocument(requested.doc_did);
    } else {
      const requestedUri = documentUri(state.initialDocDid);
      replaceCurrentDocument(null);
      renderDocumentsList();
      renderCurrentDocument();
      elements.preview.innerHTML = '<div class="error-state">Document not found. Nothing opened.</div>';
      setStatus("Document not found: " + requestedUri + ". Nothing opened.", "warning");
    }
    state.initialDocDid = "";
    return;
  }
  if (state.current) {
    renderDocumentsList();
    renderCurrentDocument();
    return;
  }
  startDraftDocument({ focusTitle: true });
}

async function loadLibraryObject(uri) {
  const sequence = ++documentSelectionSequence;
  const sessionId = state.currentSessionId;
  const response = await libraryObjectApi("read", { uri });
  if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId) return;
  const object = response.object || {};
  const name = object.name || baseName(object.uri || uri);
  replaceCurrentDocument({
    doc_did: "",
    document_uri: object.uri || uri,
    library_object_uri: object.uri || uri,
    revision: object.revision || "",
    title: name,
    file_name: name,
    working_copy_uri: object.uri || uri,
    body: base64ToUtf8(response.data || ""),
    mime: object.mime || "text/markdown",
    created_at: object.created_at || 0,
    updated_at: object.modified_at || 0,
    latest_published_cid: null,
    publish_history: [],
  });
  renderDocumentsList();
  renderCurrentDocument();
  setStatus("Opened from Library.");
  scheduleStatusClear();
}

async function openChatAttachment(payload) {
  const attachmentId = String(payload && payload.attachmentId ? payload.attachmentId : "").trim();
  if (attachmentId && state.pendingChatAttachmentId === attachmentId) {
    return false;
  }
  state.pendingChatAttachmentId = attachmentId;
  try {
    if (state.current && state.dirty) {
      const confirmed = await confirmInCapsule({
        title: "Discard changes?",
        message: "Open this Chat attachment without saving the current draft?",
        confirmLabel: "Discard",
      });
      if (!confirmed) {
        throw new Error("Opening cancelled in Documents.");
      }
    }
    resetPendingSaveIntent();
    const mime = String(payload && payload.mimeType ? payload.mimeType : "application/octet-stream");
    if (!isTextMimeType(mime)) {
      throw new Error("Documents can open text attachments only.");
    }
    const fileName = String(payload && payload.fileName ? payload.fileName : "Chat attachment").trim() || "Chat attachment";
    const body = dataUrlToUtf8(payload && payload.dataUrl);
    const response = await documentsProviderApi("import_chat_attachment", {
      attachment_id: attachmentId,
      file_name: fileName,
      mime,
      body,
    });
    const document = response.document;
    if (!document || !document.doc_did) {
      throw new Error("Documents could not import this file.");
    }
    if (state.current && state.current.doc_did === document.doc_did && !state.dirty) {
      return true;
    }
    state.mode = "shell";
    elements.shareShell.classList.add("hidden");
    elements.documentsShell.classList.remove("hidden");
    replaceCurrentDocument(document);
    upsertDocumentListItem(document);
    renderDocumentsList();
    renderCurrentDocument();
    setWorkspaceView(preferredWorkspaceView(state.initialView));
    setStatus("Opened the Chat attachment.");
    scheduleStatusClear();
    return true;
  } finally {
    state.pendingChatAttachmentId = "";
  }
}

function sendChatAttachmentOpenResult({ attachmentId, ok, message }) {
  const homeOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";
  if (!state.homeToken || !homeOrigin || window.top === window) {
    return;
  }
  const payload = {
    type: "chat-room:attachment-open-result",
    attachmentId: String(attachmentId || ""),
    ok: ok === true,
    message: String(message || ""),
  };
  window.top.postMessage({
    type: "home:deliver-to-target",
    target: "chat-room",
    homeToken: state.homeToken,
    payload,
  }, homeOrigin);
}

async function selectDocument(docDid, options) {
  const sequence = ++documentSelectionSequence;
  const sessionId = state.currentSessionId;
  const title = elements.titleInput.value;
  const body = elements.editor.value;
  const force = !!(options && options.force);
  if (state.current && state.current.doc_did !== docDid && state.dirty) {
    const confirmed = await confirmInCapsule({
      title: "Discard changes?",
      message: "Open another document without saving this draft?",
      confirmLabel: "Discard",
    });
    if (!confirmed || sequence !== documentSelectionSequence || sessionId !== state.currentSessionId) {
      return;
    }
  }
  if (!force && state.current && state.current.doc_did === docDid && !state.dirty) {
    return;
  }
  resetPendingSaveIntent();
  const response = await documentsProviderApi("get", { doc_did: docDid }).catch((error) => {
    if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId) return null;
    throw error;
  });
  if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId ||
      title !== elements.titleInput.value || body !== elements.editor.value) return;
  requireSavedWorkingCopy(response.document, docDid);
  replaceCurrentDocument(response.document);
  upsertDocumentListItem(response.document);
  renderDocumentsList();
  renderCurrentDocument();
}

async function createDocument(title) {
  if (state.current && state.dirty) {
    const confirmed = await confirmInCapsule({
      title: "Discard changes?",
      message: "Create a new document without saving this draft?",
      confirmLabel: "Discard",
    });
    if (!confirmed) {
      return;
    }
  }
  startDraftDocument({ focusTitle: !title, focusEditor: !!title });
  if (title) {
    elements.titleInput.value = title;
    setDirty(true);
  }
  clearStatus();
}

function applySavedDocumentState(persistedDocument, saveTarget, requestedTitle, requestedBody, successMessage, options) {
  const quiet = !!(options && options.quiet);
  if (!isCurrentDocumentTarget(saveTarget)) return;
  const libraryProjection = isLibraryFileProjection(persistedDocument);
  if (!libraryProjection) {
    upsertDocumentListItem(persistedDocument);
  }
  renderDocumentsList();
  if (!isCurrentDocumentTarget(saveTarget)) {
    return;
  }
  savedWorkingCopy = { ...persistedDocument };
  saveRecovery = null;
  const changedDuringSave =
    elements.titleInput.value !== requestedTitle ||
    elements.editor.value !== requestedBody;
  state.current = {
    ...persistedDocument,
    title: changedDuringSave ? elements.titleInput.value : (persistedDocument.title || requestedTitle),
    body: changedDuringSave ? elements.editor.value : requestedBody,
    file_name: persistedDocument.file_name || state.current?.file_name || "",
  };
  updateDocumentsNavigation(persistedDocument);
  if (changedDuringSave) {
    refreshPreviewFromEditor();
    renderHistory();
    setDirty(true);
    scheduleAutosave();
    return;
  }
  renderCurrentDocument();
  if (quiet) {
    clearStatus();
    return;
  }
  setStatus(successMessage);
  scheduleStatusClear();
}

function requireSavedWorkingCopy(document, expectedId) {
  if (!document || typeof document.doc_did !== "string" || !document.doc_did ||
      (expectedId && document.doc_did !== expectedId) ||
      !/^[0-9a-f]{64}$/.test(document.revision || "") ||
      typeof document.title !== "string" || typeof document.body !== "string") {
    throw new Error("Save result could not be confirmed. Your edits are kept.");
  }
  return document;
}

async function saveCurrent(options) {
  if (!state.current) return;
  const saveTarget = {
    sessionId: state.currentSessionId,
    identity: documentIdentity(state.current),
  };
  if (saveInFlight) {
    autosaveQueued = true;
    queuedSaveTarget = saveTarget;
    return saveInFlight;
  }
  if (options?.autosave && saveRecovery?.target.sessionId === saveTarget.sessionId) {
    throw new Error("Automatic saving is paused. Choose Save to check the stored document.");
  }
  let saveDocument = { ...state.current };
  const baseline = { ...(savedWorkingCopy || saveDocument) };
  const quiet = !!options?.quiet;
  clearAutosaveTimer();
  const requestedTitle = elements.titleInput.value;
  const requestedBody = elements.editor.value;
  if (!quiet) {
    setStatus("Saving…");
  }
  saveInFlight = (async () => {
    if (isLibraryFileProjection(saveDocument)) {
      const response = await libraryObjectApi("write", {
        uri: saveDocument.library_object_uri,
        data: utf8ToBase64(requestedBody),
        mime: saveDocument.mime || "text/markdown",
        if_revision: saveDocument.revision || null,
      });
      const object = response.object || {};
      applySavedDocumentState({
        ...saveDocument,
        document_uri: object.uri || saveDocument.document_uri,
        library_object_uri: object.uri || saveDocument.library_object_uri,
        revision: object.revision || saveDocument.revision,
        title: object.name || requestedTitle || saveDocument.title,
        file_name: object.name || saveDocument.file_name,
        body: requestedBody,
        mime: object.mime || saveDocument.mime,
        updated_at: object.modified_at || saveDocument.updated_at,
      }, saveTarget, requestedTitle, requestedBody, "Saved to Library.", options);
      return;
    }
    if (saveRecovery && isCurrentDocumentTarget(saveRecovery.target)) {
      const recovery = saveRecovery;
      if (recovery.phase === "create") {
        const choice = await chooseInCapsule({
          title: "Save result unknown",
          message: "A document may already exist. Check Documents before saving again. Creating a copy may make a duplicate.",
          confirmLabel: "Check Documents",
          secondaryLabel: "Create copy",
          defaultFocus: "cancel",
        });
        if (!isCurrentDocumentTarget(saveTarget)) return;
        if (choice === "confirm") {
          const response = await documentsProviderApi("summary", {});
          if (!isCurrentDocumentTarget(saveTarget)) return;
          state.documents = response.documents;
          renderDocumentsList();
          throw new Error("Your draft is kept. Select the stored document to check it, or choose Save to create a copy.");
        }
        if (choice !== "secondary") throw new Error("Your draft is kept. Saving remains paused.");
        saveRecovery = null;
      } else {
        const response = await documentsProviderApi("get", { doc_did: recovery.base.doc_did });
        if (!isCurrentDocumentTarget(saveTarget)) return;
        const observed = requireSavedWorkingCopy(response.document, recovery.base.doc_did);
        if (observed.title === recovery.title && observed.body === recovery.body) {
          applySavedDocumentState(observed, saveTarget, recovery.inputTitle, recovery.body, "Saved document confirmed.", options);
          return;
        }
        if (observed.revision !== recovery.base.revision ||
            observed.title !== recovery.base.title || observed.body !== recovery.base.body) {
          throw new Error("The stored document changed. Your edits are kept. Use Save As for a copy, or open the stored document.");
        }
        saveDocument = observed;
        saveRecovery = null;
      }
    }
    if (!isCurrentDocumentTarget(saveTarget)) return;
    if (isDraftDocument(saveDocument)) {
      saveRecovery = { target: { ...saveTarget }, phase: "create" };
      const created = await documentsProviderApi("create", { title: requestedTitle || null });
      saveDocument = requireSavedWorkingCopy(created.document);
      if (!isCurrentDocumentTarget(saveTarget)) return;
      // Retain the observed identity before a follow-up write can lose its reply.
      state.current = { ...saveDocument, title: elements.titleInput.value, body: elements.editor.value };
      savedWorkingCopy = { ...saveDocument };
      saveTarget.identity = documentIdentity(state.current);
      updateDocumentsNavigation(saveDocument);
    }
    requireSavedWorkingCopy(saveDocument);
    saveRecovery = {
      target: { ...saveTarget }, phase: "save",
      base: isDraftDocument(baseline) ? { ...saveDocument } : baseline,
      title: requestedTitle.trim() || "Untitled", inputTitle: requestedTitle, body: requestedBody,
    };
    const response = await documentsProviderApi("save", {
      doc_did: saveDocument.doc_did,
      title: requestedTitle,
      body: requestedBody,
      if_revision: saveDocument.revision,
    });
    const persisted = requireSavedWorkingCopy(response.document, saveDocument.doc_did);
    if (persisted.title !== (requestedTitle.trim() || "Untitled") || persisted.body !== requestedBody) {
      throw new Error("Save result could not be confirmed. Your edits are kept.");
    }
    applySavedDocumentState(persisted, saveTarget, requestedTitle, requestedBody, "Saved.", options);
  })();
  try {
    await saveInFlight;
  } catch (error) {
    error.documentsTarget = { ...saveTarget };
    throw error;
  } finally {
    saveInFlight = null;
    if (autosaveQueued) {
      autosaveQueued = false;
      const queued = queuedSaveTarget;
      queuedSaveTarget = null;
      if (
        isCurrentDocumentTarget(queued) && state.dirty &&
        saveRecovery?.target.sessionId !== state.currentSessionId &&
        pendingHomeWindowCloseTarget?.sessionId !== state.currentSessionId
      ) {
        void saveCurrent({ quiet: true, autosave: true }).catch((error) => {
          reportSaveFailure(error, "Autosave failed.");
        });
      }
    }
  }
}

function openSaveAsModal() {
  if (!state.current || isDraftDocument(state.current) || isLibraryFileProjection(state.current)) {
    return;
  }
  elements.saveAsTitleInput.value = elements.titleInput.value || state.current.title || "";
  elements.saveAsFileInput.value = state.current.file_name || "";
  elements.saveAsModal.classList.add("open");
  elements.saveAsModal.setAttribute("aria-hidden", "false");
  window.setTimeout(() => elements.saveAsTitleInput.focus(), 10);
}

function closeSaveAsModal() {
  elements.saveAsModal.classList.remove("open");
  elements.saveAsModal.setAttribute("aria-hidden", "true");
}

function confirmInCapsule(options) {
  return chooseInCapsule(options).then((result) => result === "confirm");
}

function closeConfirmModal(result) {
  const pending = state.pendingConfirmation;
  state.pendingConfirmation = null;
  elements.confirmModal.classList.remove("open");
  elements.confirmModal.setAttribute("aria-hidden", "true");
  elements.confirmAction.classList.remove("action-danger-solid");
  elements.confirmSecondaryAction.hidden = true;
  if (pending) {
    pending.resolve(result || "cancel");
  }
}

function sameHomeWindowCloseTarget(target) {
  return !!target && target.sessionId === state.currentSessionId;
}

function suspendAutosaveForHomeWindowClose(target) {
  if (!sameHomeWindowCloseTarget(target)) {
    return null;
  }
  const suspension = {
    sessionId: target.sessionId,
    queued: autosaveQueued,
  };
  pendingHomeWindowCloseTarget = { sessionId: target.sessionId };
  clearAutosaveTimer();
  autosaveQueued = false;
  return suspension;
}

function resumeAutosaveForHomeWindowClose(suspension) {
  if (
    suspension &&
    pendingHomeWindowCloseTarget?.sessionId === suspension.sessionId
  ) {
    pendingHomeWindowCloseTarget = null;
  }
  if (!suspension || !sameHomeWindowCloseTarget(suspension)) {
    return;
  }
  autosaveQueued = autosaveQueued || suspension.queued;
  if (!state.dirty) {
    return;
  }
  if (!saveInFlight) {
    scheduleAutosave();
  }
}

async function requestHomeWindowCloseDecision(closeTarget) {
  const autosaveSuspension = suspendAutosaveForHomeWindowClose(closeTarget);
  if (saveInFlight) {
    try {
      await saveInFlight;
    } catch (error) {
      reportSaveFailure(error, "Save failed.");
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: false, reason: "save_failed" };
    }
    if (!sameHomeWindowCloseTarget(closeTarget)) {
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: false, reason: "stale_request" };
    }
    if (!state.dirty) {
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: true, reason: "" };
    }
  }
  if (!sameHomeWindowCloseTarget(closeTarget)) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "stale_request" };
  }
  if (!state.dirty) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: true, reason: "" };
  }
  if (!sameHomeWindowCloseTarget(closeTarget)) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "stale_request" };
  }
  const decision = await chooseInCapsule({
    title: "Save changes before closing?",
    message: "Save this document, discard your edits, or keep the window open.",
    confirmLabel: "Save",
    secondaryLabel: "Discard",
    defaultFocus: "cancel",
  });
  if (!sameHomeWindowCloseTarget(closeTarget)) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "stale_request" };
  }
  if (decision === "secondary") {
    return { ok: true, reason: "discarded" };
  }
  if (decision !== "confirm") {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "cancelled" };
  }
  try {
    await saveCurrent();
    if (!sameHomeWindowCloseTarget(closeTarget)) {
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: false, reason: "stale_request" };
    }
    if (state.dirty) {
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: false, reason: "save_incomplete" };
    }
    return { ok: true, reason: "" };
  } catch (error) {
    reportSaveFailure(error, "Save failed.");
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "save_failed" };
  }
}

function postHomeWindowCloseResult(requestId, ok, phase, reason, closeTarget) {
  if (!state.homeToken || window.parent === window) {
    return;
  }
  window.parent.postMessage(
    {
      type: DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE,
      requestId,
      homeToken: state.homeToken,
      state: phase,
      ok: ok === true,
      reason: typeof reason === "string" ? reason : "",
      sessionId: closeTarget?.sessionId ?? state.currentSessionId,
    },
    "*",
  );
}

async function handleHomeWindowCloseRequest(event) {
  const data = event.data;
  if (
    event.origin !== "null" ||
    event.source !== window.parent ||
    !hasExactKeys(data, ["type", "requestId", "homeToken"]) ||
    data.type !== DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE ||
    typeof data.requestId !== "string" ||
    !data.requestId ||
    data.homeToken !== state.homeToken
  ) {
    return false;
  }
  const closeTarget = { sessionId: state.currentSessionId };
  postHomeWindowCloseResult(
    data.requestId,
    false,
    "pending",
    "awaiting_decision",
    closeTarget,
  );
  const outcome = await requestHomeWindowCloseDecision(closeTarget);
  postHomeWindowCloseResult(
    data.requestId,
    outcome.ok,
    "terminal",
    outcome.reason,
    closeTarget,
  );
  return true;
}

async function saveAsCurrent(title, fileName) {
  if (!state.current || isDraftDocument(state.current) || isLibraryFileProjection(state.current)) {
    return;
  }
  const target = { sessionId: state.currentSessionId, identity: documentIdentity(state.current) };
  const requestedTitle = elements.titleInput.value;
  const requestedBody = elements.editor.value;
  setStatus("Creating new document…");
  const response = await documentsProviderApi("save_as", {
    doc_did: state.current.doc_did,
    title: title || null,
    file_name: fileName || null,
    body: requestedBody,
  });
  if (!isCurrentDocumentTarget(target)) return;
  requireSavedWorkingCopy(response.document);
  if (elements.titleInput.value !== requestedTitle || elements.editor.value !== requestedBody) {
    upsertDocumentListItem(response.document);
    renderDocumentsList();
    setStatus("Copy saved. Newer edits are kept in this document.", "warning");
    return;
  }
  replaceCurrentDocument(response.document);
  upsertDocumentListItem(response.document);
  renderDocumentsList();
  renderCurrentDocument();
  setStatus("New document created.");
  scheduleStatusClear();
}

async function publishCurrent() {
  if (!state.current) {
    return;
  }
  if (isLibraryFileProjection(state.current)) {
    setStatus("Publish Library files from Library.", "warning");
    return;
  }
  try {
    if (state.dirty || isDraftDocument(state.current)) {
      await saveCurrent();
    }
    if (isDraftDocument(state.current)) {
      throw new Error("Save the document before publishing.");
    }
    setStatus("Publishing…");
    const response = await documentsProviderApi("publish", {
      doc_did: state.current.doc_did,
    });
    await selectDocument(state.current.doc_did, { force: true });
    setPublishedUri(response.uri);
    setStatus("Published.");
    scheduleStatusClear();
  } catch (error) {
    console.error("documents publish failed", error);
    setStatus(readableDocumentsError(error, "Publish failed."), "warning");
  }
}

async function unpublishCurrent() {
  if (
    !state.current ||
    isDraftDocument(state.current) ||
    isLibraryFileProjection(state.current) ||
    !state.current.latest_published_cid
  ) {
    return;
  }
  try {
    setStatus("Unpublishing…");
    await documentsProviderApi("unpublish", {
      doc_did: state.current.doc_did,
    });
    await selectDocument(state.current.doc_did, { force: true });
    setStatus("Unpublished.");
    scheduleStatusClear();
  } catch (error) {
    console.error("documents unpublish failed", error);
    setStatus(readableDocumentsError(error, "Unpublish failed."), "warning");
  }
}

async function requestDeleteCurrent() {
  if (!state.current || isDraftDocument(state.current) || isLibraryFileProjection(state.current)) {
    return;
  }
  const deletedDocDid = state.current.doc_did;
  const title = state.current.title || "this document";
  const confirmed = await confirmInCapsule({
    title: "",
    message: 'Delete "' + title + '" from this device?',
    confirmLabel: "Delete",
    danger: true,
  });
  if (!confirmed || !state.current || state.current.doc_did !== deletedDocDid) {
    return;
  }
  try {
    setStatus("Deleting…");
    await documentsProviderApi("delete", { doc_did: deletedDocDid });
    state.documents = state.documents.filter((document) => document.doc_did !== deletedDocDid);
    if (state.current && state.current.doc_did === deletedDocDid) {
      replaceCurrentDocument(createDraftDocument());
    }
    renderDocumentsList();
    renderCurrentDocument();
    setStatus("Deleted.");
    scheduleStatusClear();
  } catch (error) {
    console.error("documents delete failed", error);
    setStatus(readableDocumentsError(error, "Document could not be deleted."), "warning");
  }
}

function suggestedDuplicateFileName(document) {
  const fileName = String(document?.file_name || "").trim();
  if (!fileName) {
    return "";
  }
  const extMatch = /(\.[^.]+)$/.exec(fileName);
  if (!extMatch) {
    return fileName + " copy";
  }
  const ext = extMatch[1];
  return fileName.slice(0, -ext.length) + " copy" + ext;
}

async function ensureContextDocumentSelected() {
  if (!contextMenuDocDid) {
    return null;
  }
  if (!state.current || state.current.doc_did !== contextMenuDocDid) {
    await selectDocument(contextMenuDocDid);
  }
  return state.current;
}

async function runContextMenuAction(action) {
  closeContextMenu();
  if (action === "new") {
    await createDocument("");
    return;
  }
  const current = await ensureContextDocumentSelected();
  if (!current) {
    return;
  }
  if (action === "duplicate") {
    await saveAsCurrent((current.title || "Untitled document") + " copy", suggestedDuplicateFileName(current));
    return;
  }
  if (action === "publish") {
    await publishCurrent();
    return;
  }
  if (action === "unpublish") {
    await unpublishCurrent();
    return;
  }
  if (action === "copy-published-link" && current.latest_published_cid) {
    await copyPublishedUri("elastos://" + current.latest_published_cid);
    return;
  }
  if (action === "delete") {
    await requestDeleteCurrent();
  }
}

function handleHomeMenuCommand(command) {
  switch (command) {
    case "file-new-document":
      void createDocument("");
      break;
    case "file-save":
      void saveCurrent().catch((error) => {
        reportSaveFailure(error, "Save failed.");
      });
      break;
    case "file-save-as":
      openSaveAsModal();
      break;
    case "file-publish":
      void publishCurrent();
      break;
    case "file-delete-document":
      void requestDeleteCurrent();
      break;
    case "view-write":
      setWorkspaceView("write");
      break;
    case "view-split":
      setWorkspaceView("split");
      break;
    case "view-read":
      setWorkspaceView("read");
      break;
    case "view-find":
      openFindBar();
      break;
    default:
      break;
  }
}

function readableDocumentsError(error, defaultMessage) {
  const message = String(error && error.message ? error.message : error || "").trim();
  if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
    return defaultMessage;
  }
  return message;
}

function wireShellEvents() {
  elements.titleInput.addEventListener("input", () => {
    if (state.current) {
      state.current.title = elements.titleInput.value;
      const listItem = state.documents.find((item) => item.doc_did === state.current.doc_did);
      if (listItem) {
        listItem.title = elements.titleInput.value;
      }
    }
    setDirty(true);
    scheduleAutosave();
  });
  elements.editor.addEventListener("input", () => {
    syncEditorMutation();
  });
  elements.newButton.addEventListener("click", () => createDocument(""));
  elements.saveButton.addEventListener("click", () => {
    void saveCurrent().catch((error) => {
      reportSaveFailure(error, "Save failed.");
    });
  });
  elements.saveAsButton.addEventListener("click", openSaveAsModal);
  elements.publishButton.addEventListener("click", publishCurrent);
  elements.unpublishButton.addEventListener("click", unpublishCurrent);
  elements.deleteButton.addEventListener("click", requestDeleteCurrent);
  elements.sidebarToggle.addEventListener("click", () => setSidebarCollapsed(!state.sidebarCollapsed));
  elements.sidebarSearch.addEventListener("input", () => {
    state.filterQuery = elements.sidebarSearch.value || "";
    renderDocumentsList();
  });
  elements.modeWrite.addEventListener("click", () => setWorkspaceView("write"));
  elements.modeSplit.addEventListener("click", () => setWorkspaceView("split"));
  elements.modeRead.addEventListener("click", () => setWorkspaceView("read"));
  elements.findInput.addEventListener("input", () => updateFindMatches());
  elements.findPrev.addEventListener("click", () => activateFindMatch(findIndex - 1));
  elements.findNext.addEventListener("click", () => activateFindMatch(findIndex + 1));
  elements.findClose.addEventListener("click", closeFindBar);
  elements.panePreview.addEventListener("click", () => setInspectorView("preview"));
  elements.paneOutline.addEventListener("click", () => setInspectorView("outline"));
  elements.paneHistory.addEventListener("click", () => setInspectorView("history"));
  elements.preview.addEventListener("change", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement) || !target.classList.contains("task-checkbox")) {
      return;
    }
    setTaskLineChecked(Number.parseInt(target.dataset.sourceLine || "-1", 10), target.checked);
  });
  elements.documentsList.addEventListener("contextmenu", (event) => {
    const button = event.target instanceof Element ? event.target.closest(".document-list-item") : null;
    event.preventDefault();
    openContextMenu(event.clientX, event.clientY, button?.dataset.docDid || "");
  });
  elements.contextMenu.addEventListener("click", (event) => {
    const button = event.target instanceof Element ? event.target.closest("[data-menu-action]") : null;
    if (!button) {
      return;
    }
    void runContextMenuAction(button.dataset.menuAction || "").catch((error) => {
      reportSaveFailure(error, "Documents action could not be completed.");
    });
  });
  document.addEventListener("click", (event) => {
    if (event.target instanceof Element && event.target.closest("#context-menu")) {
      return;
    }
    closeContextMenu();
  });
  elements.saveAsCancel.addEventListener("click", closeSaveAsModal);
  elements.saveAsModal.addEventListener("click", (event) => {
    if (event.target === elements.saveAsModal) {
      closeSaveAsModal();
    }
  });
  elements.confirmCancel.addEventListener("click", () => closeConfirmModal("cancel"));
  elements.confirmSecondaryAction.addEventListener("click", () => closeConfirmModal("secondary"));
  elements.confirmAction.addEventListener("click", () => closeConfirmModal("confirm"));
  elements.confirmModal.addEventListener("click", (event) => {
    if (event.target === elements.confirmModal) {
      closeConfirmModal("cancel");
    }
  });
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.origin === homeOrigin && event.source === window.top) {
      if (!data || typeof data !== "object" || data.type !== "documents:open-chat-attachment") {
        return;
      }
      (async () => {
        try {
          const opened = await openChatAttachment(data);
          if (opened === false) {
            return;
          }
          sendChatAttachmentOpenResult({
            attachmentId: data.attachmentId,
            ok: true,
            message: "Opened attachment in Documents.",
          });
        } catch (error) {
          console.error("documents open chat attachment failed", error);
          const message = readableDocumentsError(error, "Could not open Chat attachment.");
          setStatus(message, "warning");
          sendChatAttachmentOpenResult({
            attachmentId: data.attachmentId,
            ok: false,
            message,
          });
        }
      })();
      return;
    }
    if (event.origin !== "null" || event.source !== window.parent) {
      return;
    }
    if (data && typeof data === "object" && data.type === DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE) {
      void handleHomeWindowCloseRequest(event);
      return;
    }
    if (!data || typeof data !== "object" || data.type !== "elastos:menu-command" || typeof data.cmd !== "string") {
      return;
    }
    handleHomeMenuCommand(data.cmd);
  });
  elements.saveAsForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const title = elements.saveAsTitleInput.value.trim();
    const fileName = elements.saveAsFileInput.value.trim();
    closeSaveAsModal();
    try {
      await saveAsCurrent(title, fileName);
    } catch (error) {
      setStatus(error.message || "Save as failed.", "warning");
    }
  });
  document.addEventListener("keydown", async (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      try {
        await saveCurrent();
      } catch (error) {
        reportSaveFailure(error, "Save failed.");
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      openFindBar();
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "g") {
      event.preventDefault();
      if (elements.findBar.classList.contains("hidden")) {
        openFindBar();
      } else {
        activateFindMatch(findIndex + (event.shiftKey ? -1 : 1));
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter" && event.target === elements.editor) {
      event.preventDefault();
      toggleTaskAtCursor();
    }
    if (event.key === "Escape" && elements.saveAsModal.classList.contains("open")) {
      closeSaveAsModal();
    }
    if (event.key === "Escape" && elements.confirmModal.classList.contains("open")) {
      closeConfirmModal("cancel");
    }
    if (event.key === "Escape" && !elements.findBar.classList.contains("hidden")) {
      closeFindBar();
    }
    if (event.key === "Escape" && !elements.contextMenu.classList.contains("hidden")) {
      closeContextMenu();
    }
  });
  window.addEventListener("beforeunload", (event) => {
    if (!state.dirty) {
      return;
    }
    event.preventDefault();
    event.returnValue = "";
  });
}

async function loadShellMode() {
  state.mode = "shell";
  elements.documentsShell.classList.remove("hidden");
  setSidebarCollapsed(window.innerWidth <= 720);
  setWorkspaceView(preferredWorkspaceView(state.initialView));
  setInspectorView("preview");
  wireShellEvents();
  announceHomeChrome();
  try {
    await loadSummary();
  } catch (error) {
    elements.documentsList.innerHTML = "";
    const message = readableDocumentsError(error, "Documents could not be loaded.");
    elements.preview.innerHTML = '<div class="error-state">' + escapeHtml(message) + "</div>";
    setStatus(message, "warning");
  }
}

async function loadText(path) {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) {
    throw new Error("Could not load " + path);
  }
  return response.text();
}

async function loadJson(path) {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) {
    throw new Error("Could not load " + path);
  }
  return response.json();
}

function setSharePreview(title, markdown, subtitle, pills) {
  document.title = title ? title + " · Documents" : "Documents";
  elements.shareTitle.textContent = title || "Documents";
  elements.shareSubtitle.textContent = subtitle || "Read-only markdown surface.";
  elements.sharePreview.innerHTML = '<article class="markdown-body">' + renderMarkdownDocument(markdown || "").html + "</article>";
  elements.shareTopline.innerHTML = "";
  (pills || []).forEach((pill) => {
    const span = document.createElement("span");
    span.className = "share-token";
    span.innerHTML = "<strong>" + escapeHtml(pill.label) + "</strong><span>" + escapeHtml(pill.value) + "</span>";
    elements.shareTopline.appendChild(span);
  });
}

async function loadShareBundle(options) {
  const basePath = options && options.basePath ? options.basePath : "";
  const sourceUri = options && options.sourceUri ? options.sourceUri : "";
  const rawCid = options && options.rawCid ? String(options.rawCid || "").trim() : "";
  const allowRawCidFallback = !!(options && options.allowRawCidFallback && rawCid);
  state.mode = "share";
  elements.shareShell.classList.remove("hidden");

  const params = new URLSearchParams(window.location.search);
  const requestedFile = params.get("file");

  try {
    const files = await loadJson(basePath + "_files.json");
    const shareMeta = await loadJson(basePath + "_share.json").catch(() => null);
    if (!Array.isArray(files) || files.length === 0) {
      throw new Error("Shared revision has no documents.");
    }
    if (requestedFile && !files.includes(requestedFile)) {
      throw new Error("Shared document not found.");
    }
    const activeFile = requestedFile || files[0];
    elements.shareSidebar.classList.toggle("hidden", files.length <= 1);
    elements.shareSidebarMeta.textContent = shareMeta ? "Version " + shareMeta.version : "";
    elements.shareDocList.innerHTML = "";

    files.forEach((file) => {
      const link = document.createElement("a");
      link.className = "share-doc-link" + (file === activeFile ? " active" : "");
      const linkParams = new URLSearchParams(window.location.search);
      linkParams.set("file", file);
      link.href = window.location.pathname + "?" + linkParams.toString();
      link.innerHTML =
        '<span class="share-doc-title">' + escapeHtml(file.replace(/\.md$/i, "")) + "</span>" +
        '<span class="share-doc-meta">' + escapeHtml(file) + "</span>";
      elements.shareDocList.appendChild(link);
    });

    const markdown = await loadText(basePath + encodeURIComponent(activeFile));
    const title = markdown.match(/^#\s+(.+)$/m)?.[1]?.trim() || activeFile.replace(/\.md$/i, "");
    const pills = [];
    if (sourceUri) {
      pills.push({ label: "Revision", value: sourceUri });
    }
    if (shareMeta && shareMeta.share_id) {
      pills.push({ label: "Document", value: shareMeta.share_id });
    }
    if (shareMeta && shareMeta.version) {
      pills.push({ label: "Version", value: String(shareMeta.version) });
    }
    setSharePreview(title, markdown, "Immutable shared revision.", pills);
    return;
  } catch (error) {
    if (allowRawCidFallback) {
      try {
        await loadRawCidMode(rawCid, sourceUri);
        return;
      } catch (fallbackError) {
        elements.sharePreview.innerHTML = '<div class="error-state">' + escapeHtml(readableDocumentsError(fallbackError, "Shared content could not be loaded.")) + "</div>";
        return;
      }
    }
    elements.sharePreview.innerHTML = '<div class="error-state">' + escapeHtml(readableDocumentsError(error, "Shared document could not be loaded.")) + "</div>";
  }
}

async function loadRawCidMode(cid, sourceUri) {
  const cleanCid = String(cid || "").trim();
  if (!cleanCid) {
    throw new Error("This shared file link is incomplete.");
  }
  state.mode = "share";
  elements.shareShell.classList.remove("hidden");
  elements.shareSidebar.classList.add("hidden");
  elements.shareSidebarMeta.textContent = "";
  elements.shareDocList.innerHTML = "";

  const contentPath = "/content/" + encodeURIComponent(cleanCid);
  const response = await fetch(contentPath, { cache: "no-store" });
  if (!response.ok) {
    throw new Error("Could not load " + contentPath);
  }
  const contentType = response.headers.get("content-type") || "application/octet-stream";
  const buffer = await response.arrayBuffer();
  const pills = [
    { label: "Content", value: sourceUri || "elastos://" + cleanCid },
    { label: "Type", value: contentType },
    { label: "Size", value: formatBytes(buffer.byteLength) },
  ];
  if (looksLikeTextContent(contentType, buffer)) {
    const markdown = new TextDecoder("utf-8").decode(buffer);
    const title = markdown.match(/^#\s+(.+)$/m)?.[1]?.trim() || shortCid(cleanCid);
    setSharePreview(title, markdown, "Shared file.", pills);
    return;
  }

  document.title = shortCid(cleanCid) + " · Documents";
  elements.shareTitle.textContent = shortCid(cleanCid);
  elements.shareSubtitle.textContent = "Shared binary file.";
  elements.shareTopline.innerHTML = "";
  pills.forEach((pill) => {
    const span = document.createElement("span");
    span.className = "share-token";
    span.innerHTML = "<strong>" + escapeHtml(pill.label) + "</strong><span>" + escapeHtml(pill.value) + "</span>";
    elements.shareTopline.appendChild(span);
  });
  const download = document.createElement("a");
  download.className = "action-primary";
  download.href = contentPath;
  download.download = cleanCid;
  download.textContent = "Download content";
  elements.sharePreview.innerHTML = '<div class="empty-document"><h2>Binary file</h2><p>This link points to a file that Documents cannot preview.</p></div>';
  elements.sharePreview.querySelector(".empty-document").appendChild(download);
}

async function loadShareMode() {
  await loadShareBundle({ basePath: "", sourceUri: "" });
}

async function loadPublishedRevisionMode(cid) {
  const cleanCid = String(cid || "").trim();
  if (!cleanCid) {
    throw new Error("This published document link is incomplete.");
  }
  await loadShareBundle({
    basePath: "/s/" + encodeURIComponent(cleanCid) + "/",
    sourceUri: "elastos://" + cleanCid,
    rawCid: cleanCid,
    allowRawCidFallback: true,
  });
}

function loadLockedMode() {
  state.mode = "locked";
  document.title = "Open Documents from Home · ElastOS";
  elements.shareShell.classList.remove("hidden");
  elements.shareSidebar.classList.add("hidden");
  elements.shareTopline.innerHTML = '<span class="share-token"><strong>Access</strong><span>Home required</span></span>';
  elements.shareTitle.textContent = "Open Documents from Home";
  elements.shareSubtitle.textContent = "Documents needs a Home launch to edit local markdown.";
  elements.sharePreview.innerHTML =
    '<div class="empty-document"><h2>Documents is ready.</h2><p>Open it from Home to create, edit, or publish documents.</p></div>';
}

(async function init() {
  if (state.homeToken) {
    await loadShellMode();
  } else if (window.location.pathname.startsWith("/apps/documents")) {
    loadLockedMode();
  } else {
    await loadShareMode();
  }
})();

elements.copyPublishedLink.addEventListener("click", async () => {
  try {
    await copyPublishedUri(elements.copyPublishedLink.dataset.copyUri || "");
  } catch (error) {
    console.error("documents copy link failed", error);
    setStatus(readableDocumentsError(error, "Copy failed."), "warning");
  }
});
