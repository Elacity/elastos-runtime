const errorNode = document.querySelector(".system-error");
const handleForm = document.querySelector("#handle-form");
const handleInput = document.querySelector("#handle-input");
const handleSaveButton = document.querySelector("#handle-save");
const handleStatusNode = document.querySelector('[data-field="handle-status"]');
const storageNoteNode = document.querySelector('[data-field="storage-note"]');
const backgroundInput = document.querySelector("#background-input");
const backgroundResetButton = document.querySelector("#background-reset");
const backgroundStatusNode = document.querySelector('[data-field="background-status"]');
const backgroundPreview = document.querySelector("#background-preview");
const backgroundOverlayInput = document.querySelector("#background-overlay");
const backgroundOverlayRange = document.querySelector("#background-overlay-range");
const backgroundOverlayOpacityInput = document.querySelector("#background-overlay-opacity");
const backgroundOverlayOpacityValue = document.querySelector("#background-overlay-opacity-value");
const overlayStatusNode = document.querySelector('[data-field="overlay-status"]');
const homeToken = readQueryParam("home_token");
const DEFAULT_BACKGROUND_IMAGE_URL = "/apps/home/wallpaper.webp";
const BACKGROUND_IMAGE_MAX_BYTES = 5 * 1024 * 1024;
const BACKGROUND_IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);
const BACKGROUND_OVERLAY_OPACITY_DEFAULT = 0.55;
const BACKGROUND_OVERLAY_OPACITY_MAX = 0.8;

boot().catch((error) => {
  console.error("system boot failed", error);
  showError(error);
});

async function boot() {
  configureHandleEditor();
  configureAppearanceEditor();
  if (hasShellAccess()) {
    await fetchJson("/api/apps/home/runtime/ensure", { method: "POST" });
  }
  await refreshSystemSummary();
}

function hasShellAccess() {
  return homeToken.length > 0;
}

async function refreshSystemSummary() {
  const systemSummary = await fetchJson("/api/apps/system/summary", {
    headers: shellHeaders(),
  });
  renderSystemSummary(systemSummary);
}

async function fetchJson(url, init) {
  const response = await fetch(url, init);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const suffix = detail.trim() ? ` ${detail.trim()}` : ` ${response.statusText}`;
    throw new Error(`request failed: ${response.status}${suffix}`);
  }
  return response.json();
}

function renderSystemSummary(systemSummary) {
  const identity = systemSummary.identity || {};
  const appearance = systemSummary.appearance || {};
  const runtime = systemSummary.runtime || {};
  const storage = systemSummary.storage || {};

  setField("device-did", identity.device_did, "");
  setHandle(identity.handle);
  setAppearance(appearance);
  setRuntimeState(runtime);
  setStorageState(storage);
}

function setField(field, value, emptyText) {
  const hasValue = typeof value === "string" && value.trim().length > 0;
  for (const node of document.querySelectorAll(`[data-field="${field}"]`)) {
    node.textContent = hasValue ? value : emptyText;
    node.dataset.missing = hasValue ? "false" : "true";
    if (hasValue) {
      node.title = value;
      continue;
    }
    node.removeAttribute("title");
  }
}

function setTextFields(field, value) {
  for (const node of document.querySelectorAll(`[data-field="${field}"]`)) {
    node.textContent = value;
  }
}

function setHiddenFields(field, hidden) {
  for (const node of document.querySelectorAll(`[data-field="${field}"]`)) {
    node.hidden = hidden;
  }
}

function setHandle(value) {
  const handle = readText(value);
  if (handleInput && document.activeElement !== handleInput) {
    handleInput.value = handle;
  }
}

function configureHandleEditor() {
  if (!handleInput || !handleSaveButton) {
    return;
  }
  const editable = hasShellAccess();
  handleInput.disabled = !editable;
  handleSaveButton.disabled = !editable;
  if (editable && handleForm) {
    handleForm.addEventListener("submit", onHandleSubmit);
  }
}

function configureAppearanceEditor() {
  const editable = hasShellAccess();
  if (backgroundInput) {
    backgroundInput.disabled = !editable;
    if (editable) {
      backgroundInput.addEventListener("change", onBackgroundInputChange);
    }
  }
  if (backgroundResetButton) {
    backgroundResetButton.disabled = !editable;
    if (editable) {
      backgroundResetButton.addEventListener("click", onBackgroundReset);
    }
  }
  if (backgroundOverlayInput) {
    backgroundOverlayInput.disabled = !editable;
    if (editable) {
      backgroundOverlayInput.addEventListener("change", onBackgroundOverlayChange);
    }
  }
  if (backgroundOverlayOpacityInput) {
    backgroundOverlayOpacityInput.disabled = !editable;
    if (editable) {
      backgroundOverlayOpacityInput.addEventListener("input", () => {
        setOverlayOpacity(readOverlayOpacityInput());
      });
      backgroundOverlayOpacityInput.addEventListener("change", onBackgroundOverlayChange);
    }
  }
}

async function onHandleSubmit(event) {
  event.preventDefault();
  if (!handleInput || !handleSaveButton || !hasShellAccess()) {
    return;
  }
  const handle = handleInput.value.trim();
  clearHandleStatus();
  handleInput.disabled = true;
  handleSaveButton.disabled = true;
  try {
    const identity = await fetchJson("/api/apps/system/identity/handle", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify({ handle }),
    });
    setHandle(identity.handle);
    showHandleStatus("Saved.", "success");
  } catch (error) {
    showHandleStatus(String(error.message || error), "error");
  } finally {
    handleInput.disabled = false;
    handleSaveButton.disabled = false;
  }
}

async function onBackgroundInputChange() {
  if (!backgroundInput || !hasShellAccess()) {
    return;
  }
  const file = backgroundInput.files && backgroundInput.files[0] ? backgroundInput.files[0] : null;
  if (!file) {
    return;
  }
  clearBackgroundStatus();
  if (!BACKGROUND_IMAGE_TYPES.has(file.type)) {
    showBackgroundStatus("Choose a PNG, JPEG, WebP, or GIF image.", "error");
    backgroundInput.value = "";
    return;
  }
  if (file.size > BACKGROUND_IMAGE_MAX_BYTES) {
    showBackgroundStatus("Choose an image under 5 MB.", "error");
    backgroundInput.value = "";
    return;
  }
  setAppearanceControlsDisabled(true);
  try {
    const appearance = await fetchJson("/api/apps/system/appearance/background-image", {
      method: "POST",
      headers: {
        "content-type": file.type,
        "x-elastos-home-token": homeToken,
      },
      body: file,
    });
    setAppearance(appearance);
    showBackgroundStatus("Updated.", "success");
    notifyHomeAppearanceChanged();
  } catch (error) {
    showBackgroundStatus(String(error.message || error), "error");
  } finally {
    backgroundInput.value = "";
    setAppearanceControlsDisabled(false);
  }
}

async function onBackgroundReset() {
  if (!hasShellAccess()) {
    return;
  }
  clearBackgroundStatus();
  setAppearanceControlsDisabled(true);
  try {
    const appearance = await fetchJson("/api/apps/system/appearance/background-image", {
      method: "DELETE",
      headers: {
        "x-elastos-home-token": homeToken,
      },
    });
    setAppearance(appearance);
    showBackgroundStatus("Reset.", "success");
    notifyHomeAppearanceChanged();
  } catch (error) {
    showBackgroundStatus(String(error.message || error), "error");
  } finally {
    setAppearanceControlsDisabled(false);
  }
}

async function onBackgroundOverlayChange() {
  if (!backgroundOverlayInput || !hasShellAccess()) {
    return;
  }
  clearOverlayStatus();
  setOverlayRangeVisible(backgroundOverlayInput.checked);
  setAppearanceControlsDisabled(true);
  try {
    const appearance = await fetchJson("/api/apps/system/appearance/background-overlay", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify({
        enabled: backgroundOverlayInput.checked,
        opacity: readOverlayOpacityInput(),
      }),
    });
    setAppearance(appearance);
    showOverlayStatus("Saved.", "success");
    notifyHomeAppearanceChanged();
  } catch (error) {
    showOverlayStatus(String(error.message || error), "error");
  } finally {
    setAppearanceControlsDisabled(false);
  }
}

function setAppearance(appearance) {
  const imageUrl = readText(appearance && appearance.background_image_url);
  const overlayEnabled = appearance && appearance.background_overlay_enabled === true;
  const overlayOpacity = clampOverlayOpacity(Number(appearance && appearance.background_overlay_opacity));
  if (backgroundPreview) {
    backgroundPreview.style.backgroundImage = `url("${imageUrl || DEFAULT_BACKGROUND_IMAGE_URL}")`;
    backgroundPreview.dataset.empty = imageUrl ? "false" : "true";
  }
  if (backgroundResetButton) {
    backgroundResetButton.disabled = !hasShellAccess() || imageUrl.length === 0;
  }
  if (backgroundOverlayInput && document.activeElement !== backgroundOverlayInput) {
    backgroundOverlayInput.checked = overlayEnabled;
  }
  setOverlayRangeVisible(overlayEnabled);
  if (backgroundOverlayOpacityInput && document.activeElement !== backgroundOverlayOpacityInput) {
    setOverlayOpacity(overlayOpacity);
  } else {
    setOverlayOpacity(readOverlayOpacityInput());
  }
}

function setAppearanceControlsDisabled(disabled) {
  if (backgroundInput) {
    backgroundInput.disabled = disabled || !hasShellAccess();
  }
  if (backgroundResetButton) {
    backgroundResetButton.disabled = disabled || !hasShellAccess() || backgroundPreview?.dataset.empty === "true";
  }
  if (backgroundOverlayInput) {
    backgroundOverlayInput.disabled = disabled || !hasShellAccess();
  }
  if (backgroundOverlayOpacityInput) {
    backgroundOverlayOpacityInput.disabled = disabled || !hasShellAccess();
  }
}

function setOverlayRangeVisible(visible) {
  if (backgroundOverlayRange) {
    backgroundOverlayRange.hidden = !visible;
  }
}

function clampOverlayOpacity(value) {
  if (!Number.isFinite(value)) {
    return BACKGROUND_OVERLAY_OPACITY_DEFAULT;
  }
  return Math.min(BACKGROUND_OVERLAY_OPACITY_MAX, Math.max(0, value));
}

function readOverlayOpacityInput() {
  if (!backgroundOverlayOpacityInput) {
    return BACKGROUND_OVERLAY_OPACITY_DEFAULT;
  }
  return clampOverlayOpacity(Number(backgroundOverlayOpacityInput.value) / 100);
}

function setOverlayOpacity(opacity) {
  const clamped = clampOverlayOpacity(opacity);
  const percent = Math.round(clamped * 100);
  if (backgroundOverlayOpacityInput) {
    backgroundOverlayOpacityInput.value = String(percent);
  }
  if (backgroundOverlayOpacityValue) {
    backgroundOverlayOpacityValue.textContent = `${percent}%`;
  }
}

function showBackgroundStatus(message, tone) {
  if (!backgroundStatusNode) {
    return;
  }
  backgroundStatusNode.hidden = false;
  backgroundStatusNode.dataset.tone = tone;
  backgroundStatusNode.textContent = message;
}

function clearBackgroundStatus() {
  if (!backgroundStatusNode) {
    return;
  }
  backgroundStatusNode.hidden = true;
  backgroundStatusNode.textContent = "";
  backgroundStatusNode.dataset.tone = "";
}

function showOverlayStatus(message, tone) {
  if (!overlayStatusNode) {
    return;
  }
  overlayStatusNode.hidden = false;
  overlayStatusNode.dataset.tone = tone;
  overlayStatusNode.textContent = message;
}

function clearOverlayStatus() {
  if (!overlayStatusNode) {
    return;
  }
  overlayStatusNode.hidden = true;
  overlayStatusNode.textContent = "";
  overlayStatusNode.dataset.tone = "";
}

function notifyHomeAppearanceChanged() {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage({
      type: "home:refresh-summary",
      homeToken,
    }, window.location.origin);
  }
}

function showHandleStatus(message, tone) {
  if (!handleStatusNode) {
    return;
  }
  handleStatusNode.hidden = false;
  handleStatusNode.dataset.tone = tone;
  handleStatusNode.textContent = message;
}

function clearHandleStatus() {
  if (!handleStatusNode) {
    return;
  }
  handleStatusNode.hidden = true;
  handleStatusNode.textContent = "";
  handleStatusNode.dataset.tone = "";
}

function setRuntimeState(runtime) {
  const version = readText(runtime && runtime.version);
  setTextFields("runtime-status", version);
  setHiddenFields("runtime-status", version.length === 0);
}

function setStorageState(storage) {
  const available = Boolean(storage && storage.available);
  const documentsCount = Number(storage && storage.documents_count ? storage.documents_count : 0);
  const draftsCount = Number(storage && storage.drafts_count ? storage.drafts_count : 0);
  const publishedCount = Number(storage && storage.published_count ? storage.published_count : 0);
  const status = available
    ? `${documentsCount} ${documentsCount === 1 ? "document" : "documents"}`
    : "";
  setTextFields("storage-status", status);
  setHiddenFields("storage-status", status.length === 0);

  if (!storageNoteNode) {
    return;
  }
  if (!available) {
    const note = readText(storage && storage.note);
    storageNoteNode.hidden = note.length === 0;
    storageNoteNode.textContent = note;
    return;
  }

  const parts = [`${draftsCount} ${draftsCount === 1 ? "draft" : "drafts"}`, `${publishedCount} published`];
  storageNoteNode.hidden = parts.length === 0;
  storageNoteNode.textContent = parts.join(" · ");
}

function readQueryParam(key) {
  const url = new URL(window.location.href);
  return (url.searchParams.get(key) || "").trim();
}

function shellHeaders(extra) {
  return Object.assign(
    homeToken.length > 0 ? { "x-elastos-home-token": homeToken } : {},
    extra || {},
  );
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function showError(error) {
  if (!errorNode) {
    return;
  }
  errorNode.hidden = false;
  errorNode.textContent = String(error.message || error);
}
