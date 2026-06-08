import {
  base64ToBlob,
  escapeHtml,
  formatBytes,
  previewKind,
  shortUri,
} from "./model.js";

const PREVIEW_MAX_BYTES = 8 * 1024 * 1024;
const TEXT_PREVIEW_MAX_BYTES = 1 * 1024 * 1024;

export function createLibraryPreview({
  dialog,
  providerApi,
  setStatus,
  showProperties,
  state,
}) {
  function revokePreviewUrl() {
    if (state.previewUrl) {
      URL.revokeObjectURL(state.previewUrl);
      state.previewUrl = "";
    }
  }

  async function previewObject(object) {
    const kind = previewKind(object);
    if (!kind) {
      showProperties(object);
      return;
    }
    const maxBytes = kind === "text" ? TEXT_PREVIEW_MAX_BYTES : PREVIEW_MAX_BYTES;
    if (Number(object.size || 0) > maxBytes) {
      setStatus(`Preview for ${object.name} is too large. Use Open or Download.`);
      showProperties(object);
      return;
    }
    setStatus(`Loading preview for ${object.name}...`);
    const data = await providerApi("read", { uri: object.uri });
    const blob = base64ToBlob(data.data, data.object?.mime || object.mime);
    revokePreviewUrl();
    let previewMarkup = "";
    if (kind === "text") {
      const text = await blob.text();
      previewMarkup = `<pre class="preview-text">${escapeHtml(text)}</pre>`;
    } else {
      state.previewUrl = URL.createObjectURL(blob);
      const url = escapeHtml(state.previewUrl);
      if (kind === "image") {
        previewMarkup = `<img src="${url}" alt="${escapeHtml(object.name)}">`;
      } else if (kind === "video") {
        previewMarkup = `<video src="${url}" controls playsinline></video>`;
      } else if (kind === "audio") {
        previewMarkup = `<audio src="${url}" controls></audio>`;
      } else if (kind === "pdf") {
        previewMarkup = `<iframe src="${url}" title="${escapeHtml(object.name)} preview"></iframe>`;
      }
    }
    dialog.innerHTML = `
      <div class="dialog-card">
        <div>
          <p class="eyebrow">Preview</p>
          <h2>${escapeHtml(object.name)}</h2>
        </div>
        <div class="preview-frame">${previewMarkup}</div>
        <div class="details">
          <div><strong>Type</strong><br>${escapeHtml(object.mime || "application/octet-stream")}</div>
          <div><strong>Size</strong><br>${escapeHtml(formatBytes(object.size))}</div>
          <div><strong>Source</strong><br>${escapeHtml(shortUri(object.uri))}</div>
        </div>
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri;
    dialog.classList.remove("hidden");
    setStatus(`Previewing ${object.name}.`);
  }

  return {
    previewObject,
    revokePreviewUrl,
  };
}
