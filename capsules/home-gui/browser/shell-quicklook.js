/* Quick Look — Space on a selected desktop item opens a frameless preview
 * card (glyph, name, type, Open). Esc / click-outside / Space again closes.
 *
 * Deliberately NO byte-level preview: raw object bytes are gated behind
 * Library-scoped launch tokens (header-only, no cookies) and the shell holds
 * no such token. Widening that route to Home would be ambient authority;
 * "Open" routes through the same canonical openFileObject/openTarget path
 * the desktop already uses, so the viewer capsule does the real rendering.
 */

import {
  shellState,
  mountGlyph,
  targetById,
  desktopObjectByEntryId,
} from "./shell-core.js?v=home-20260804ax";
import { openFileObject } from "./shell-surface.js?v=home-20260804ax";
import { openTarget } from "./shell-windows.js?v=home-20260804ax";
import {
  closeOtherShellPopovers,
  registerShellPopover,
  setOverlayOpen,
} from "./shell-popovers.js?v=home-20260804ax";

/* Bound by bindQuickLook() once the lazy GUI template is in the DOM. */
let panel = null;
let stage = null;
let titleNode = null;
let metaNode = null;
let openButton = null;
let closeButton = null;

let open = false;

function selectedEntry() {
  const entryId = shellState.selectedDesktopTargetId;
  if (!entryId) {
    return null;
  }
  const app = targetById(shellState.currentSummary, entryId);
  if (app) {
    return { kind: "app", entryId, app };
  }
  const object = desktopObjectByEntryId(shellState.currentSummary, entryId);
  if (object) {
    return { kind: "object", entryId, object };
  }
  return null;
}

function renderQuickLook(entry) {
  stage.replaceChildren();
  if (entry.kind === "app") {
    const glyph = document.createElement("span");
    glyph.className = "quick-look-glyph app-glyph";
    mountGlyph(glyph, entry.app.target);
    stage.appendChild(glyph);
    titleNode.textContent = entry.app.title || entry.app.target;
    metaNode.textContent = "Application";
    return;
  }
  const object = entry.object;
  titleNode.textContent = object.name || "Item";
  metaNode.textContent = object.mime || (object.kind === "directory" ? "Folder" : "File");
  const glyph = document.createElement("span");
  glyph.className = "quick-look-glyph app-glyph";
  const glyphId =
    object.kind === "directory"
      ? "file-folder"
      : object.metadata?.system_kind === "trash"
        ? object.metadata?.empty === false
          ? "trash-full"
          : "trash"
        : "documents";
  mountGlyph(glyph, glyphId);
  stage.appendChild(glyph);
}

function openEntry() {
  const entry = selectedEntry();
  if (!entry) {
    return;
  }
  hideQuickLook();
  if (entry.kind === "app") {
    openTarget(entry.app.target);
    return;
  }
  openFileObject(entry.object);
}

export function showQuickLook() {
  if (!panel || !selectedEntry()) {
    return false;
  }
  const entry = selectedEntry();
  open = true;
  closeOtherShellPopovers("quick-look");
  renderQuickLook(entry);
  setOverlayOpen(panel, true, {
    invoker: document.activeElement,
    focusEl: closeButton || panel,
  });
  return true;
}

export function hideQuickLook() {
  if (!panel || !open) {
    return;
  }
  open = false;
  setOverlayOpen(panel, false);
  stage?.replaceChildren();
}

export function toggleQuickLook() {
  if (open) {
    hideQuickLook();
    return true;
  }
  return showQuickLook();
}

export function isQuickLookOpen() {
  return open;
}

/* Called by the home-gui facade once ensureHomeGuiDom() has instantiated the
   lazy GUI template — these nodes do not exist at module-evaluation time. */
export function bindQuickLook() {
  if (panel) {
    return;
  }
  panel = document.querySelector("#quick-look");
  stage = document.querySelector("#quick-look-stage");
  titleNode = document.querySelector("#quick-look-title");
  metaNode = document.querySelector("#quick-look-meta");
  openButton = document.querySelector("#quick-look-open");
  closeButton = document.querySelector("#quick-look-close");
  if (!panel) {
    return;
  }
  registerShellPopover("quick-look", () => hideQuickLook());
  closeButton?.addEventListener("click", () => hideQuickLook());
  openButton?.addEventListener("click", () => openEntry());
  panel.addEventListener("click", (event) => {
    if (event.target === panel) {
      hideQuickLook();
    }
  });
}
