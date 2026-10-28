import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";

const state = {
  homeToken: new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "",
  initialDocDid: new URLSearchParams(window.location.search).get("doc") || "",
  initialCid: new URLSearchParams(window.location.search).get("cid") || "",
  // A shared file's stated name: its title and default save name only.
  initialName: sharedFileName(new URLSearchParams(window.location.search).get("name")),
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
  // "Save automatically"; Home remembers the person's choice (off by default).
  autosave: false,
  workspaceView: "write",
  inspectorView: "preview",
  sidebarCollapsed: false,
  filterQuery: "",
  pendingConfirmation: null,
  pendingChatAttachmentId: "",
};
// Launched by Home: framed, with the Home token Home put in the launch
// address. Decided once here; the current address may change later.
const DOCUMENTS_INSIDE_HOME = window.top !== window && !!state.homeToken;
const homeOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";
const homeNavigation = createHomeNavigationClient({ homeToken: state.homeToken, homeOrigin });
const DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE =
  "elastos.documents.window-close.request/v1";
const DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE =
  "elastos.documents.window-close.result/v1";
const DOCUMENTS_PREFERENCES_REQUEST_TYPE =
  "elastos.documents.preferences.request/v1";
const DOCUMENTS_PREFERENCES_SET_TYPE =
  "elastos.documents.preferences.set/v1";
const DOCUMENTS_PREFERENCES_TYPE =
  "elastos.documents.preferences/v1";

const elements = {
  documentsShell: document.getElementById("documents-shell"),
  documentsList: document.getElementById("documents-list"),
  sidebarSearch: document.getElementById("sidebar-search"),
  titleInput: document.getElementById("title-input"),
  sidebarToggle: document.getElementById("sidebar-toggle"),
  sidebarShow: document.getElementById("sidebar-show"),
  saveButton: document.getElementById("save-button"),
  saveToLibrary: document.getElementById("save-to-library"),
  autosaveToggle: document.getElementById("autosave-toggle"),
  saveAsButton: document.getElementById("save-as-button"),
  deleteButton: document.getElementById("delete-button"),
  statusRow: document.getElementById("status-row"),
  statusText: document.getElementById("status-text"),
  statusAction: document.getElementById("status-action"),
  findBar: document.getElementById("find-bar"),
  findInput: document.getElementById("find-input"),
  findCount: document.getElementById("find-count"),
  findPrev: document.getElementById("find-prev"),
  findNext: document.getElementById("find-next"),
  findClose: document.getElementById("find-close"),
  contextMenu: document.getElementById("context-menu"),
  editor: document.getElementById("editor"),
  preview: document.getElementById("preview"),
  previewPanel: document.getElementById("preview-panel"),
  outlinePanel: document.getElementById("outline-panel"),
  newButton: document.getElementById("new-document"),
  modeWrite: document.getElementById("mode-write"),
  modeSplit: document.getElementById("mode-split"),
  toolbarMore: document.querySelector(".toolbar-more"),
  moreButton: document.getElementById("more-button"),
  modeRead: document.getElementById("mode-read"),
  panePreview: document.getElementById("pane-preview"),
  paneOutline: document.getElementById("pane-outline"),
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
      html += renderMarkdownLink(match[5], renderInlineMarkdown(match[4]));
    }
    offset = match.index + match[0].length;
  }
  return html + escapeHtml(text.slice(offset));
}

// Inside Home rendered Markdown has no href at all: any navigation would
// replace the Documents frame (or its address), and a replaced frame cannot
// answer Home's close request. A web link (same origin included) asks Home
// through `activateMarkdownLink` (#436, as Chat); a link to a place in the
// document scrolls there without touching the address; every other link
// (relative paths, elastos://, localhost://, refused addresses) is plain
// text. Outside Home links are normal anchors. `DOCUMENTS_INSIDE_HOME` is
// decided once at launch, never from the current address.
function renderMarkdownLink(value, labelHtml) {
  const href = markdownHref(value);
  if (!DOCUMENTS_INSIDE_HOME) {
    return '<a href="' + escapeHtml(href) + '" rel="noopener noreferrer">' + labelHtml + "</a>";
  }
  if (/^https?:\/\//i.test(href)) {
    return '<a data-open-external="' + escapeHtml(href) + '" role="link" tabindex="0">' + labelHtml + "</a>";
  }
  if (href.length > 1 && href.startsWith("#")) {
    return '<a data-scroll-to="' + escapeHtml(href.slice(1)) + '" role="link" tabindex="0">' + labelHtml + "</a>";
  }
  return "<span>" + labelHtml + "</span>";
}

// Browsers drop tabs and newlines from an address and read "\" as "/", so
// spellings such as "/\host", "/<tab>/host" or "\\host" would reach another
// host; any address with a control character or backslash is refused.
function markdownHref(value) {
  const href = String(value || "").trim();
  if (/[\u0000-\u001f\u007f\\]/.test(href)) {
    return "#";
  }
  return /^(https?:\/\/|elastos:\/\/|localhost:\/\/|#|\/(?!\/)|\.{1,2}\/)/i.test(href) ? href : "#";
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
  const inline = (text) => renderInlineMarkdown(text);
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
      blocks.push("<h" + heading[1].length + ' id="' + id + '">' + inline(title) + "</h" + heading[1].length + ">");
      outline.push({ depth: heading[1].length, title: title, id: id });
      index += 1;
      continue;
    }

    if (/^>\s?/.test(line)) {
      const quote = [];
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(inline(lines[index].replace(/^>\s?/, "")));
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
          ? '<label class="task-item-label"><input class="task-checkbox" type="checkbox" aria-label="Toggle task completion" data-source-line="' + String(index) + '"' + (checked ? " checked" : "") + '><span>' + inline(label) + "</span></label>"
          : '<span>' + inline((checked ? "[x] " : "[ ] ") + label) + "</span>";
        items.push('<li class="task-item">' + checkbox + "</li>");
        index += 1;
      }
      blocks.push("<ul>" + items.join("") + "</ul>");
      continue;
    }

    if (/^[-*]\s+/.test(line)) {
      const items = [];
      while (index < lines.length && /^[-*]\s+/.test(lines[index])) {
        items.push("<li>" + inline(lines[index].replace(/^[-*]\s+/, "")) + "</li>");
        index += 1;
      }
      blocks.push("<ul>" + items.join("") + "</ul>");
      continue;
    }

    if (/^\d+\.\s+/.test(line)) {
      const items = [];
      while (index < lines.length && /^\d+\.\s+/.test(lines[index])) {
        items.push("<li>" + inline(lines[index].replace(/^\d+\.\s+/, "")) + "</li>");
        index += 1;
      }
      blocks.push("<ol>" + items.join("") + "</ol>");
      continue;
    }

    if (/^\|.*\|$/.test(line) && index + 1 < lines.length && /^\|?[\s:-]+\|[\s|:-]*$/.test(lines[index + 1])) {
      const headerCells = line.split("|").slice(1, -1).map((cell) => "<th>" + inline(cell.trim()) + "</th>");
      index += 2;
      const rows = [];
      while (index < lines.length && /^\|.*\|$/.test(lines[index])) {
        const cells = lines[index].split("|").slice(1, -1).map((cell) => "<td>" + inline(cell.trim()) + "</td>");
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
      paragraph.push(inline(lines[index]));
      index += 1;
    }
    if (!paragraph.length) {
      blocks.push("<p>" + inline(line) + "</p>");
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

// The same rule as Home's sharedFileName (home-external-link.js, which names
// the Runtime limit it follows): no path separators, control characters or
// leading dots, at most 200 UTF-8 bytes, cut before the extension.
function sharedFileName(value) {
  if (typeof value !== "string" || /[/\\]/.test(value)) return "";
  const name = value.replace(/[\u0000-\u001f\u007f-\u009f]/g, "").replace(/^[.\s]+/, "").trimEnd();
  const encoder = new TextEncoder();
  const bytes = (text) => encoder.encode(text).length;
  if (bytes(name) <= 200) return name;
  const dot = name.lastIndexOf(".");
  const extension = dot > 0 && bytes(name.slice(dot)) <= 16 ? name.slice(dot) : "";
  let stem = "";
  let used = bytes(extension);
  for (const character of extension ? name.slice(0, dot) : name) {
    used += bytes(character);
    if (used > 200) break;
    stem += character;
  }
  stem = stem.replace(/[\s.]+$/, "");
  return stem ? stem + extension : "";
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

let draftSerial = 0;

function createDraftDocument() {
  draftSerial += 1;
  return {
    doc_did: "",
    document_uri: "",
    draft_key: "draft-" + draftSerial,
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

// A list row's title: the document's title, or a plain file's name, without a
// .md or .txt ending, the same rule for both.
function listTitle(item) {
  const title = String(item.title || "").trim();
  return title.replace(/\.(md|txt)$/i, "") || title || "Untitled";
}

// The folder a file sits in, when that is a folder inside Documents.
function listFolder(item) {
  const parts = String(item.file_name || "").split("/");
  return parts.length > 1 ? parts[parts.length - 2] : "";
}

// When a row last changed, short: "Today 4:41 PM", "Yesterday", "Oct 26"
// (with the year when it is not this year).
function shortTime(timestamp, now = new Date()) {
  if (!timestamp) {
    return "";
  }
  const date = new Date(timestamp * 1000);
  const startOfDay = (value) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const daysAgo = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000);
  if (daysAgo === 0) {
    return "Today " + date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  if (daysAgo === 1) {
    return "Yesterday";
  }
  return date.toLocaleDateString(undefined, date.getFullYear() === now.getFullYear()
    ? { month: "short", day: "numeric" }
    : { month: "short", day: "numeric", year: "numeric" });
}

function preferredWorkspaceView(view) {
  return view === "read" ? "read" : view === "split" ? "split" : "write";
}

function clearStatus() {
  elements.statusText.textContent = "";
  elements.statusRow.classList.add("hidden");
  elements.statusAction.classList.add("hidden");
}

// A Library file whose save failed (most often because it changed in another
// window) can be reloaded from what is stored, here or from its list row.
function libraryFileSaveFailed() {
  return isLibraryFileProjection(state.current) && state.dirty &&
    saveRecovery?.phase === "library" && isCurrentSessionTarget(saveRecovery.target);
}

async function loadSavedLibraryVersion() {
  const current = state.current;
  if (!isLibraryFileProjection(current)) {
    return;
  }
  const confirmed = await confirmInCapsule({
    title: "Load the saved version?",
    message: "Your edits to " + (current.file_name || "this file") + " will be lost.",
    confirmLabel: "Load saved version",
    danger: true,
  });
  if (!confirmed || state.current !== current) {
    return;
  }
  unsavedDocuments.delete(unsavedKey(current));
  state.dirty = false;
  await loadLibraryObject(current.library_object_uri).catch((error) => {
    showError(readableDocumentsError(error, "This file could not be opened."));
  });
}

// Save as copy for a Library file: the edits become a new file in Documents,
// through the same import Chat attachments use, and that copy opens.
async function saveLibraryFileCopy(fileName) {
  const target = { sessionId: state.currentSessionId, identity: documentIdentity(state.current) };
  const name = String(fileName || "").trim() || suggestedDuplicateFileName(state.current) || "Copy.txt";
  const response = await documentsProviderApi("import_chat_attachment", {
    attachment_id: "copy-" + window.crypto.randomUUID(),
    file_name: name,
    mime: /\.md$/i.test(name) ? "text/markdown" : "text/plain",
    body: elements.editor.value,
  });
  if (!isCurrentDocumentTarget(target)) return;
  const copy = requireSavedWorkingCopy(response.document);
  // The edits now live in the copy; the original file keeps what is stored.
  replaceCurrentDocument(copy);
  upsertDocumentListItem(copy);
  renderDocumentsList();
  renderCurrentDocument();
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
    !state.autosave ||
    !state.current ||
    state.mode !== "shell" ||
    !state.dirty ||
    saveRecovery?.target.sessionId === state.currentSessionId ||
    pendingHomeWindowCloseTarget?.sessionId === state.currentSessionId
  ) {
    return;
  }
  autosaveTimerId = window.setTimeout(() => {
    void saveCurrent({ autosave: true }).catch((error) => {
      reportSaveFailure(error, "Autosave failed.");
    });
  }, 900);
}

function applyAutosavePreference(enabled) {
  state.autosave = enabled === true;
  if (elements.autosaveToggle) {
    elements.autosaveToggle.setAttribute("aria-checked", String(state.autosave));
  }
  scheduleAutosave();
}

function toggleAutosavePreference() {
  const enabled = !state.autosave;
  applyAutosavePreference(enabled);
  if (state.homeToken && window.parent !== window) {
    window.parent.postMessage({ type: DOCUMENTS_PREFERENCES_SET_TYPE, homeToken: state.homeToken, autosave: enabled }, "*");
  }
}

function requestAutosavePreference() {
  if (state.homeToken && window.parent !== window) {
    window.parent.postMessage({ type: DOCUMENTS_PREFERENCES_REQUEST_TYPE, homeToken: state.homeToken }, "*");
  }
}

function handleAutosavePreferenceMessage(data) {
  if (!hasExactKeys(data, ["type", "homeToken", "autosave"]) ||
      data.type !== DOCUMENTS_PREFERENCES_TYPE ||
      data.homeToken !== state.homeToken ||
      typeof data.autosave !== "boolean") {
    return false;
  }
  applyAutosavePreference(data.autosave);
  return true;
}

function unsavedDocumentName() {
  return elements.titleInput.value.trim() || state.current?.file_name || "Untitled";
}

// Unsaved edits ask before they leave the editor: Save keeps them, Don't save
// drops them, Cancel stays. Resolves true when the caller may go on.
async function resolveUnsavedChanges() {
  if (!state.current || !state.dirty) return true;
  const sessionId = state.currentSessionId;
  clearAutosaveTimer();
  const choice = await chooseInCapsule({
    title: "Save changes to " + unsavedDocumentName() + "?",
    message: "Your changes will be lost if you don't save them.",
    confirmLabel: "Save",
    secondaryLabel: "Don't save",
    defaultFocus: "cancel",
  });
  if (sessionId !== state.currentSessionId) return false;
  if (choice === "secondary") return true;
  if (choice !== "confirm") {
    scheduleAutosave();
    return false;
  }
  try {
    await saveCurrent();
  } catch (error) {
    reportSaveFailure(error, "Save failed.");
    return false;
  }
  return sessionId === state.currentSessionId && !state.dirty;
}

// Unsaved documents this window holds while another one is open, keyed by
// document. Switching documents, New and opening a file keep the text here
// instead of asking; switching back brings it back. Only closing the window,
// Lock and sign-out ask, since only they would lose it.
const unsavedDocuments = new Map();

function unsavedKey(document) {
  if (!document) return "";
  return isDraftDocument(document) ? "draft:" + (document.draft_key || "") : documentIdentity(document);
}

function listItemUnsavedKey(item) {
  if (item.draft_key) return "draft:" + item.draft_key;
  return item.library_object_uri ? "library:" + item.library_object_uri : "document:" + item.doc_did;
}

function hasUnsavedDocuments() {
  return (state.dirty && !!state.current && !state.current.shared) || unsavedDocuments.size > 0;
}

// Keeps the open document's unsaved text just before another replaces it.
function keepUnsavedCurrent() {
  const current = state.current;
  if (!current || !state.dirty || current.shared) return;
  unsavedDocuments.set(unsavedKey(current), {
    document: { ...current, title: elements.titleInput.value, body: elements.editor.value },
    savedWorkingCopy: savedWorkingCopy ? { ...savedWorkingCopy } : null,
    saveRecovery: saveRecovery && isCurrentSessionTarget(saveRecovery.target) ? { ...saveRecovery } : null,
  });
}

// Before leaving a document: a new one still being created finishes first,
// so it is not created twice; with Save automatically on and no save under
// way, the document saves now, as it would have anyway. A save under way for
// a stored document finishes on its own (reconcileKeptAfterSave).
async function prepareToLeaveCurrent() {
  if (saveInFlight && isDraftDocument(state.current)) {
    await saveInFlight.catch(() => {});
  }
  if (!saveInFlight && state.autosave && state.dirty && state.current && !state.current.shared) {
    await saveCurrent({ autosave: true }).catch((error) => {
      reportSaveFailure(error, "Autosave failed.");
    });
  }
}

// A save that finishes after its document was left: the kept text is saved
// if it is what was sent, or else now builds on the saved revision.
function reconcileKeptAfterSave(identity, persistedDocument, requestedTitle, requestedBody) {
  const kept = unsavedDocuments.get(identity);
  if (!kept) {
    return;
  }
  if (kept.document.body === requestedBody && kept.document.title === requestedTitle) {
    unsavedDocuments.delete(identity);
  } else {
    kept.document = { ...kept.document, revision: persistedDocument.revision };
    kept.savedWorkingCopy = { ...persistedDocument };
    kept.saveRecovery = null;
  }
  renderDocumentsList();
}

// Brings back a document's unsaved text, keeping the open one's first.
function restoreUnsavedDocument(key) {
  const kept = unsavedDocuments.get(key);
  if (!kept) return false;
  keepUnsavedCurrent();
  unsavedDocuments.delete(key);
  replaceCurrentDocument({ ...kept.document });
  savedWorkingCopy = kept.savedWorkingCopy;
  if (kept.saveRecovery) {
    saveRecovery = { ...kept.saveRecovery,
      target: { sessionId: state.currentSessionId, identity: documentIdentity(state.current) } };
  }
  state.mode = "shell";
  elements.shareShell.classList.add("hidden");
  elements.documentsShell.classList.remove("hidden");
  renderCurrentDocument();
  setDirty(true);
  scheduleAutosave();
  return true;
}

async function openKeptDraft(draftKey) {
  await prepareToLeaveCurrent();
  restoreUnsavedDocument("draft:" + draftKey);
}

function unsavedDocumentNames() {
  const names = state.dirty && state.current && !state.current.shared ? [unsavedDocumentName()] : [];
  for (const kept of unsavedDocuments.values()) {
    names.push(kept.document.title.trim() || kept.document.file_name || "Untitled");
  }
  return names;
}

// Save all, from closing the window: the open document, then each kept one.
// Stops at the first that does not save, which stays open with its text.
async function saveAllUnsaved() {
  // Edits made while a save runs, in any document, are saved too: close only
  // when nothing in the window is left unsaved. Each round saves one document.
  for (let round = 0; round < 100; round += 1) {
    if (state.dirty && state.current && !state.current.shared) {
      await saveCurrent();
      continue;
    }
    const key = unsavedDocuments.keys().next().value;
    if (key === undefined) {
      return true;
    }
    restoreUnsavedDocument(key);
  }
  return !hasUnsavedDocuments();
}

function reportSaveFailure(error, fallback) {
  if (error?.documentsTarget && !isCurrentDocumentTarget(error.documentsTarget)) return;
  showError(readableDocumentsError(error, fallback));
  if (libraryFileSaveFailed()) {
    elements.statusAction.textContent = "Load saved version";
    elements.statusAction.classList.remove("hidden");
  }
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

// Home frames apps without popups or top navigation, so inside Home a web
// link in a Markdown view has no href (renderMarkdownLink) and asks Home to
// show the exact address and open it only when the person chooses Open
// (#436, the same request as Chat's links). Without the Home session it
// refuses and says so. A link to a place in the document scrolls there.
// Nothing navigates either way.
function activateMarkdownLink(event) {
  if (event.type === "keydown" && event.key !== "Enter" && event.key !== " ") {
    return;
  }
  const link = event.target instanceof Element ? event.target.closest("[data-open-external], [data-scroll-to]") : null;
  if (!link) {
    return;
  }
  event.preventDefault();
  const selection = window.getSelection();
  if (event.type === "click" && selection && !selection.isCollapsed && selection.containsNode(link, true)) {
    return;
  }
  if (link.hasAttribute("data-scroll-to")) {
    const target = document.getElementById(link.getAttribute("data-scroll-to") || "");
    if (target) {
      target.scrollIntoView({ block: "start" });
    }
    return;
  }
  if (!canAnnounceHomeChrome()) {
    const message = "Open Documents from Home to open web links.";
    if (state.mode === "share") {
      elements.shareSubtitle.textContent = message;
    } else {
      showError(message);
    }
    return;
  }
  const url = link.getAttribute("data-open-external") || "";
  window.top.postMessage({ type: "home:open-external", url, homeToken: state.homeToken }, homeOrigin);
}

// Every Markdown view renders through here, so each one gets the inside-Home
// link policy (renderMarkdownLink) and the handlers for its controls.
const markdownLinkContainers = new WeakSet();
function renderMarkdownInto(container, markdown, options) {
  const rendered = renderMarkdownDocument(markdown, options);
  container.innerHTML = rendered.html;
  if (!markdownLinkContainers.has(container)) {
    markdownLinkContainers.add(container);
    container.addEventListener("click", activateMarkdownLink);
    container.addEventListener("keydown", activateMarkdownLink);
  }
  return rendered;
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
  const rendered = renderMarkdownInto(elements.preview, elements.editor.value || "", { interactiveTasks: true });
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
  if (state.current?.shared) {
    refreshPreviewFromEditor();
    return;
  }
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
  if (state.current?.shared) return;
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

// A shared file shows the reading view and Save to Library only; every other
// document gets Write, Split and Save back.
function setSharedChrome(shared) {
  elements.modeWrite.classList.toggle("hidden", shared);
  elements.modeSplit.classList.toggle("hidden", shared);
  elements.saveButton.classList.toggle("hidden", shared);
  elements.saveToLibrary.classList.toggle("hidden", !shared);
  if (shared) {
    setWorkspaceView("read");
  }
}

function setWorkspaceView(view) {
  // A shared read-only file has the reading view only.
  state.workspaceView = state.current?.shared ? "read" : preferredWorkspaceView(view);
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
}

function setInspectorView(view) {
  state.inspectorView = view === "outline" ? "outline" : "preview";
  elements.previewPanel.classList.toggle("hidden", state.inspectorView !== "preview");
  elements.outlinePanel.classList.toggle("hidden", state.inspectorView !== "outline");
  elements.panePreview.classList.toggle("active", state.inspectorView === "preview");
  elements.paneOutline.classList.toggle("active", state.inspectorView === "outline");
  if (!elements.findBar.classList.contains("hidden")) {
    updateFindMatches({ preserveIndex: true });
  }
}

// The line under the title carries errors only; saving and other progress stay
// quiet (the Save button and the list's "unsaved" marker show the dirty state).
function showError(message) {
  elements.statusText.textContent = readableDocumentsError(message, "Documents action could not be completed.");
  elements.statusRow.classList.remove("hidden");
}

function setDirty(dirty) {
  state.dirty = !!dirty;
  const hasCurrent = !!state.current;
  const hasPersistedCurrent = hasCurrent && !isDraftDocument(state.current);
  const isLibraryFile = isLibraryFileProjection(state.current);
  elements.saveButton.disabled = !hasCurrent || !state.dirty || !!state.current.shared;
  // The Save button itself shows that this document has unsaved changes.
  const unsaved = !elements.saveButton.disabled;
  elements.saveButton.classList.toggle("has-unsaved", unsaved);
  elements.saveButton.title = unsaved ? "Save (unsaved changes)" : "Save";
  elements.saveAsButton.disabled = !hasPersistedCurrent;
  elements.deleteButton.disabled = !hasPersistedCurrent || isLibraryFile;
  if (hasCurrent && !state.dirty) {
    clearStatus();
  }
  renderDocumentsList();
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
    setSharedChrome(false);
    elements.preview.innerHTML = '<div class="empty-state">Create or open a document.</div>';
    renderOutline([]);
    clearStatus();
    setDirty(false);
    return;
  }

  const shared = !!state.current.shared;
  elements.titleInput.disabled = isLibraryFileProjection(state.current);
  elements.titleInput.readOnly = shared;
  elements.editor.disabled = false;
  elements.editor.readOnly = shared;
  setSharedChrome(shared);
  elements.titleInput.value = state.current.title;
  elements.editor.value = state.current.body;
  refreshPreviewFromEditor();

  clearStatus();
  setDirty(false);
}

function renderDocumentsList() {
  elements.documentsList.innerHTML = "";
  const query = state.filterQuery.trim().toLowerCase();
  // New documents kept unsaved in this window list first, until saved.
  const keptDrafts = [...unsavedDocuments.values()]
    .filter((kept) => isDraftDocument(kept.document))
    .map((kept) => ({ draft_key: kept.document.draft_key, title: kept.document.title.trim() || "Untitled",
      file_name: "", updated_at: 0 }));
  const visibleDocuments = [...keptDrafts, ...state.documents].filter((item) => {
    if (!query) {
      return true;
    }
    return item.title.toLowerCase().includes(query) || item.file_name.toLowerCase().includes(query);
  });
  if (!visibleDocuments.length) {
    elements.documentsList.innerHTML = '<div class="sidebar-note">' + (query ? "No documents match this search." : "No documents yet.") + "</div>";
    return;
  }

  const currentKey = state.current ? state.current.library_object_uri || state.current.doc_did : "";
  visibleDocuments.forEach((item) => {
    const button = window.document.createElement("button");
    button.type = "button";
    const classes = ["document-list-item"];
    const isCurrent = !!currentKey && currentKey === (item.library_object_uri || item.doc_did);
    if (isCurrent) {
      classes.push("active");
    }
    button.className = classes.join(" ");
    if (item.draft_key) {
      button.dataset.draftKey = item.draft_key;
    } else if (item.library_object_uri) {
      button.dataset.objectUri = item.library_object_uri;
    } else {
      button.dataset.docDid = item.doc_did;
    }
    const unsaved = (isCurrent && state.dirty) || unsavedDocuments.has(listItemUnsavedKey(item));
    const meta = [listFolder(item), shortTime(item.updated_at)].filter(Boolean).join(" · ");
    if (item.file_name) {
      button.title = item.file_name;
    }
    button.innerHTML =
      '<span class="document-list-title-row"><span class="document-list-title">' + escapeHtml(listTitle(item)) + "</span>" +
      (unsaved ? '<span class="document-list-unsaved" role="img" aria-label="Unsaved changes" title="Unsaved changes"></span>' : "") +
      "</span>" +
      (meta ? '<span class="document-list-meta">' + escapeHtml(meta) + "</span>" : "");
    button.addEventListener("click", () => item.draft_key
      ? openKeptDraft(item.draft_key)
      : item.library_object_uri
        ? selectLibraryFile(item.library_object_uri)
        : selectDocument(item.doc_did));
    elements.documentsList.appendChild(button);
  });
}

// Every Documents request ends within this deadline, so a save that never
// answers fails, keeps the edits (a later Save checks what was stored), and
// frees a Lock or sign-out that waits on it.
const DOCUMENTS_REQUEST_DEADLINE_MS = 20_000;

async function documentsApi(path, options) {
  const request = { method: "GET", ...(options || {}) };
  request.headers = {
    ...(request.headers || {}),
    "x-elastos-home-token": state.homeToken,
  };
  if (request.body && !request.headers["content-type"]) {
    request.headers["content-type"] = "application/json";
  }
  const controller = new AbortController();
  const deadline = window.setTimeout(() => controller.abort(), DOCUMENTS_REQUEST_DEADLINE_MS);
  try {
    const response = await fetch(path, { ...request, signal: controller.signal });
    if (!response.ok) {
      const text = await response.text();
      throw new Error(text || "Documents request failed");
    }
    if (response.status === 204) {
      return null;
    }
    const contentType = response.headers.get("content-type") || "";
    if (contentType.includes("application/json")) {
      return await response.json();
    }
    return await response.text();
  } catch (error) {
    if (controller.signal.aborted) {
      throw new Error("Documents did not answer in time. Your edits are kept.");
    }
    throw error;
  } finally {
    window.clearTimeout(deadline);
  }
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

// Documents' own objects, then plain .md and .txt files in the Documents
// folder that have none (made in Library, say); those open as Library files
// and are known by their URI only (no doc_did, which a new draft also lacks).
function summaryListItems(summary) {
  const files = (summary.files || []).map((file) => ({
    library_object_uri: file.uri,
    title: file.name,
    file_name: file.path,
    updated_at: file.updated_at,
    latest_published_cid: null,
  }));
  return [...(summary.documents || []), ...files];
}

async function loadSummary() {
  const summary = await documentsProviderApi("summary");
  state.documents = summaryListItems(summary);
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
      await loadSharedFile(requestedCid);
      setWorkspaceView(preferredWorkspaceView(state.initialView));
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
      showError("Document not found: " + requestedUri + ". Nothing opened.");
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
  const title = elements.titleInput.value;
  const body = elements.editor.value;
  const response = await libraryObjectApi("read", { uri });
  // Edits made while the file was read stay; the file does not replace them.
  if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId ||
      title !== elements.titleInput.value || body !== elements.editor.value) return;
  const object = response.object || {};
  const name = object.name || baseName(object.uri || uri);
  keepUnsavedCurrent();
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
}

// A file shared in Chat opens in the normal layout as a read-only copy; Save
// to Library keeps a private copy. Documents shows the content and the file
// name the Chat post states, if any: no CID,
// address, type or size. Home opens Documents for a shared file only after it
// read the bytes as text (home-external-link.js), so the bytes are the file.
async function readSharedText(cid) {
  const response = await fetch("/content/" + encodeURIComponent(cid), { cache: "no-store" });
  if (!response.ok) {
    throw new Error("This shared file could not be opened.");
  }
  const buffer = await response.arrayBuffer();
  if (!looksLikeTextContent(response.headers.get("content-type") || "", buffer)) {
    throw new Error("Documents can open text files only.");
  }
  return new TextDecoder("utf-8").decode(buffer);
}

async function loadSharedFile(cid) {
  const sessionId = state.currentSessionId;
  let body;
  try {
    body = await readSharedText(cid);
  } catch (error) {
    if (sessionId !== state.currentSessionId) return;
    replaceCurrentDocument(null);
    renderDocumentsList();
    renderCurrentDocument();
    showError(readableDocumentsError(error, "This shared file could not be opened."));
    return;
  }
  if (sessionId !== state.currentSessionId) return;
  replaceCurrentDocument({ ...createDraftDocument(), title: state.initialName || "Shared file", body, shared: true, shared_cid: cid });
  renderDocumentsList();
  renderCurrentDocument();
}

// The default name for a saved shared file: .md and .txt stay, anything else
// becomes a .txt file, since Documents keeps text only.
function sharedSaveFileName(title) {
  return sharedFileName(/\.(md|txt)$/i.test(title) ? title : title + ".txt");
}

// Save to Library keeps the shared file under its own name in the Documents
// folder, through the same import Chat attachments use; saving the same file
// again opens the copy already kept.
async function saveSharedCopy() {
  const shared = state.current;
  if (!shared?.shared) return;
  const sessionId = state.currentSessionId;
  const fileName = sharedSaveFileName(shared.title);
  const response = await documentsProviderApi("import_chat_attachment", {
    attachment_id: "shared-" + shared.shared_cid,
    file_name: fileName,
    mime: /\.md$/i.test(fileName) ? "text/markdown" : "text/plain",
    body: shared.body,
  });
  if (sessionId !== state.currentSessionId) return;
  const document = requireSavedWorkingCopy(response.document);
  replaceCurrentDocument(document);
  upsertDocumentListItem(document);
  renderDocumentsList();
  renderCurrentDocument();
}

async function selectLibraryFile(uri) {
  if (state.current && state.current.library_object_uri === uri) {
    // The open file's own row reloads it only after its save failed.
    if (libraryFileSaveFailed()) {
      await loadSavedLibraryVersion();
    }
    return;
  }
  await prepareToLeaveCurrent();
  if (restoreUnsavedDocument("library:" + uri)) {
    return;
  }
  await loadLibraryObject(uri).catch((error) => {
    showError(readableDocumentsError(error, "This file could not be opened."));
  });
}

async function openChatAttachment(payload) {
  const attachmentId = String(payload && payload.attachmentId ? payload.attachmentId : "").trim();
  if (attachmentId && state.pendingChatAttachmentId === attachmentId) {
    return false;
  }
  state.pendingChatAttachmentId = attachmentId;
  try {
    await prepareToLeaveCurrent();
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
    if (state.current && state.current.doc_did === document.doc_did) {
      return true;
    }
    state.mode = "shell";
    elements.shareShell.classList.add("hidden");
    elements.documentsShell.classList.remove("hidden");
    if (restoreUnsavedDocument("document:" + document.doc_did)) {
      return true;
    }
    keepUnsavedCurrent();
    replaceCurrentDocument(document);
    upsertDocumentListItem(document);
    renderDocumentsList();
    renderCurrentDocument();
    setWorkspaceView(preferredWorkspaceView(state.initialView));
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
  const force = !!(options && options.force);
  const sameDocument = !!state.current && state.current.doc_did === docDid;
  // Clicking the open document keeps the editor as it is. An explicit reload,
  // or opening the stored copy after a save conflict, replaces the text here,
  // so it asks first; any other switch keeps the unsaved text in this window.
  const storedCopyWanted = sameDocument && state.dirty &&
    saveRecovery?.target.sessionId === state.currentSessionId;
  if (sameDocument && !force && !storedCopyWanted) {
    return;
  }
  const replacesText = sameDocument && state.dirty;
  if (replacesText) {
    const sessionBeforePrompt = state.currentSessionId;
    const proceed = await resolveUnsavedChanges();
    if (!proceed || sequence !== documentSelectionSequence || sessionBeforePrompt !== state.currentSessionId) {
      return;
    }
  } else {
    await prepareToLeaveCurrent();
    if (sequence !== documentSelectionSequence) {
      return;
    }
    if (restoreUnsavedDocument("document:" + docDid)) {
      return;
    }
  }
  const sessionId = state.currentSessionId;
  const title = elements.titleInput.value;
  const body = elements.editor.value;
  resetPendingSaveIntent();
  const response = await documentsProviderApi("get", { doc_did: docDid }).catch((error) => {
    if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId) return null;
    throw error;
  });
  if (sequence !== documentSelectionSequence || sessionId !== state.currentSessionId ||
      title !== elements.titleInput.value || body !== elements.editor.value) return;
  requireSavedWorkingCopy(response.document, docDid);
  if (!replacesText) {
    keepUnsavedCurrent();
  }
  replaceCurrentDocument(response.document);
  upsertDocumentListItem(response.document);
  renderDocumentsList();
  renderCurrentDocument();
}

async function createDocument(title) {
  await prepareToLeaveCurrent();
  keepUnsavedCurrent();
  startDraftDocument({ focusTitle: !title, focusEditor: !!title });
  if (title) {
    elements.titleInput.value = title;
    setDirty(true);
  }
  clearStatus();
}

function applySavedDocumentState(persistedDocument, saveTarget, requestedTitle, requestedBody) {
  if (!isCurrentDocumentTarget(saveTarget)) {
    reconcileKeptAfterSave(saveTarget.identity, persistedDocument, requestedTitle, requestedBody);
    return;
  }
  const libraryProjection = isLibraryFileProjection(persistedDocument);
  if (libraryProjection) {
    const listItem = state.documents.find((item) => item.library_object_uri === persistedDocument.library_object_uri);
    if (listItem) {
      listItem.updated_at = persistedDocument.updated_at;
      sortDocuments();
    }
  } else {
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
    setDirty(true);
    scheduleAutosave();
    return;
  }
  renderCurrentDocument();
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
  if (!state.current || state.current.shared) return;
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
  clearAutosaveTimer();
  const requestedTitle = elements.titleInput.value;
  const requestedBody = elements.editor.value;
  saveInFlight = (async () => {
    if (isLibraryFileProjection(saveDocument)) {
      // A write whose reply was lost (or timed out) may have landed. Check the
      // stored file before writing again, as ordinary saves do.
      if (saveRecovery?.phase === "library" && isCurrentDocumentTarget(saveRecovery.target)) {
        const recovery = saveRecovery;
        const stored = await libraryObjectApi("read", { uri: saveDocument.library_object_uri });
        if (!isCurrentDocumentTarget(saveTarget)) return;
        const storedObject = stored?.object || {};
        const storedBody = base64ToUtf8(stored?.data || "");
        if (storedObject.revision && storedBody === recovery.body) {
          saveDocument = { ...saveDocument, revision: storedObject.revision };
        } else if (storedObject.revision !== recovery.revision) {
          throw new Error("The file changed since you opened it. Your edits are kept here.");
        }
        saveRecovery = null;
      }
      saveRecovery = { target: { ...saveTarget }, phase: "library", revision: saveDocument.revision, body: requestedBody };
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
      }, saveTarget, requestedTitle, requestedBody);
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
          state.documents = summaryListItems(response);
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
          applySavedDocumentState(observed, saveTarget, recovery.inputTitle, recovery.body);
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
      // Listed from now on, so its text stays reachable if the save below fails.
      upsertDocumentListItem(saveDocument);
      renderDocumentsList();
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
    applySavedDocumentState(persisted, saveTarget, requestedTitle, requestedBody);
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
        void saveCurrent({ autosave: true }).catch((error) => {
          reportSaveFailure(error, "Autosave failed.");
        });
      }
    }
  }
}

function openSaveAsModal() {
  if (!state.current || isDraftDocument(state.current)) {
    return;
  }
  elements.saveAsTitleInput.value = elements.titleInput.value || state.current.title || "";
  // A Library file is saved as a copy beside the others in Documents.
  elements.saveAsFileInput.value = isLibraryFileProjection(state.current)
    ? suggestedDuplicateFileName(state.current)
    : state.current.file_name || "";
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
    if (!hasUnsavedDocuments()) {
      resumeAutosaveForHomeWindowClose(autosaveSuspension);
      return { ok: true, reason: "" };
    }
  }
  if (!sameHomeWindowCloseTarget(closeTarget)) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "stale_request" };
  }
  if (!hasUnsavedDocuments()) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: true, reason: "" };
  }
  if (!sameHomeWindowCloseTarget(closeTarget)) {
    resumeAutosaveForHomeWindowClose(autosaveSuspension);
    return { ok: false, reason: "stale_request" };
  }
  // One question for every unsaved document in this window.
  const names = unsavedDocumentNames();
  const decision = await chooseInCapsule({
    title: names.length === 1
      ? "Save changes to " + names[0] + "?"
      : "Save changes to " + names.length + " documents?",
    message: "Your changes will be lost if you don't save them.",
    confirmLabel: names.length === 1 ? "Save" : "Save all",
    secondaryLabel: "Don't save",
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
    // Saving every document switches between them on purpose, so the window's
    // own session check does not apply here.
    if (!(await saveAllUnsaved())) {
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
  if (!state.current || isDraftDocument(state.current)) {
    return;
  }
  if (isLibraryFileProjection(state.current)) {
    await saveLibraryFileCopy(fileName);
    return;
  }
  const target = { sessionId: state.currentSessionId, identity: documentIdentity(state.current) };
  const requestedTitle = elements.titleInput.value;
  const requestedBody = elements.editor.value;
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
    showError("Copy saved. Newer edits are kept in this document.");
    return;
  }
  replaceCurrentDocument(response.document);
  upsertDocumentListItem(response.document);
  renderDocumentsList();
  renderCurrentDocument();
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
    await documentsProviderApi("unpublish", {
      doc_did: state.current.doc_did,
    });
    await selectDocument(state.current.doc_did, { force: true });
  } catch (error) {
    console.error("documents unpublish failed", error);
    showError(readableDocumentsError(error, "Unpublish failed."));
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
    await documentsProviderApi("delete", { doc_did: deletedDocDid });
    state.documents = state.documents.filter((document) => document.doc_did !== deletedDocDid);
    if (state.current && state.current.doc_did === deletedDocDid) {
      replaceCurrentDocument(createDraftDocument());
    }
    renderDocumentsList();
    renderCurrentDocument();
  } catch (error) {
    console.error("documents delete failed", error);
    showError(readableDocumentsError(error, "Document could not be deleted."));
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
  if (action === "unpublish") {
    await unpublishCurrent();
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
  elements.autosaveToggle.addEventListener("click", toggleAutosavePreference);
  elements.saveToLibrary.addEventListener("click", () => {
    void saveSharedCopy().catch((error) => {
      reportSaveFailure(error, "Save failed.");
    });
  });
  elements.saveAsButton.addEventListener("click", openSaveAsModal);
  elements.statusAction.addEventListener("click", () => {
    void loadSavedLibraryVersion();
  });
  elements.deleteButton.addEventListener("click", requestDeleteCurrent);
  elements.sidebarToggle.addEventListener("click", () => {
    setSidebarCollapsed(true);
    elements.sidebarShow.focus({ preventScroll: true });
  });
  elements.sidebarShow.addEventListener("click", () => {
    setSidebarCollapsed(false);
    elements.sidebarToggle.focus({ preventScroll: true });
  });
  wireMoreMenu();
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
          showError(message);
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
    if (data && typeof data === "object" && data.type === DOCUMENTS_PREFERENCES_TYPE) {
      handleAutosavePreferenceMessage(data);
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
      showError(error.message || "Save as failed.");
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
    if (!hasUnsavedDocuments()) {
      return;
    }
    event.preventDefault();
    event.returnValue = "";
  });
}

function setMoreMenuOpen(open) {
  elements.toolbarMore.dataset.open = String(open);
  elements.moreButton.setAttribute("aria-expanded", String(open));
}

// Phone-only disclosure for the secondary actions; the actions keep their own
// handlers, so picking one also closes the menu.
function wireMoreMenu() {
  elements.moreButton.addEventListener("click", () => {
    setMoreMenuOpen(elements.toolbarMore.dataset.open !== "true");
  });
  document.getElementById("more-menu").addEventListener("click", (event) => {
    if (event.target instanceof Element && event.target.closest("button")) {
      setMoreMenuOpen(false);
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!elements.toolbarMore.contains(event.target)) {
      setMoreMenuOpen(false);
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && elements.toolbarMore.dataset.open === "true") {
      setMoreMenuOpen(false);
      elements.moreButton.focus();
    }
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
  requestAutosavePreference();
  try {
    await loadSummary();
  } catch (error) {
    elements.documentsList.innerHTML = "";
    const message = readableDocumentsError(error, "Documents could not be loaded.");
    elements.preview.innerHTML = '<div class="error-state">' + escapeHtml(message) + "</div>";
    showError(message);
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
  const article = document.createElement("article");
  article.className = "markdown-body";
  elements.sharePreview.replaceChildren(article);
  renderMarkdownInto(article, markdown || "");
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
  state.mode = "share";
  elements.shareShell.classList.remove("hidden");

  const params = new URLSearchParams(window.location.search);
  const requestedFile = (options && options.file) || params.get("file");

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
      if (DOCUMENTS_INSIDE_HOME) {
        // Inside Home a navigation would replace the Documents frame (and
        // drop its Home token), so the chosen file opens in place.
        link.setAttribute("role", "link");
        link.tabIndex = 0;
        const open = (event) => {
          if (event.type === "keydown" && event.key !== "Enter" && event.key !== " ") {
            return;
          }
          event.preventDefault();
          void loadShareBundle({ ...options, file });
        };
        link.addEventListener("click", open);
        link.addEventListener("keydown", open);
      } else {
        const linkParams = new URLSearchParams(window.location.search);
        linkParams.set("file", file);
        link.href = window.location.pathname + "?" + linkParams.toString();
      }
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
    elements.sharePreview.innerHTML = '<div class="error-state">' + escapeHtml(readableDocumentsError(error, "Shared document could not be loaded.")) + "</div>";
  }
}

async function loadShareMode() {
  await loadShareBundle({ basePath: "", sourceUri: "" });
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
    '<div class="empty-document"><h2>Documents is ready.</h2><p>Open it from Home to create or edit documents.</p></div>';
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
