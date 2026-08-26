#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const source = readFileSync(resolve("capsules/documents/browser/index.html"), "utf8");

const announceMatch = source.match(/function announceHomeChrome\(\) \{([\s\S]*?)\n\}/);
const syncManifestMatch = source.match(/function syncHomeMenuManifest\(\) \{([\s\S]*?)\n\}/);

assert(
  source.includes('<script src="./elastos-theme.js"></script>') &&
    source.includes('<link rel="stylesheet" href="./elastos-ui.css">'),
  "Documents must load the canonical shared theme and token sheet.",
);
assert(
  source.includes('id="mode-write" class="workspace-tab"') &&
    source.includes('aria-label="Write — edit markdown"') &&
    source.includes('title="Write — edit markdown"') &&
    source.includes('aria-label="Split — edit and preview"') &&
    source.includes('aria-label="Read — preview only"') &&
    source.includes('aria-pressed="true"'),
  "Documents view controls must use donor icon-segment markup with exact labels and pressed state.",
);
assert(
  source.includes('id="copy-published-link" class="action-secondary action-icon-button copy-link-button hidden"') &&
    !source.includes('<span>Copy link</span>'),
  "Documents Copy Published Link must stay an icon button.",
);
assert(
  source.includes('id="status-row" class="status-row hidden"') &&
    source.includes('function clearStatus()') &&
    source.includes('elements.statusRow.classList.add("hidden");') &&
    source.includes('workspaceView: "write"'),
  "Documents must hide idle status and default to Write when no authoritative preference exists.",
);
assert(
  !!announceMatch &&
    !!syncManifestMatch &&
    announceMatch[1].includes('window.top.postMessage({ type: "home:app-ready", homeToken: state.homeToken }, homeOrigin);') &&
    announceMatch[1].includes("homeChromeReady = true;") &&
    announceMatch[1].includes("syncHomeMenuManifest();") &&
    syncManifestMatch[1].includes('if (!canAnnounceHomeChrome() || !homeChromeReady) {') &&
    syncManifestMatch[1].includes('if (signature === lastHomeMenuManifestSignature) {') &&
    syncManifestMatch[1].includes('type: "home:menu-manifest"'),
  "Documents must send Home ready once before a deduplicated menu manifest.",
);
assert(
  source.includes('title: "File"') &&
    source.includes('title: "View"') &&
    source.includes('cmd: "file-new-document"') &&
    source.includes('cmd: "file-save"') &&
    source.includes('cmd: "file-save-as"') &&
    source.includes('cmd: "file-publish"') &&
    source.includes('cmd: "file-delete-document"') &&
    source.includes('cmd: "__close-window"') &&
    source.includes('cmd: "view-write"') &&
    source.includes('cmd: "view-split"') &&
    source.includes('cmd: "view-read"') &&
    source.includes('cmd: "view-find"') &&
    !source.includes('cmd: "file-refresh"'),
  "Documents Home menu must use the accepted File and View command set only.",
);
assert(
  source.includes('if (event.origin !== "null" || event.source !== window.parent) {') &&
    source.includes('data.type !== "elastos:menu-command"') &&
    source.includes('event.origin === homeOrigin && event.source === window.top'),
  "Documents must keep exact outbound top-window and inbound trusted-parent menu boundaries.",
);
assert(
  source.includes('contextMenuDocDid = docDid || "";') &&
    source.includes('{ action: "new", label: "New note" }') &&
    source.includes('{ action: "duplicate", label: "Duplicate" }') &&
    source.includes('{ action: "publish", label: "Publish" }') &&
    source.includes('{ action: "unpublish", label: "Unpublish" }') &&
    source.includes('{ action: "delete", label: "Delete", danger: true }') &&
    !source.includes("set_pin") &&
    !source.includes("Pin") &&
    !source.includes('data-menu-action="find"') &&
    !source.includes('data-menu-action="save"'),
  "Documents row context menus must stay item-aware and omit unsupported pinning and global actions.",
);
assert(
  source.includes('data-source-line="') &&
    source.includes('class="task-checkbox"') &&
    source.includes("function setTaskLineChecked(lineNumber, checked)") &&
    source.includes("function toggleTaskAtCursor()") &&
    source.includes('event.key === "Enter" && event.target === elements.editor') &&
    source.includes('elements.preview.addEventListener("change"') &&
    source.includes("scheduleAutosave();"),
  "Documents task-checkbox editing must stay on the existing dirty and autosave path.",
);
assert(
  source.includes("function computeSourceFindMatches(query)") &&
    source.includes("elements.editor.setSelectionRange(match.start, match.end, \"forward\");") &&
    source.includes("function applyPreviewFindHighlights()") &&
    source.includes('cmd: "view-find"') &&
    source.includes('event.key.toLowerCase() === "f"') &&
    source.includes('event.key.toLowerCase() === "g"'),
  "Documents find must stay source-driven, move the editor selection, and keep the current Home menu or keyboard flow.",
);
assert(
  source.includes('void saveCurrent({ quiet: true, autosave: true }).catch((error) => {') &&
    source.includes('reportSaveFailure(error, "Autosave failed.");') &&
    source.includes('currentSessionId: 0,') &&
    source.includes("function replaceCurrentDocument(document)") &&
    source.includes("function isCurrentDocumentTarget(target)") &&
    source.includes("if (!isCurrentDocumentTarget(saveTarget)) {") &&
    source.includes("if (state.dirty) {") &&
    source.includes("if (!quiet) {") &&
    source.includes('setStatus("Saving…");') &&
    source.includes("if (saveInFlight) {") &&
    source.includes("autosaveQueued = true;") &&
    source.includes("if (changedDuringSave) {") &&
    source.includes("setDirty(true);") &&
    source.includes("scheduleAutosave();"),
  "Documents autosave must stay caught, quiet, dirty-preserving, and single-queue.",
);
assert(
  source.includes('await homeClipboard.writeText(text, { purpose: "resource.uri" });') &&
    !source.includes("navigator.clipboard") &&
    !source.includes("localStorage") &&
    !source.includes("sessionStorage") &&
    !source.includes("IndexedDB"),
  "Documents must keep trusted Home Clipboard only and avoid browser storage authority.",
);

console.log("documents-product-behavior-smoke: OK");
