import { escapeHtml } from "./model.js";

// Home opens this view only after the shared file is in the local store.
export function sharedFileMarkup(cid) {
  return `<div class="dialog-card"><div><p class="eyebrow">Shared file</p><h2>File is ready</h2>
    <p style="overflow-wrap:anywhere">${escapeHtml(`elastos://${cid}`)}</p>
    <p>This file is on your Home.</p>
    <div class="button-row"><button class="btn btn-primary" type="button" data-shared-download>Download file</button>
    <button class="btn" type="button" data-dialog-close>Close</button></div>
    </div></div>`;
}
