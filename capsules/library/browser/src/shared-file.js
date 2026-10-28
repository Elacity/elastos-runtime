import { escapeHtml } from "./model.js";

// The file name the Chat post states, for the download only: the same rule as
// Home's sharedFileName (home-external-link.js), no path separators, control
// characters or leading dots, at most 120 characters.
export function sharedFileName(value) {
  if (typeof value !== "string" || /[/\\]/.test(value)) return "";
  const cleaned = value.replace(/[\u0000-\u001f\u007f-\u009f]/g, "").replace(/^[.\s]+/, "");
  return [...cleaned].slice(0, 120).join("").trimEnd();
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
