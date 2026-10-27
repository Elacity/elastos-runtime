import { escapeHtml } from "./model.js";

// Home opens this view only after the shared file is in the local store.
export function sharedFileMarkup(cid, fileName) {
  return `<div class="dialog-card"><div><p class="eyebrow">Shared file</p><h2>File is ready</h2>
    <p style="overflow-wrap:anywhere">${escapeHtml(`elastos://${cid}`)}</p>
    <p>This file is on your Home.</p>
    <div class="button-row"><a class="btn btn-primary" href="/content/${encodeURIComponent(cid)}" download="${escapeHtml(fileName || cid)}">Download file</a>
    <button class="btn" type="button" data-dialog-close>Close</button></div>
    </div></div>`;
}
