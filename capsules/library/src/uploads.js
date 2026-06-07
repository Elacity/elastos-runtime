import { escapeHtml } from "./model.js";

export function createLibraryUploads({ container, perf, state }) {
  let uploadRenderFrame = 0;

  function cancelUploadRender() {
    if (!uploadRenderFrame) return;
    window.cancelAnimationFrame(uploadRenderFrame);
    uploadRenderFrame = 0;
  }

  function scheduleUploadRender() {
    if (uploadRenderFrame) return;
    perf.uploadRenderScheduledCount += 1;
    uploadRenderFrame = window.requestAnimationFrame(() => {
      uploadRenderFrame = 0;
      renderUploads();
    });
  }

  function renderUploads() {
    cancelUploadRender();
    perf.uploadRenderCount += 1;
    if (!state.uploads.length) {
      container.classList.add("hidden");
      container.innerHTML = "";
      return;
    }
    container.classList.remove("hidden");
    container.innerHTML = state.uploads.map((upload) => `
      <div class="upload-row">
        <div>
          <div class="upload-name">${escapeHtml(upload.name)}</div>
          <div>${escapeHtml(upload.status)}</div>
        </div>
        <div class="upload-meter" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(upload.progress)}">
          <div class="upload-fill" style="--progress: ${Math.max(0, Math.min(100, upload.progress))}%"></div>
        </div>
      </div>
    `).join("");
  }

  function setUploadProgress(id, patch) {
    const upload = state.uploads.find((entry) => entry.id === id);
    if (!upload) return;
    Object.assign(upload, patch);
    scheduleUploadRender();
  }

  return {
    renderUploads,
    setUploadProgress,
  };
}
