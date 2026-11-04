import { escapeHtml } from "./model.js";

// The file name the Chat post states, for the download only: the same rule as
// Home's sharedFileName (home-external-link.js, which names the Runtime limit
// it follows): no path separators, control characters or leading dots, at
// most 200 UTF-8 bytes, cut before the extension.
export function sharedFileName(value) {
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

// Home opens this view only after the shared file is in the local store.
export function sharedFileMarkup(cid) {
  return `<div class="dialog-card"><div><p class="eyebrow">Shared file</p><h2>File is ready</h2>
    <p style="overflow-wrap:anywhere">${escapeHtml(`elastos://${cid}`)}</p>
    <p>This file is on your Home.</p>
    <div class="button-row"><button class="btn btn-primary" type="button" data-shared-download>Download file</button>
    <button class="btn" type="button" data-dialog-close>Close</button></div>
    </div></div>`;
}
