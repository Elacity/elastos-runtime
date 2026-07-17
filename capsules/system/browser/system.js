import {
  gatePreviewAuditView,
  inspectActionRequestValidation,
  provenanceView,
} from "./esp-projections.mjs";

const errorNode = document.querySelector(".system-error");
const backgroundInput = document.querySelector("#background-input");
const backgroundResetButton = document.querySelector("#background-reset");
const backgroundStatusNode = document.querySelector('[data-field="background-status"]');
const backgroundPreview = document.querySelector("#background-preview");
const backgroundOverlayInput = document.querySelector("#background-overlay");
const backgroundOverlayRange = document.querySelector("#background-overlay-range");
const backgroundOverlayOpacityInput = document.querySelector("#background-overlay-opacity");
const backgroundOverlayOpacityValue = document.querySelector("#background-overlay-opacity-value");
const overlayStatusNode = document.querySelector('[data-field="overlay-status"]');
const guestRegistrationInput = document.querySelector("#guest-registration");
const guestRegistrationStatusNode = document.querySelector('[data-field="guest-registration-status"]');
const passkeyStatusNode = document.querySelector('[data-field="passkey-status"]');
const accountListNode = document.querySelector("#account-list");
const recoveryDownloadButton = document.querySelector("#recovery-download");
const recoveryImportInput = document.querySelector("#recovery-import");
const recoveryPasswordInput = document.querySelector("#recovery-password");
const recoveryStatusNode = document.querySelector('[data-field="recovery-status"]');
const recoveryNoteNode = document.querySelector('[data-field="recovery-note"]');
const recoveryPendingNode = document.querySelector("#recovery-pending");
const recoveryPendingTextNode = document.querySelector('[data-field="recovery-pending-text"]');
const recoveryAttachButton = document.querySelector("#recovery-attach");
const recoveryCancelButton = document.querySelector("#recovery-cancel");
const chainTableNode = document.querySelector("#chain-table");
const activeShellOptions = document.querySelector("#active-shell-options");
const activeShellStatusNode = document.querySelector('[data-field="active-shell-status"]');
const capsuleCatalogNode = document.querySelector("#capsule-catalog");
const capsuleCatalogStatusNode = document.querySelector("#capsule-catalog-status");
const capsuleCatalogRefreshButton = document.querySelector("#capsule-catalog-refresh");
const technicalDetailsNode = document.querySelector("#technical-details");
const technicalInspectListNode = document.querySelector("#technical-inspect-list");
const technicalInspectDetailNode = document.querySelector("#technical-inspect-detail");
const technicalInspectStatusNode = document.querySelector("#technical-inspect-status");
const technicalInspectRefreshButton = document.querySelector("#technical-inspect-refresh");
const frameHomeToken = readLaunchToken();
const homeParentOrigin = readQueryParam("home_origin");
const HOME_HOST_ID = "home";
const HOME_GUI_SHELL_ID = "home-gui";
if (frameHomeToken && homeParentOrigin && window.top !== window) {
  window.top.postMessage({ type: "home:app-ready", homeToken: frameHomeToken }, homeParentOrigin);
}
let apiHomeToken = frameHomeToken;
let chainNetworks = [];
let chainStatusById = new Map();
let chainLifecycleById = new Map();
let technicalInspectEntries = [];
let technicalSelectedId = "";
let registeredProviderSchemes = new Set();
let currentAccess = {};
let passkeyAuthorityActive = false;
let pendingRecoveryImport = null;
let activeShellName = "";
let activeShellBusy = false;
const DEFAULT_BACKGROUND_IMAGE_URL = "/apps/home-gui/wallpaper.webp";
const BACKGROUND_IMAGE_MAX_BYTES = 5 * 1024 * 1024;
const BACKGROUND_IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);
const BACKGROUND_OVERLAY_OPACITY_DEFAULT = 0.55;
const BACKGROUND_OVERLAY_OPACITY_MAX = 0.8;
const CHAIN_NAMESPACE_LABELS = new Map([
  ["bip122:000000000019d6689c085ae165831e93", "Bitcoin"],
  ["eip155:1", "Ethereum"],
  ["eip155:10", "Optimism"],
  ["eip155:20", "Elastos Smart Chain"],
  ["eip155:56", "BNB Chain"],
  ["eip155:137", "Polygon"],
  ["eip155:8453", "Base"],
  ["eip155:42161", "Arbitrum"],
  ["eip155:43114", "Avalanche"],
]);
const READABLE_CHAIN_KINDS = new Set([
  "evm_json_rpc",
  "mainchain_rest",
  "bitcoin_core_rpc",
  "bitcoin_rest",
]);
const CATALOG_GROUPS = [
  { role: "app", label: "Apps" },
  { role: "viewer", label: "Viewers" },
  { role: "content", label: "Content" },
  { role: "provider", label: "Background services" },
  { role: "shell", label: "Home views" },
];

boot().catch((error) => {
  console.error("system boot failed", error);
  showError(error);
});

async function boot() {
  if (!hasShellAccess()) {
    document.querySelector(".settings-container").hidden = true;
    document.querySelector("#system-locked").hidden = false;
    return;
  }
  configureSettingsTabs();
  configureAppearanceEditor();
  configureGuestAccess();
  configurePasskeyAccess();
  configureRecoveryAccess();
  configureChainAccess();
  configureActiveShell();
  configureCapsuleCatalog();
  configureTechnicalDetails();
  await refreshSystemSummary();
  await refreshActiveShell().catch((error) => showActiveShellStatus(String(error.message || error), "error"));
  await refreshAccountList().catch(() => {});
  await refreshRecoveryStatus();
  await refreshChainNetworks();
  await refreshCapsuleCatalog().catch((error) => {
    console.error("catalog refresh failed", error);
    showCapsuleCatalogStatus("Apps and services could not be loaded.", "error");
  });
}

function configureSettingsTabs() {
  const settingsShell = document.querySelector(".settings");
  if (!settingsShell) {
    return;
  }
  settingsShell.addEventListener("click", (event) => {
    const item = event.target.closest(".settings-sidebar-item");
    if (!item || !settingsShell.contains(item)) {
      return;
    }
    activateSettingsTab(item.dataset.settings);
    document.querySelector(".settings-sidebar")?.classList.remove("active");
  });
  document.querySelector(".sidebar-toggle")?.addEventListener("click", () => {
    document.querySelector(".settings-sidebar")?.classList.toggle("active");
  });
}

function activateSettingsTab(settings) {
  const tab = readText(settings);
  if (!tab) {
    return;
  }
  for (const item of document.querySelectorAll(".settings-sidebar-item")) {
    item.classList.toggle("active", item.dataset.settings === tab);
  }
  const container = document.querySelector(".settings-content-container");
  for (const content of document.querySelectorAll(".settings-content")) {
    content.classList.toggle("active", content.dataset.settings === tab);
  }
  if (container) {
    container.scrollTop = 0;
  }
}

function hasShellAccess() {
  return apiHomeToken.length > 0;
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
  const authority = systemSummary.authority || {};
  const access = systemSummary.access || {};
  const runtime = systemSummary.runtime || {};
  const source = systemSummary.source || {};

  setField("device-did", shortDid(identity.device_did), "", identity.device_did);
  setAccessPolicy(access);
  setPasskeyAuthority(authority);
  setAppearance(appearance);
  setRuntimeState(runtime);
  setSourceState(source);
}

function setField(field, value, emptyText, titleValue) {
  const hasValue = typeof value === "string" && value.trim().length > 0;
  for (const node of document.querySelectorAll(`[data-field="${field}"]`)) {
    node.textContent = hasValue ? value : emptyText;
    node.dataset.missing = hasValue ? "false" : "true";
    if (hasValue) {
      node.title = readText(titleValue) || value;
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

function configureGuestAccess() {
  if (!guestRegistrationInput) {
    return;
  }
  guestRegistrationInput.disabled = !hasShellAccess();
  if (hasShellAccess()) {
    guestRegistrationInput.addEventListener("change", onGuestRegistrationChange);
  }
}

async function onGuestRegistrationChange() {
  if (!guestRegistrationInput || !hasShellAccess()) {
    return;
  }
  clearGuestRegistrationStatus();
  guestRegistrationInput.disabled = true;
  try {
    const access = await fetchJson("/api/apps/system/access/guest-registration", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ enabled: guestRegistrationInput.checked }),
    });
    setAccessPolicy(access);
    showGuestRegistrationStatus(access.guest_registration_enabled ? "Guest creation on." : "Guest creation off.", "success");
  } catch (error) {
    guestRegistrationInput.checked = currentAccess.guest_registration_enabled === true;
    showGuestRegistrationStatus(String(error.message || error), "error");
  } finally {
    setGuestRegistrationControlState();
  }
}

function configurePasskeyAccess() {
  if (!window.PublicKeyCredential) {
    setPasskeyButtonsDisabled(true);
    showPasskeyStatus("Not supported", "muted");
    return;
  }
  refreshPasskeyStatus().catch(() => {
    showPasskeyStatus("Not set", "muted");
  });
  if (accountListNode) {
    accountListNode.addEventListener("click", onAccountListClick);
  }
}

function configureRecoveryAccess() {
  if (recoveryDownloadButton) {
    recoveryDownloadButton.disabled = !hasShellAccess();
  }
  if (hasShellAccess()) {
    if (recoveryDownloadButton) {
      recoveryDownloadButton.addEventListener("click", onRecoveryDownload);
    }
    if (recoveryImportInput) {
      recoveryImportInput.addEventListener("change", onRecoveryImport);
    }
    if (recoveryAttachButton) {
      recoveryAttachButton.addEventListener("click", onRecoveryAttach);
    }
    if (recoveryCancelButton) {
      recoveryCancelButton.addEventListener("click", clearRecoveryPending);
    }
  }
}

function configureChainAccess() {
  if (chainTableNode) {
    chainTableNode.addEventListener("click", onChainRowClick);
    chainTableNode.addEventListener("keydown", onChainRowKeydown);
  }
}

function configureActiveShell() {
  activeShellOptions?.addEventListener("click", (event) => {
    const button = event.target.closest("[data-shell-name]");
    if (!button || !activeShellOptions.contains(button) || activeShellBusy) {
      return;
    }
    const next = shellName(button.dataset.shellName);
    if (!next || next === activeShellName) {
      return;
    }
    applyActiveShell(next).catch((error) => {
      setActiveShellBusy(false);
      showActiveShellStatus(String(error.message || error), "error");
    });
  });
}

function configureCapsuleCatalog() {
  if (!capsuleCatalogRefreshButton) {
    return;
  }
  capsuleCatalogRefreshButton.disabled = !hasShellAccess();
  if (hasShellAccess()) {
    capsuleCatalogRefreshButton.addEventListener("click", () => {
      refreshCapsuleCatalog().catch((error) => {
        console.error("catalog refresh failed", error);
        showCapsuleCatalogStatus("Apps and services could not be loaded.", "error");
      });
    });
  }
}

function configureTechnicalDetails() {
  if (!technicalDetailsNode) {
    return;
  }
  technicalInspectRefreshButton.disabled = !hasShellAccess();
  technicalDetailsNode.addEventListener("toggle", () => {
    if (technicalDetailsNode.open && technicalInspectEntries.length === 0 && hasShellAccess()) {
      refreshTechnicalDetails().catch(onTechnicalDetailsError);
    }
  });
  technicalInspectRefreshButton?.addEventListener("click", () => {
    refreshTechnicalDetails().catch(onTechnicalDetailsError);
  });
  technicalInspectListNode?.addEventListener("click", (event) => {
    const row = event.target.closest("[data-technical-inspect-id]");
    if (!row || !hasShellAccess()) {
      return;
    }
    showTechnicalObject(readText(row.dataset.technicalInspectId)).catch(onTechnicalDetailsError);
  });
  technicalInspectDetailNode?.addEventListener("change", onTechnicalOperationChange);
  technicalInspectDetailNode?.addEventListener("click", onTechnicalDetailClick);
}

function onTechnicalDetailsError(error) {
  console.error("technical details failed", error);
  showTechnicalInspectStatus("Technical details could not be loaded.", "error");
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
        "x-elastos-home-token": apiHomeToken,
      },
      body: file,
    });
    setAppearance(appearance);
    showBackgroundStatus("Updated.", "success");
    notifyHomeSummaryChanged();
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
        "x-elastos-home-token": apiHomeToken,
      },
    });
    setAppearance(appearance);
    showBackgroundStatus("Reset.", "success");
    notifyHomeSummaryChanged();
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
        "x-elastos-home-token": apiHomeToken,
      },
      body: JSON.stringify({
        enabled: backgroundOverlayInput.checked,
        opacity: readOverlayOpacityInput(),
      }),
    });
    setAppearance(appearance);
    showOverlayStatus("Saved.", "success");
    notifyHomeSummaryChanged();
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
  backgroundStatusNode.textContent = tone === "error"
    ? publicSystemError(message, "Background could not be updated.")
    : message;
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
  overlayStatusNode.textContent = tone === "error"
    ? publicSystemError(message, "Background contrast could not be updated.")
    : message;
}

function clearOverlayStatus() {
  if (!overlayStatusNode) {
    return;
  }
  overlayStatusNode.hidden = true;
  overlayStatusNode.textContent = "";
  overlayStatusNode.dataset.tone = "";
}

function openCapsuleTarget(target) {
  const id = readText(target);
  if (!id || !homeParentOrigin || !window.top || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:open-target",
    target: id,
    homeToken: apiHomeToken,
  }, homeParentOrigin);
}

async function refreshActiveShell() {
  if (!activeShellOptions || !hasShellAccess()) {
    return;
  }
  showActiveShellStatus("Loading", "muted");
  const summary = await fetchJson("/api/apps/home/active-shell", {
    headers: shellHeaders(),
  });
  renderActiveShell(summary);
  showActiveShellStatus("", "muted");
}

function renderActiveShell(summary) {
  if (!activeShellOptions) {
    return;
  }
  const active = shellName(readText(summary?.active) || "home-gui");
  activeShellName = active;
  const candidates = Array.isArray(summary?.candidates) ? summary.candidates : [];
  activeShellOptions.replaceChildren();
  for (const candidate of candidates) {
    const name = shellName(readText(candidate.name));
    if (!name || name === HOME_HOST_ID) {
      continue;
    }
    activeShellOptions.append(createShellChoice(name, candidate, name === active));
  }
  if (activeShellOptions.children.length === 0) {
    activeShellOptions.append(createShellChoice(active, {}, true));
  }
  setActiveShellBusy(false);
}

function createShellChoice(name, candidate, current) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "shell-choice";
  button.dataset.shellName = name;
  button.dataset.current = current ? "true" : "false";
  button.setAttribute("aria-pressed", current ? "true" : "false");
  button.disabled = candidate?.launchable === false;

  const preview = document.createElement("span");
  preview.className = `shell-choice-preview shell-choice-preview-${shellPreviewKind(name)}`;
  preview.setAttribute("aria-hidden", "true");

  const copy = document.createElement("span");
  copy.className = "shell-choice-copy";
  const title = document.createElement("strong");
  title.textContent = formatShellLabel(name, readText(candidate?.title));
  const description = document.createElement("small");
  description.textContent = shellChoiceDescription(name, readText(candidate?.description));
  copy.append(title, description);

  const state = document.createElement("span");
  state.className = "shell-choice-state";
  state.textContent = current ? "Current" : "";
  button.append(preview, copy, state);
  return button;
}

function shellPreviewKind(name) {
  return name === "home-cli" ? "terminal" : "desktop";
}

function shellChoiceDescription(name, fallback = "") {
  if (name === "home-gui") {
    return "Apps, windows and taskbar";
  }
  if (name === "home-cli") {
    return "Full-screen terminal";
  }
  return fallback || "Home shell";
}

async function applyActiveShell(active) {
  active = shellName(active);
  if (!active) {
    return;
  }
  setActiveShellBusy(true);
  showActiveShellStatus("Switching...", "muted");
  const summary = await fetchJson("/api/apps/home/active-shell", {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ active }),
  });
  renderActiveShell(summary);
  notifyHomeActiveShellApplied(readText(summary?.active) || active);
  notifyHomeSummaryChanged();
  showActiveShellStatus("", "ok");
}

function setActiveShellBusy(busy) {
  activeShellBusy = busy;
  for (const button of activeShellOptions?.querySelectorAll("[data-shell-name]") || []) {
    button.disabled = busy;
  }
}

function notifyHomeSummaryChanged() {
  if (!homeParentOrigin || !window.top || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:refresh-summary",
    homeToken: apiHomeToken,
  }, homeParentOrigin);
}

function notifyHomeActiveShellApplied(active) {
  const activeShell = readText(active);
  if (!activeShell || !apiHomeToken || !homeParentOrigin || !window.top || window.top === window) {
    return;
  }
  window.top.postMessage({
    type: "home:active-shell-applied",
    activeShell,
    homeToken: apiHomeToken,
  }, homeParentOrigin);
}

function showActiveShellStatus(message, tone = "muted") {
  if (!activeShellStatusNode) {
    return;
  }
  const text = tone === "error"
    ? publicSystemError(message, "Home view could not be updated.")
    : readText(message);
  activeShellStatusNode.textContent = text;
  activeShellStatusNode.dataset.tone = tone;
  activeShellStatusNode.hidden = !text;
}

function formatShellLabel(name, title = "") {
  if (name === "home-gui") {
    return "Desktop";
  }
  if (name === "home-cli") {
    return "Terminal";
  }
  const label = readText(title);
  if (label) {
    return label;
  }
  return formatShellName(name);
}

function formatShellName(value) {
  const name = shellName(value);
  return name || "unknown";
}

function shellName(value) {
  return readText(value);
}

async function refreshTechnicalDetails() {
  if (!technicalInspectListNode || !hasShellAccess()) {
    return;
  }
  setTechnicalInspectBusy(true);
  showTechnicalInspectStatus("Loading", "muted");
  try {
    const result = await inspectProvider("capsules", {});
    technicalInspectEntries = (Array.isArray(result.capsules) ? result.capsules : [])
      .filter((entry) => entry && readText(entry.id))
      .sort((left, right) => {
        const kindOrder = technicalKindLabel(left).localeCompare(technicalKindLabel(right));
        return kindOrder || technicalDisplayName(left).localeCompare(technicalDisplayName(right));
      });
    registeredProviderSchemes = new Set(
      technicalInspectEntries
        .filter((entry) => readText(entry.kind) === "provider" && readText(entry.state) === "running")
        .map((entry) => readText(entry.id).replace(/^provider:/, ""))
        .filter(Boolean),
    );
    renderTechnicalInspectList(technicalInspectEntries);
    showTechnicalInspectStatus(`${technicalInspectEntries.length} objects`, "muted");
    const selected = technicalInspectEntries.find((entry) => readText(entry.id) === technicalSelectedId)
      || technicalInspectEntries.find((entry) => readText(entry.kind) === "capsule")
      || technicalInspectEntries[0];
    if (selected) {
      await showTechnicalObject(readText(selected.id));
    } else {
      technicalInspectDetailNode?.replaceChildren(technicalEmpty("No technical details are available."));
    }
  } finally {
    setTechnicalInspectBusy(false);
  }
}

function renderTechnicalInspectList(entries) {
  if (!technicalInspectListNode) {
    return;
  }
  technicalInspectListNode.replaceChildren();
  if (entries.length === 0) {
    technicalInspectListNode.append(technicalEmpty("No technical details are available."));
    return;
  }
  for (const entry of entries) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "technical-inspect-row";
    button.dataset.technicalInspectId = readText(entry.id);
    button.setAttribute("aria-label", `View technical details for ${technicalDisplayName(entry)}`);

    const body = document.createElement("span");
    body.className = "technical-inspect-row-body";
    const name = document.createElement("strong");
    name.textContent = technicalDisplayName(entry);
    const kind = document.createElement("small");
    kind.textContent = technicalKindLabel(entry);
    body.append(name, kind);

    const state = document.createElement("span");
    state.className = "technical-inspect-state";
    state.textContent = humanizeName(readText(entry.state));
    button.append(body, state);
    technicalInspectListNode.append(button);
  }
}

async function showTechnicalObject(id) {
  const inspectId = readText(id);
  if (!technicalInspectDetailNode || !inspectId) {
    return;
  }
  technicalSelectedId = inspectId;
  setTechnicalSelection(inspectId);
  technicalInspectDetailNode.replaceChildren(technicalEmpty("Loading"));
  const object = await inspectProvider("capsule", { id: inspectId });
  renderTechnicalObject(object);
}

function renderTechnicalObject(object) {
  if (!technicalInspectDetailNode) {
    return;
  }
  technicalInspectDetailNode.replaceChildren();
  const header = document.createElement("header");
  header.className = "technical-detail-header";
  const title = document.createElement("h3");
  title.textContent = technicalDisplayName(object);
  const meta = document.createElement("span");
  meta.textContent = technicalKindLabel(object);
  header.append(title, meta);
  technicalInspectDetailNode.append(header);

  const manifest = object && object.manifest && typeof object.manifest === "object" ? object.manifest : {};
  const provenance = provenanceView(object);
  const trust = object && object.trust_evidence && typeof object.trust_evidence === "object"
    ? object.trust_evidence
    : {};
  appendTechnicalSection(technicalInspectDetailNode, "Identity", [
    ["Identifier", readText(object && object.id)],
    ["Author", readText(provenance.author)],
    ["State", humanizeName(readText(object && object.state))],
    ["Type", humanizeName(readText(object && object.type))],
    ["Role", humanizeName(readText(manifest.role))],
    ["Version", readText(manifest.version)],
  ]);

  const permissionRows = technicalPermissionRows(object);
  appendTechnicalSection(technicalInspectDetailNode, "Permissions", permissionRows);

  const hasVerificationEvidence = Boolean(
    Object.prototype.hasOwnProperty.call(trust, "verified")
      || provenance.cid
      || provenance.signature_fingerprint
      || readText(trust.verified_by),
  );
  if (hasVerificationEvidence) {
    appendTechnicalSection(technicalInspectDetailNode, "Verification", [
      ["Status", trust.verified === true ? "Verified" : "Unverified"],
      ["Author", readText(provenance.author)],
      ["Content ID", readText(provenance.cid)],
      ["Signature", readText(provenance.signature_fingerprint)],
      ["Verified by", readText(trust.verified_by)],
    ]);
  }

  const operations = providerOperations(object);
  if (operations.length > 0) {
    technicalInspectDetailNode.append(technicalApprovalSection(object, operations));
  }
}

function technicalPermissionRows(object) {
  const rows = [];
  const required = Array.isArray(object && object.required_capabilities)
    ? object.required_capabilities.map(readText).filter(Boolean)
    : [];
  if (required.length > 0) {
    rows.push(["Required", joinWords(required)]);
  }
  const authority = object && object.authority && typeof object.authority === "object" ? object.authority : {};
  for (const capability of Array.isArray(authority.capabilities) ? authority.capabilities : []) {
    const resource = readText(capability && capability.resource);
    const actions = Array.isArray(capability && capability.actions)
      ? capability.actions.map(readText).filter(Boolean)
      : [];
    if (resource) {
      rows.push(["Allows", actions.length > 0 ? `${resource} — ${joinWords(actions)}` : resource]);
    }
  }
  const storage = Array.isArray(object && object.storage_namespaces)
    ? object.storage_namespaces.map(readText).filter(Boolean)
    : [];
  if (storage.length > 0) {
    rows.push(["Storage", joinWords(storage)]);
  }
  return rows;
}

function providerOperations(object) {
  if (readText(object.manifest && object.manifest.role) !== "provider") {
    return [];
  }
  const scheme = providerScheme(object);
  if (!scheme || !registeredProviderSchemes.has(scheme)) {
    return [];
  }
  const authority = object && object.authority && typeof object.authority === "object" ? object.authority : {};
  return [...new Set(
    (Array.isArray(authority.capabilities) ? authority.capabilities : [])
      .flatMap((capability) => Array.isArray(capability && capability.operations) ? capability.operations : [])
      .map(readText)
      .filter(Boolean),
  )].sort();
}

function providerScheme(object) {
  const provides = readText(object && object.manifest && object.manifest.provides);
  const match = provides.match(/^elastos:\/\/([^/]+)\//);
  if (match) {
    return match[1];
  }
  return readText(object && object.name).replace(/-(provider|adapter)$/, "");
}

function technicalApprovalSection(object, operations) {
  const section = document.createElement("section");
  section.className = "technical-section technical-approval";
  section.dataset.technicalInspectId = readText(object.id);
  const title = document.createElement("h3");
  title.className = "technical-section-title";
  title.textContent = "Approval";

  const controls = document.createElement("div");
  controls.className = "technical-approval-controls";
  const label = document.createElement("label");
  label.textContent = "Operation";
  const select = document.createElement("select");
  select.className = "pc2-input technical-operation";
  select.setAttribute("aria-label", "Provider operation");
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = "Choose an operation";
  placeholder.selected = true;
  select.append(placeholder);
  for (const operation of operations) {
    const option = document.createElement("option");
    option.value = operation;
    option.textContent = humanizeName(operation);
    select.append(option);
  }
  label.append(select);
  const previewButton = document.createElement("button");
  previewButton.type = "button";
  previewButton.className = "pc2-btn pc2-btn-secondary";
  previewButton.dataset.technicalPreview = "";
  previewButton.disabled = true;
  previewButton.textContent = "Preview";
  controls.append(label, previewButton);

  const preview = document.createElement("div");
  preview.className = "technical-approval-preview";
  preview.hidden = true;
  const requestButton = document.createElement("button");
  requestButton.type = "button";
  requestButton.className = "pc2-btn";
  requestButton.dataset.technicalRequest = "";
  requestButton.hidden = true;
  requestButton.textContent = "Request approval";
  const status = document.createElement("p");
  status.className = "system-status technical-approval-status";
  status.hidden = true;
  section.append(title, controls, preview, requestButton, status);
  return section;
}

function onTechnicalOperationChange(event) {
  const select = event.target.closest(".technical-operation");
  if (!select) {
    return;
  }
  const section = select.closest(".technical-approval");
  const previewButton = section?.querySelector("[data-technical-preview]");
  const preview = section?.querySelector(".technical-approval-preview");
  const requestButton = section?.querySelector("[data-technical-request]");
  if (previewButton) {
    previewButton.disabled = !readText(select.value);
  }
  if (preview) {
    preview.hidden = true;
    preview.replaceChildren();
  }
  if (requestButton) {
    requestButton.hidden = true;
  }
  showTechnicalApprovalStatus(section, "", "muted");
}

function onTechnicalDetailClick(event) {
  const previewButton = event.target.closest("[data-technical-preview]");
  if (previewButton) {
    previewTechnicalOperation(previewButton.closest(".technical-approval")).catch(onTechnicalDetailsError);
    return;
  }
  const requestButton = event.target.closest("[data-technical-request]");
  if (requestButton) {
    requestTechnicalApproval(requestButton.closest(".technical-approval")).catch(onTechnicalDetailsError);
  }
}

async function previewTechnicalOperation(section) {
  const id = readText(section && section.dataset.technicalInspectId);
  const operation = readText(section?.querySelector(".technical-operation")?.value);
  if (!id || !operation) {
    return;
  }
  showTechnicalApprovalStatus(section, "Loading", "muted");
  const result = await inspectProvider("plan", { id, operation });
  const view = gatePreviewAuditView(result);
  if (view.state !== "preview") {
    throw new Error("approval preview is not fail-closed");
  }
  const rows = [["Operation", humanizeName(operation)]];
  for (const capability of Array.isArray(result.capabilities) ? result.capabilities : []) {
    const resource = readText(capability && capability.resource);
    const actions = Array.isArray(capability && capability.actions)
      ? capability.actions.map(readText).filter(Boolean)
      : [];
    if (resource) {
      rows.push(["Permission", actions.length > 0 ? `${resource} — ${joinWords(actions)}` : resource]);
    }
  }
  const audit = Array.isArray(result.audit_events) ? result.audit_events.map(readText).filter(Boolean) : [];
  if (audit.length > 0) {
    rows.push(["Audit", joinWords(audit)]);
  }
  const preview = section.querySelector(".technical-approval-preview");
  preview.replaceChildren(technicalFactList(rows));
  preview.hidden = false;
  const requestButton = section.querySelector("[data-technical-request]");
  requestButton.dataset.technicalOperation = operation;
  requestButton.hidden = false;
  showTechnicalApprovalStatus(section, "", "muted");
}

async function requestTechnicalApproval(section) {
  const id = readText(section && section.dataset.technicalInspectId);
  const requestButton = section?.querySelector("[data-technical-request]");
  const operation = readText(requestButton && requestButton.dataset.technicalOperation);
  if (!id || !operation) {
    return;
  }
  requestButton.disabled = true;
  showTechnicalApprovalStatus(section, "Sending to Inbox", "muted");
  try {
    const result = await inspectProvider("request_act", { id, operation, request: {} });
    const validation = inspectActionRequestValidation(result, {});
    if (!validation.ok) {
      throw new Error("approval request did not include a valid preview and request binding");
    }
    requestButton.hidden = true;
    showTechnicalApprovalStatus(section, "Sent to Inbox for approval.", "ok");
    notifyHomeSummaryChanged();
  } finally {
    requestButton.disabled = false;
  }
}

async function inspectProvider(operation, body) {
  const response = await fetchJson(`/api/provider/inspect/${encodeURIComponent(operation)}`, {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body || {}),
  });
  if (response.status === "ok") {
    return response.data || {};
  }
  if (
    operation === "request_act"
      && response.schema === "elastos.inspect.action-request/v1"
      && response.status === "pending"
  ) {
    return response;
  }
  throw new Error(readText(response.message) || readText(response.code) || "inspection failed");
}

function appendTechnicalSection(parent, title, rows) {
  const values = rows.filter(([, value]) => readText(value));
  if (values.length === 0) {
    return;
  }
  const section = document.createElement("section");
  section.className = "technical-section";
  const heading = document.createElement("h3");
  heading.className = "technical-section-title";
  heading.textContent = title;
  section.append(heading, technicalFactList(values));
  parent.append(section);
}

function technicalFactList(rows) {
  const list = document.createElement("dl");
  list.className = "technical-facts";
  for (const [label, value] of rows) {
    const item = document.createElement("div");
    const key = document.createElement("dt");
    key.textContent = label;
    const text = document.createElement("dd");
    text.textContent = readText(value);
    item.append(key, text);
    list.append(item);
  }
  return list;
}

function technicalDisplayName(entry) {
  const name = readText(entry && entry.name);
  return humanizeName(name.replace(/-(provider|adapter)$/, "")) || "Object";
}

function technicalKindLabel(entry) {
  return readText(entry && entry.kind) === "provider" ? "Runtime service" : "Component";
}

function technicalEmpty(message) {
  const empty = document.createElement("div");
  empty.className = "technical-empty";
  empty.textContent = message;
  return empty;
}

function setTechnicalSelection(id) {
  for (const row of document.querySelectorAll("[data-technical-inspect-id]")) {
    row.classList.toggle("active", row.dataset.technicalInspectId === id);
  }
}

function setTechnicalInspectBusy(busy) {
  if (technicalInspectRefreshButton) {
    technicalInspectRefreshButton.disabled = busy || !hasShellAccess();
  }
}

function showTechnicalInspectStatus(message, tone) {
  if (!technicalInspectStatusNode) {
    return;
  }
  technicalInspectStatusNode.textContent = readText(message);
  technicalInspectStatusNode.dataset.tone = tone;
}

function showTechnicalApprovalStatus(section, message, tone) {
  const status = section?.querySelector(".technical-approval-status");
  if (!status) {
    return;
  }
  const text = readText(message);
  status.textContent = text;
  status.dataset.tone = tone;
  status.hidden = !text;
}

async function refreshCapsuleCatalog() {
  if (!capsuleCatalogNode || !hasShellAccess()) {
    return;
  }
  setCapsuleCatalogBusy(true);
  showCapsuleCatalogStatus("Loading", "muted");
  try {
    const [catalog, interfaces] = await Promise.all([
      fetchJson("/api/capsules/catalog", { headers: shellHeaders() }),
      fetchJson("/api/capsules/interfaces", { headers: shellHeaders() }),
    ]);
    const entries = (Array.isArray(catalog.capsules) ? catalog.capsules : [])
      .filter((entry) => entry && entry.installed !== false);
    const interfaceEntries = Array.isArray(interfaces.interfaces) ? interfaces.interfaces : [];
    renderCapsuleCatalog(entries, interfaceEntries);
    showCapsuleCatalogStatus(`${entries.length} available`, "muted");
  } finally {
    setCapsuleCatalogBusy(false);
  }
}

function renderCapsuleCatalog(entries, interfaceEntries) {
  if (!capsuleCatalogNode) {
    return;
  }
  capsuleCatalogNode.replaceChildren();
  if (entries.length === 0) {
    capsuleCatalogNode.append(catalogEmpty("No apps or services are installed."));
    return;
  }
  const entriesByName = new Map(entries.map((entry) => [readText(entry.name), entry]));
  const interfacesByCapsule = new Map();
  for (const entry of interfaceEntries) {
    const capsule = readText(entry && entry.capsule);
    if (!capsule) {
      continue;
    }
    const current = interfacesByCapsule.get(capsule) || [];
    current.push(entry);
    interfacesByCapsule.set(capsule, current);
  }
  for (const group of CATALOG_GROUPS) {
    const members = entries
      .filter((entry) => readText(entry.role) === group.role)
      .sort((left, right) => catalogTitle(left).localeCompare(catalogTitle(right)));
    if (members.length === 0) {
      continue;
    }
    const section = document.createElement("section");
    section.className = "catalog-group";
    const heading = document.createElement("div");
    heading.className = "catalog-group-heading";
    const title = document.createElement("h2");
    title.className = "catalog-group-title";
    title.textContent = group.label;
    const count = document.createElement("span");
    count.className = "catalog-group-count";
    count.textContent = String(members.length);
    heading.append(title, count);
    const list = document.createElement("div");
    list.className = "catalog-list";
    for (const entry of members) {
      list.append(catalogRow(entry, entriesByName, interfacesByCapsule.get(readText(entry.name)) || []));
    }
    section.append(heading, list);
    capsuleCatalogNode.append(section);
  }
}

function catalogRow(entry, entriesByName, interfaceEntries) {
  const row = document.createElement("article");
  row.className = "catalog-row";
  const body = document.createElement("div");
  body.className = "catalog-row-body";
  const title = document.createElement("h3");
  title.textContent = catalogTitle(entry);
  const summary = document.createElement("p");
  summary.className = "catalog-summary";
  summary.textContent = catalogSummary(entry);
  body.append(title, summary);

  const facts = catalogFacts(entry, entriesByName, interfaceEntries);
  if (facts.length > 0) {
    const list = document.createElement("dl");
    list.className = "catalog-facts";
    for (const [label, value] of facts) {
      const item = document.createElement("div");
      const key = document.createElement("dt");
      key.textContent = label;
      const text = document.createElement("dd");
      text.textContent = value;
      item.append(key, text);
      list.append(item);
    }
    body.append(list);
  }
  row.append(body);

  if (entry.launchable === true && readText(entry.launch_target)) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "pc2-btn pc2-btn-secondary catalog-open";
    button.textContent = "Open";
    button.addEventListener("click", () => openCapsuleTarget(readText(entry.launch_target)));
    row.append(button);
  }
  return row;
}

function catalogFacts(entry, entriesByName, interfaceEntries) {
  const facts = [];
  const role = readText(entry.role);
  if (role === "viewer") {
    const accepts = acceptedContent(entry, interfaceEntries);
    if (accepts.length > 0) {
      facts.push(["Accepts", joinWords(accepts)]);
    }
  }
  if (role === "content") {
    const viewer = readText(entry.viewer_title) || catalogTitle(entriesByName.get(readText(entry.viewer)));
    if (viewer) {
      facts.push(["Opens with", viewer]);
    }
  }
  const dependencies = (Array.isArray(entry.requires) ? entry.requires : [])
    .map((requirement) => {
      const name = readText(requirement && requirement.name);
      const dependency = entriesByName.get(name);
      return dependency ? catalogTitle(dependency) : "";
    })
    .filter(Boolean);
  if (dependencies.length > 0) {
    facts.push(["Needs", joinWords(dependencies)]);
  }
  const executable = executableActions(interfaceEntries)
    .filter((action) => action !== "Open");
  if (executable.length > 0) {
    facts.push(["Available", joinWords(executable)]);
  }
  return facts;
}

function acceptedContent(entry, interfaceEntries) {
  const content = (Array.isArray(entry.accepted_content) ? entry.accepted_content : [])
    .map((item) => readText(item && item.title) || humanizeName(readText(item && item.name)))
    .filter(Boolean);
  const extensions = new Set();
  for (const interfaceEntry of interfaceEntries) {
    const methods = interfaceEntry && interfaceEntry.interface && Array.isArray(interfaceEntry.interface.methods)
      ? interfaceEntry.interface.methods
      : [];
    for (const method of methods) {
      const accepts = method && method.input_schema && Array.isArray(method.input_schema.accepts)
        ? method.input_schema.accepts
        : [];
      for (const accepted of accepts) {
        if (readText(accepted && accepted.mode) === "unsupported_family_diagnostic") {
          continue;
        }
        for (const extension of Array.isArray(accepted && accepted.extensions) ? accepted.extensions : []) {
          const value = readText(extension);
          if (value) {
            extensions.add(value);
          }
        }
      }
    }
  }
  if (extensions.size > 0) {
    content.push(`${joinWords([...extensions])} files`);
  }
  return content;
}

function executableActions(interfaceEntries) {
  const actions = [];
  for (const interfaceEntry of interfaceEntries) {
    for (const binding of Array.isArray(interfaceEntry && interfaceEntry.bindings) ? interfaceEntry.bindings : []) {
      if (!binding || binding.executable !== true) {
        continue;
      }
      const methodId = readText(binding.method);
      if (methodId === "capsule.open") {
        actions.push("Open");
        continue;
      }
      const operation = methodId.split(".").filter(Boolean).at(-1);
      if (operation) actions.push(humanizeName(operation));
    }
  }
  return [...new Set(actions)];
}

function catalogSummary(entry) {
  const title = catalogTitle(entry);
  const role = readText(entry && entry.role);
  if (role === "provider") {
    return "Service for apps on this Home.";
  }
  if (role === "shell") {
    return readText(entry && entry.name).endsWith("-cli")
      ? "Use Home from a command line."
      : "Use the Home desktop.";
  }
  if (readText(entry && entry.name) === "home") {
    return "Keeps your selected Home view available.";
  }
  const description = readText(entry && entry.description)
    .replace(/\s+through the ElastOS [^.]+ boundary/gi, "")
    .replace(/\s+inside ElastOS/gi, "")
    .replace(/\bcapsules\b/gi, "apps")
    .replace(/\bproviders\b/gi, "services")
    .replace(/\bruntime settings\b/gi, "settings");
  if (description && !/\b(runtime|capsules?|providers?|projection|schema|derived facts?|boundary|capabilit(?:y|ies)|affordances?|host-loaded|frontend)\b/i.test(description)) {
    return description;
  }
  if (role === "viewer") {
    return `Open compatible content with ${title}.`;
  }
  return `${title} is available on this Home.`;
}

function catalogTitle(entry) {
  const title = readText(entry && entry.title);
  if (readText(entry && entry.role) === "provider") {
    return serviceLabel(entry);
  }
  return title ? normalizeDisplayTitle(title) : humanizeName(readText(entry && entry.name));
}

function serviceLabel(entry) {
  const title = readText(entry && entry.title) || humanizeName(readText(entry && entry.name));
  return normalizeDisplayTitle(title.replace(/\s+(Provider|Adapter)$/i, ""));
}

function normalizeDisplayTitle(value) {
  const acronyms = new Map([["Did", "DID"], ["Gba", "GBA"], ["Ipfs", "IPFS"], ["Cli", "CLI"], ["Gui", "GUI"]]);
  return readText(value).split(/\s+/).map((part) => acronyms.get(part) || part).join(" ");
}

function humanizeName(value) {
  const acronyms = new Map([
    ["did", "DID"],
    ["gba", "GBA"],
    ["ipfs", "IPFS"],
    ["cli", "CLI"],
    ["gui", "GUI"],
    ["wasm", "WASM"],
    ["microvm", "microVM"],
    ["ucity", "uCity"],
    ["metamask", "MetaMask"],
    ["unisat", "UniSat"],
    ["walletconnect", "WalletConnect"],
  ]);
  return readText(value)
    .split(/[\s_-]+/)
    .filter(Boolean)
    .map((part) => acronyms.get(part.toLowerCase()) || `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function joinWords(values) {
  const words = [...new Set(values.map(readText).filter(Boolean))];
  if (words.length < 2) {
    return words[0] || "";
  }
  return `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`;
}

function catalogEmpty(message) {
  const empty = document.createElement("div");
  empty.className = "catalog-empty";
  empty.textContent = message;
  return empty;
}

function setCapsuleCatalogBusy(busy) {
  if (capsuleCatalogRefreshButton) {
    capsuleCatalogRefreshButton.disabled = busy || !hasShellAccess();
  }
}

function showCapsuleCatalogStatus(message, tone) {
  if (!capsuleCatalogStatusNode) {
    return;
  }
  capsuleCatalogStatusNode.textContent = tone === "error"
    ? publicSystemError(message, "Apps and services could not be loaded.")
    : readText(message);
  capsuleCatalogStatusNode.dataset.tone = tone;
}

async function refreshPasskeyStatus() {
  if (!passkeyStatusNode || !window.PublicKeyCredential) {
    return;
  }
  if (passkeyAuthorityActive) {
    return;
  }
  const status = await fetchJson("/api/auth/passkey/status");
  showPasskeyStatus(status.registered ? "" : "Not set", status.registered ? "muted" : "muted");
}

async function refreshAccountList() {
  if (!accountListNode || !hasShellAccess()) {
    return;
  }
  const data = await fetchJson("/api/auth/passkeys", {
    headers: shellHeaders(),
  });
  renderAccounts(Array.isArray(data.passkeys) ? data.passkeys : []);
}

function renderAccounts(accounts) {
  if (!accountListNode) {
    return;
  }
  accountListNode.replaceChildren();
  const activeAccounts = accounts.filter((account) => !account.revoked_at);
  if (activeAccounts.length === 0) {
    const empty = document.createElement("div");
    empty.className = "account-empty";
    empty.textContent = "No accounts yet";
    accountListNode.append(empty);
    return;
  }
  const adminCount = activeAccounts.filter((account) => readText(account.role) === "admin").length;
  const table = document.createElement("table");
  table.className = "account-table";
  table.innerHTML = `
    <thead>
      <tr>
        <th scope="col">Account</th>
        <th scope="col">Role</th>
        <th scope="col">Sign-in</th>
        <th scope="col">Last used</th>
        <th scope="col">Actions</th>
      </tr>
    </thead>
  `;
  const body = document.createElement("tbody");
  for (const account of activeAccounts) {
    body.append(accountRow(account, {
      activeCount: activeAccounts.length,
      adminCount,
    }));
  }
  table.append(body);
  accountListNode.append(table);
}

function accountRow(passkey, listState = {}) {
  const row = document.createElement("tr");
  row.className = "account-row";

  const nameCell = accountCell("Account", "account-name");
  const nameWrap = document.createElement("div");
  nameWrap.className = "account-name-wrap";

  const title = document.createElement("strong");
  const role = passkeyRoleLabel(passkey.role);
  const label = readText(passkey.display_name) || (passkey.current ? "Current account" : "Account");
  title.textContent = label;

  nameWrap.append(title);
  nameCell.append(nameWrap);

  const roleCell = accountCell("Role", "account-role-cell");
  const roleBadge = document.createElement("span");
  roleBadge.className = "account-role";
  roleBadge.dataset.role = readText(passkey.role) || "guest";
  roleBadge.textContent = passkey.current ? `${role} · current` : role;
  roleCell.append(roleBadge);

  const methodCell = accountCell("Sign-in", "account-method");
  methodCell.textContent = "Passkey";

  const usedCell = accountCell("Last used", "account-used");
  usedCell.textContent = passkey.last_used_at ? formatTimestamp(passkey.last_used_at) : "Not used yet";

  const actions = document.createElement("div");
  actions.className = "account-actions";

  const passkeyRole = readText(passkey.role);
  const canManagePasskeyRoles = currentAccess.role === "admin";
  const adminCount = Number(listState.adminCount);
  if (canManagePasskeyRoles && passkeyRole === "guest") {
    const promote = document.createElement("button");
    promote.className = "system-button system-button-secondary passkey-promote";
    promote.type = "button";
    promote.textContent = "Make admin";
    promote.dataset.passkeyPromote = passkey.proof_binding_id || "";
    promote.disabled = !promote.dataset.passkeyPromote || !hasShellAccess();
    actions.append(promote);
  }
  if (canManagePasskeyRoles && passkeyRole === "admin" && !passkey.current) {
    const demote = document.createElement("button");
    demote.className = "system-button system-button-secondary passkey-demote";
    demote.type = "button";
    demote.textContent = "Make guest";
    demote.dataset.passkeyDemote = passkey.proof_binding_id || "";
    demote.disabled = !demote.dataset.passkeyDemote || !hasShellAccess() || adminCount <= 1;
    if (adminCount <= 1) {
      demote.title = "At least one admin must remain.";
    }
    actions.append(demote);
  }

  const remove = document.createElement("button");
  remove.className = "system-button system-button-secondary passkey-remove";
  remove.type = "button";
  remove.textContent = "Remove";
  remove.dataset.passkeyRevoke = passkey.proof_binding_id || "";
  remove.dataset.current = passkey.current ? "true" : "false";
  const protectsLastAdmin = readText(passkey.role) === "admin"
    && Number(listState.adminCount) <= 1
    && Number(listState.activeCount) > 1;
  remove.disabled = !remove.dataset.passkeyRevoke || !hasShellAccess() || protectsLastAdmin;
  if (protectsLastAdmin) {
    remove.title = "Remove guest passkeys before removing the last admin.";
  }
  actions.append(remove);

  const actionsCell = accountCell("Actions", "account-actions-cell");
  actionsCell.append(actions);

  row.append(nameCell, roleCell, methodCell, usedCell, actionsCell);
  return row;
}

function accountCell(label, className) {
  const cell = document.createElement("td");
  cell.dataset.label = label;
  if (className) {
    cell.className = className;
  }
  return cell;
}

function formatTimestamp(timestamp) {
  const seconds = Number(timestamp);
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return "recently";
  }
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(seconds * 1000));
}

async function onAccountListClick(event) {
  const button = event.target.closest([
    "[data-passkey-promote]",
    "[data-passkey-demote]",
    "[data-passkey-revoke]",
  ].join(", "));
  if (!button || !hasShellAccess()) {
    return;
  }
  const promote = Boolean(button.dataset.passkeyPromote);
  const demote = Boolean(button.dataset.passkeyDemote);
  let action = "revoke";
  let proofBindingId = readText(button.dataset.passkeyRevoke);
  if (promote) {
    action = "promote-admin";
    proofBindingId = readText(button.dataset.passkeyPromote);
  } else if (demote) {
    action = "demote-guest";
    proofBindingId = readText(button.dataset.passkeyDemote);
  }
  if (!proofBindingId) {
    return;
  }
  const revokingCurrent = button.dataset.current === "true";
  button.disabled = true;
  showPasskeyStatus(promote || demote ? "Updating" : "Removing", "muted");
  try {
    await fetchJson(`/api/auth/passkeys/${encodeURIComponent(proofBindingId)}/${action}`, {
      method: "POST",
      headers: shellHeaders(),
    });
    if (!promote && revokingCurrent) {
      apiHomeToken = "";
      passkeyAuthorityActive = false;
      renderAccounts([]);
      showPasskeyStatus("Removed. Open Home to sign in.", "muted");
    } else {
      await refreshAccountList();
      await refreshPasskeyStatus();
      showPasskeyStatus(promote || demote ? "Updated" : "Removed", "success");
    }
    notifyHomeSummaryChanged();
  } catch (error) {
    showPasskeyStatus(String(error.message || error), "error");
    button.disabled = false;
  }
}

async function refreshRecoveryStatus() {
  if (!recoveryStatusNode || !hasShellAccess()) {
    return;
  }
  try {
    const status = await fetchJson("/api/auth/recovery/status", {
      headers: shellHeaders(),
    });
    setRecoveryStatus(status);
  } catch (error) {
    showRecoveryStatus("Unavailable", "error");
    showRecoveryNote(String(error.message || error), "error");
    setRecoveryButton("Download Recovery Kit", true);
  }
}

function setRecoveryStatus(status) {
  const configured = status && status.recovery_configured === true;
  const downloadAvailable = status && status.recovery_download_available === true;
  const protectedRoot = status && status.protection_configured === true;
  if (configured && downloadAvailable) {
    showRecoveryStatus("", "success");
    showRecoveryNote("Downloads Home data recovery plus built-in Wallet recovery keys after passkey verification.", "muted");
    setRecoveryButton("Download Recovery Kit", false);
    return;
  }
  if (configured) {
    showRecoveryStatus("Needs download", "muted");
    showRecoveryNote("Create a fresh Recovery Kit to enable future downloads.", "muted");
    setRecoveryButton("Create Recovery Kit", false);
    return;
  }
  if (protectedRoot) {
    showRecoveryStatus("Verify kit", "muted");
    showRecoveryNote("Create or import a verified Recovery Kit before allowing public guests.", "muted");
  } else {
    showRecoveryStatus("Not set", "muted");
    showRecoveryNote("Create a Recovery Kit before storing important data or funds.", "muted");
  }
  setRecoveryButton("Create Recovery Kit", false);
}

async function onRecoveryDownload() {
  if (!hasShellAccess() || !recoveryDownloadButton) {
    return;
  }
  clearRecoveryPending();
  setRecoveryButton(recoveryDownloadButton.textContent, true);
  showRecoveryStatus("Preparing", "muted");
  showRecoveryNote("", "muted");
  try {
    const status = await fetchJson("/api/auth/recovery/status", {
      headers: shellHeaders(),
    });
    const bundle = await exportFullRecoveryBundle(status);
    downloadRecoveryKit(bundle);
    if (recoveryPasswordInput) {
      recoveryPasswordInput.value = "";
    }
    showRecoveryStatus("", "success");
    showRecoveryNote("Recovery Kit downloaded. Store it offline; it can recover Home data and included built-in Wallet accounts.", "success");
    setRecoveryButton("Download Recovery Kit", false);
  } catch (error) {
    showRecoveryStatus("Not set", "error");
    showRecoveryNote(String(error.message || error), "error");
    setRecoveryButton("Download Recovery Kit", false);
  }
}

async function onRecoveryImport(event) {
  if (!hasShellAccess()) {
    return;
  }
  clearRecoveryPending();
  const file = event && event.target && event.target.files && event.target.files[0];
  if (!file) {
    return;
  }
  showRecoveryStatus("Importing", "muted");
  showRecoveryNote("", "muted");
  try {
    const imported = JSON.parse(await file.text());
    const status = await fetchJson("/api/auth/recovery/status", {
      headers: shellHeaders(),
    });
    const plan = recoveryImportPlan(status, imported, { allowReassign: false });
    if (plan.reassign) {
      pendingRecoveryImport = { imported };
      showRecoveryStatus("Review", "muted");
      showRecoveryPending(plan);
      return;
    }
    await submitRecoveryImport(plan.request);
  } catch (error) {
    showRecoveryStatus("Import failed", "error");
    showRecoveryNote(String(error.message || error), "error");
  } finally {
    if (recoveryImportInput) {
      recoveryImportInput.value = "";
    }
  }
}

async function onRecoveryAttach() {
  if (!hasShellAccess() || !pendingRecoveryImport) {
    return;
  }
  if (recoveryAttachButton) {
    recoveryAttachButton.disabled = true;
  }
  showRecoveryStatus("Attaching", "muted");
  showRecoveryNote("", "muted");
  try {
    const status = await fetchJson("/api/auth/recovery/status", {
      headers: shellHeaders(),
    });
    const plan = recoveryImportPlan(status, pendingRecoveryImport.imported, { allowReassign: true });
    await submitRecoveryImport(plan.request);
    clearRecoveryPending();
  } catch (error) {
    showRecoveryStatus("Attach failed", "error");
    showRecoveryNote(String(error.message || error), "error");
    if (recoveryAttachButton) {
      recoveryAttachButton.disabled = false;
    }
  }
}

async function submitRecoveryImport(body) {
  const response = await fetchJson("/api/auth/recovery/full-import", {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body),
  });
  if (readText(response.system_token)) {
    apiHomeToken = readText(response.system_token);
  } else if (readText(response.home_token)) {
    apiHomeToken = readText(response.home_token);
  }
  if (recoveryPasswordInput) {
    recoveryPasswordInput.value = "";
  }
  showRecoveryStatus("", "success");
  showRecoveryNote(
    response.status === "reassigned"
      ? "Recovered root attached. Home may refresh to use it."
      : recoveryImportSuccessMessage(response),
    "success",
  );
  await refreshRecoveryStatus();
  await refreshAccountList();
  notifyHomeSummaryChanged();
}

function recoveryImportPlan(status, imported, options = {}) {
  const principalId = readText(status && status.principal_id);
  const localhostRoot = readText(status && status.localhost_root);
  const importedSchema = readText(imported && imported.schema);
  const kitPrincipal = readText(imported && imported.principal_id);
  const kitRoot = readText(imported && imported.localhost_root);
  const reassign = kitPrincipal !== principalId || kitRoot !== localhostRoot;
  const allowReassign = options.allowReassign === true;
  if (importedSchema === "elastos.full-recovery-bundle/v1") {
    return {
      request: fullRecoveryImportRequest(principalId, localhostRoot, imported, null, reassign, allowReassign),
      reassign,
      kitPrincipal,
      kitRoot,
    };
  }
  if (importedSchema === "elastos.full-recovery-bundle.package/v1") {
    return {
      request: fullRecoveryImportRequest(principalId, localhostRoot, null, imported, reassign, allowReassign),
      reassign,
      kitPrincipal,
      kitRoot,
    };
  }
  throw new Error("Unsupported Recovery Kit file.");
}

async function exportFullRecoveryBundle(status) {
  const downloadPassword = recoveryDownloadPassword();
  const intent = {
    principal_id: readText(status.principal_id),
    localhost_root: readText(status.localhost_root),
    label: "Recovery Kit",
  };
  const homeToken = await requestFreshPasskeyHomeToken(
    "auth.full-recovery-bundle.export",
    intent,
  );
  return fetchJson("/api/auth/recovery/full-export", {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }, homeToken),
    body: JSON.stringify({
      schema: "elastos.full-recovery-bundle.export.request/v1",
      ...intent,
      home_token: homeToken,
      ...(downloadPassword ? { download_password: downloadPassword } : {}),
    }),
  });
}

function fullRecoveryImportRequest(principalId, localhostRoot, bundle, recoveryPackage, reassign, allowReassign) {
  const request = {
    schema: "elastos.full-recovery-bundle.import.request/v1",
    principal_id: principalId,
    localhost_root: localhostRoot,
    reassign_to_current_principal: Boolean(reassign && allowReassign),
  };
  if (bundle) {
    request.bundle = bundle;
  }
  if (recoveryPackage) {
    request.package = recoveryPackage;
    const password = recoveryDownloadPassword();
    if (password) {
      request.password = password;
    }
  }
  return request;
}

function recoveryImportSuccessMessage(response) {
  const count = Number(response && response.wallet_recovery_key_count ? response.wallet_recovery_key_count : 0);
  if (count > 0) {
    return `Recovery Kit imported. Restored ${count} built-in Wallet ${count === 1 ? "account" : "accounts"}.`;
  }
  return "Recovery Kit imported.";
}

function recoveryDownloadPassword() {
  return readText(recoveryPasswordInput && recoveryPasswordInput.value);
}

function downloadRecoveryKit(kit) {
  const principal = shortText(readText(kit && kit.principal_id), 12) || "principal";
  const blob = new Blob([JSON.stringify(kit, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `elastos-recovery-${principal}.json`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

function setRecoveryButton(label, disabled) {
  if (recoveryDownloadButton) {
    recoveryDownloadButton.textContent = readText(label) || "Download Recovery Kit";
    recoveryDownloadButton.disabled = disabled || !hasShellAccess();
  }
}

function showRecoveryPending(plan) {
  if (!recoveryPendingNode || !recoveryPendingTextNode) {
    return;
  }
  const root = shortText(readText(plan && plan.kitRoot), 14) || "this root";
  recoveryPendingTextNode.textContent = `This kit belongs to another Home account (${root}). Recover that account with this passkey? The current temporary account will be replaced.`;
  recoveryPendingNode.hidden = false;
  showRecoveryNote("", "muted");
  if (recoveryAttachButton) {
    recoveryAttachButton.disabled = !hasShellAccess();
  }
}

function clearRecoveryPending() {
  pendingRecoveryImport = null;
  if (recoveryPendingNode) {
    recoveryPendingNode.hidden = true;
  }
  if (recoveryPendingTextNode) {
    recoveryPendingTextNode.textContent = "";
  }
  if (recoveryAttachButton) {
    recoveryAttachButton.disabled = false;
  }
}

function showRecoveryStatus(message, tone) {
  if (!recoveryStatusNode) {
    return;
  }
  const text = tone === "error"
    ? publicSystemError(message, "Recovery could not be updated.")
    : readText(message);
  recoveryStatusNode.hidden = text.length === 0;
  recoveryStatusNode.textContent = text;
  recoveryStatusNode.dataset.tone = tone;
  recoveryStatusNode.classList.toggle("system-value-muted", tone !== "success");
}

function showRecoveryNote(message, tone) {
  if (!recoveryNoteNode) {
    return;
  }
  const text = tone === "error"
    ? publicSystemError(message, "Recovery action could not be completed.")
    : readText(message);
  recoveryNoteNode.hidden = text.length === 0;
  recoveryNoteNode.textContent = text;
  recoveryNoteNode.dataset.tone = tone;
}

async function refreshChainNetworks() {
  if (!chainTableNode || !hasShellAccess()) {
    return;
  }
  try {
    const data = await fetchProviderJson("/api/provider/chain/networks", {});
    chainNetworks = Array.isArray(data.networks) ? data.networks : [];
    chainStatusById = new Map();
    chainLifecycleById = new Map();
    renderChainTable();
    await refreshChainStatuses();
  } catch (error) {
    chainNetworks = [];
    chainStatusById = new Map();
    chainLifecycleById = new Map();
    renderChainTable();
  }
}

async function refreshChainStatuses() {
  if (!chainTableNode || !hasShellAccess()) {
    return;
  }
  if (chainNetworks.length === 0) {
    return;
  }
  const next = new Map();
  for (const network of chainNetworks) {
    const networkId = readText(network.id);
    if (!READABLE_CHAIN_KINDS.has(network.kind)) {
      next.set(networkId, { tone: "muted", text: "Listed", detail: "Status unavailable" });
      continue;
    }
    try {
      const data = await fetchProviderJson("/api/provider/chain/status", { network: networkId });
      next.set(networkId, chainStatusView(network, data));
    } catch (error) {
      next.set(networkId, {
        tone: "error",
        text: "Unavailable",
        detail: publicSystemError(error, chainFailureNote(network)),
      });
    }
    await refreshChainLifecycle(networkId, false);
  }
  chainStatusById = next;
  renderChainTable();
}

async function onChainRowClick(event) {
  const actionButton = event.target && event.target.closest("[data-chain-action]");
  if (actionButton) {
    await onChainLifecycleAction(actionButton);
    return;
  }
  const row = event.target && event.target.closest("[data-chain-id]");
  if (!row || !hasShellAccess()) {
    return;
  }
  const chainId = readText(row.dataset.chainId);
  const network = chainNetworks.find((candidate) => readText(candidate.id) === chainId);
  if (!network) {
    return;
  }
  chainStatusById.set(chainId, { tone: "muted", text: "Refreshing", detail: "Checking status" });
  renderChainTable();
  try {
    if (!READABLE_CHAIN_KINDS.has(network.kind)) {
      chainStatusById.set(chainId, { tone: "muted", text: "Listed", detail: "Status unavailable" });
      return;
    }
    const data = await fetchProviderJson("/api/provider/chain/status", { network: chainId });
    chainStatusById.set(chainId, chainStatusView(network, data));
    await refreshChainLifecycle(chainId, false);
  } catch (error) {
    chainStatusById.set(chainId, {
      tone: "error",
      text: "Unavailable",
      detail: publicSystemError(error, chainFailureNote(network)),
    });
    await refreshChainLifecycle(chainId, false);
  } finally {
    renderChainTable();
  }
}

async function onChainRowKeydown(event) {
  if (event.key !== "Enter" && event.key !== " ") {
    return;
  }
  const row = event.target && event.target.closest("[data-chain-id]");
  if (!row || event.target.closest("[data-chain-action]")) {
    return;
  }
  event.preventDefault();
  await onChainRowClick({ target: row });
}

async function refreshChainLifecycle(chainId, renderWhenDone) {
  try {
    const lifecycle = await fetchProviderJson("/api/provider/chain/node_lifecycle", {
      network: chainId,
      action: "status",
    });
    chainLifecycleById.set(chainId, chainLifecycleView(lifecycle));
  } catch (error) {
    chainLifecycleById.set(chainId, {
      tone: "muted",
      text: "Controls unavailable",
      detail: "This network cannot be controlled from here.",
      control_available: false,
      busy: false,
    });
  }
  if (renderWhenDone) {
    renderChainTable();
  }
}

async function onChainLifecycleAction(button) {
  if (!hasShellAccess()) {
    return;
  }
  const chainId = readText(button.dataset.chainId);
  const action = readText(button.dataset.chainAction);
  if (!chainId || !["start", "stop", "restart"].includes(action)) {
    return;
  }
  const current = chainLifecycleById.get(chainId) || {};
  if (current.control_available !== true) {
    return;
  }
  chainLifecycleById.set(chainId, {
    ...current,
    tone: "muted",
    text: actionLabel(action),
    detail: "Applying change.",
    busy: true,
  });
  renderChainTable();
  try {
    const lifecycle = await fetchProviderJson("/api/provider/chain/node_lifecycle", {
      network: chainId,
      action,
    });
    chainLifecycleById.set(chainId, chainLifecycleView(lifecycle));
    const network = chainNetworks.find((candidate) => readText(candidate.id) === chainId);
    if (network && READABLE_CHAIN_KINDS.has(network.kind)) {
      const data = await fetchProviderJson("/api/provider/chain/status", { network: chainId });
      chainStatusById.set(chainId, chainStatusView(network, data));
    }
  } catch (error) {
    chainLifecycleById.set(chainId, {
      tone: "error",
      text: "Could not update",
      detail: publicSystemError(error, "The network control could not be updated."),
      control_available: current.control_available === true,
      busy: false,
    });
  } finally {
    renderChainTable();
  }
}

function chainLifecycleView(data) {
  const controlAvailable = data && data.control_available === true;
  const state = readText(data && data.state);
  const action = readText(data && data.action);
  return {
    tone: controlAvailable ? "success" : "muted",
    text: lifecycleLabel(state),
    detail: controlAvailable ? "Controls available" : "Controls unavailable",
    action,
    state,
    control_available: controlAvailable,
    busy: false,
  };
}

function lifecycleLabel(state) {
  switch (readText(state)) {
    case "managed_local":
      return "On this device";
    case "external_loopback":
      return "Local node";
    case "remote_backend":
      return "Remote";
    case "not_configured":
      return "Not configured";
    default:
      return "Unavailable";
  }
}

function actionLabel(action) {
  switch (readText(action)) {
    case "start":
      return "Starting";
    case "stop":
      return "Stopping";
    case "restart":
      return "Restarting";
    default:
      return "Updating";
  }
}

function chainStatusView(network, data) {
  if (
    network.kind === "bitcoin_core_rpc"
    || network.kind === "bitcoin_rest"
    || network.kind === "mainchain_rest"
  ) {
    const height = Number(data.block_height);
    const heightText = Number.isFinite(height) ? height.toLocaleString() : "unknown";
    return { tone: "success", text: "Online", detail: `Height ${heightText}` };
  }
  const blockNumber = Number(data.block_number);
  const blockText = Number.isFinite(blockNumber) ? blockNumber.toLocaleString() : readText(data.block_number_hex);
  return { tone: "success", text: "Online", detail: `Block ${blockText || "unknown"}` };
}

function chainFailureNote(network) {
  if (network.kind === "bitcoin_core_rpc") {
    return "Bitcoin status is unavailable.";
  }
  return "";
}

async function fetchProviderJson(url, body) {
  const payload = await fetchJson(url, {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body || {}),
  });
  if (payload && payload.status === "error") {
    throw new Error(readText(payload.message) || readText(payload.code) || "provider error");
  }
  return payload && payload.data ? payload.data : {};
}

function renderChainTable() {
  if (!chainTableNode) {
    return;
  }
  chainTableNode.replaceChildren();
  if (chainNetworks.length === 0) {
    const empty = document.createElement("div");
    empty.className = "network-row network-row-empty";
    empty.textContent = "No networks available.";
    chainTableNode.append(empty);
    return;
  }
  for (const network of chainNetworks) {
    const id = readText(network.id);
    const status = chainStatusById.get(id) || { tone: "muted", text: "Pending", detail: "Not checked yet" };
    const lifecycle = chainLifecycleById.get(id);
    const row = document.createElement("div");
    row.className = "network-row";
    row.role = "button";
    row.tabIndex = 0;
    row.title = `Refresh ${networkLabel(network)}`;
    row.dataset.chainId = id;
    row.dataset.tone = status.tone;
    if (status.text === "Refreshing" || (lifecycle && lifecycle.busy)) {
      row.dataset.busy = "true";
    }

    const icon = document.createElement("span");
    icon.className = "network-icon";
    icon.textContent = chainIconLabel(network);

    const main = document.createElement("span");
    main.className = "network-main";
    const name = document.createElement("strong");
    name.textContent = networkLabel(network);
    const address = document.createElement("small");
    address.textContent = `elastos://chain/${id}/status`;
    main.append(name, address);

    const state = document.createElement("span");
    state.className = "network-state";
    const stateText = document.createElement("strong");
    stateText.textContent = status.text;
    const detail = document.createElement("small");
    detail.textContent = lifecycle ? `${status.detail} · ${lifecycle.text}` : status.detail;
    state.append(stateText, detail);

    row.append(icon, main, state);
    if (lifecycle && lifecycle.control_available === true) {
      row.append(renderLifecycleActions(id, lifecycle));
    }
    chainTableNode.append(row);
  }
}

function renderLifecycleActions(chainId, lifecycle) {
  const actions = document.createElement("span");
  actions.className = "network-actions";
  for (const action of ["start", "stop", "restart"]) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "network-action";
    button.dataset.chainId = chainId;
    button.dataset.chainAction = action;
    button.disabled = lifecycle.busy === true;
    button.textContent = action === "restart" ? "Restart" : action === "start" ? "Start" : "Stop";
    actions.append(button);
  }
  return actions;
}

function networkLabel(network) {
  const name = readText(network.display_name) || readText(network.id) || "Unknown chain";
  const symbol = readText(network.native_symbol);
  return symbol ? `${name} (${symbol})` : name;
}

function chainLabel(namespace) {
  const value = readText(namespace);
  if (CHAIN_NAMESPACE_LABELS.has(value)) {
    return CHAIN_NAMESPACE_LABELS.get(value);
  }
  if (value.startsWith("eip155:")) {
    const chainId = value.slice("eip155:".length);
    return chainId ? `EVM ${chainId}` : "EVM";
  }
  if (value.startsWith("bip122:")) {
    return "Bitcoin";
  }
  return value;
}

function chainIconLabel(network) {
  const id = readText(network.id).toLowerCase();
  const symbol = readText(network.native_symbol).toUpperCase();
  if (id.includes("btc") || symbol === "BTC") {
    return "BTC";
  }
  if (id.includes("esc")) {
    return "ESC";
  }
  if (id.includes("base")) {
    return "BAS";
  }
  return symbol || "ELA";
}

function setPasskeyButtonsDisabled(disabled) {
  if (accountListNode) {
    accountListNode.dataset.busy = disabled ? "true" : "false";
  }
}

function showPasskeyStatus(message, tone) {
  if (!passkeyStatusNode) {
    return;
  }
  const text = tone === "error"
    ? publicSystemError(message, "Account action could not be completed.")
    : readText(message);
  passkeyStatusNode.hidden = text.length === 0;
  passkeyStatusNode.textContent = text;
  passkeyStatusNode.dataset.tone = tone;
  passkeyStatusNode.classList.toggle("system-value-muted", tone !== "success");
}

function setAccessPolicy(access) {
  currentAccess = {
    role: readText(access && access.role),
    localhost_root: readText(access && access.localhost_root),
    guest_registration_enabled: access && access.guest_registration_enabled === true,
  };
  setGuestRegistrationControlState();
}

function setGuestRegistrationControlState() {
  if (!guestRegistrationInput) {
    setPasskeyButtonsDisabled(false);
    return;
  }
  const isAdmin = currentAccess.role === "admin";
  guestRegistrationInput.checked = currentAccess.guest_registration_enabled === true;
  guestRegistrationInput.disabled = !hasShellAccess() || !isAdmin;
  setPasskeyButtonsDisabled(false);
}

function showGuestRegistrationStatus(message, tone) {
  if (!guestRegistrationStatusNode) {
    return;
  }
  const text = tone === "error"
    ? publicSystemError(message, "Access setting could not be updated.")
    : readText(message);
  guestRegistrationStatusNode.hidden = text.length === 0;
  guestRegistrationStatusNode.textContent = text;
  guestRegistrationStatusNode.dataset.tone = tone;
}

function clearGuestRegistrationStatus() {
  showGuestRegistrationStatus("", "muted");
}

function passkeyRoleLabel(role) {
  return readText(role) === "admin" ? "Admin" : "Guest";
}

function setPasskeyAuthority(authority) {
  const proofBinding = readText(authority && authority.proof_binding_id);
  if (!proofBinding || !proofBinding.startsWith("proof:passkey:")) {
    return;
  }
  passkeyAuthorityActive = true;
  showPasskeyStatus("", "muted");
}

function shortText(value, size) {
  const text = readText(value);
  const limit = Number(size);
  if (!Number.isFinite(limit) || text.length <= limit) {
    return text;
  }
  const side = Math.max(3, Math.floor(limit / 2));
  return `${text.slice(0, side)}…${text.slice(-side)}`;
}

function shortDid(did) {
  const value = readText(did);
  if (value.length <= 34) {
    return value;
  }
  const prefix = value.startsWith("did:key:") ? "did:key:" : "";
  const body = prefix ? value.slice(prefix.length) : value;
  return `${prefix}${body.slice(0, 10)}…${body.slice(-8)}`;
}

function setRuntimeState(runtime) {
  const version = readText(runtime && runtime.version);
  setTextFields("runtime-status", version);
  setHiddenFields("runtime-status", version.length === 0);
}

function setSourceState(source) {
  const configured = Boolean(source && source.configured);
  const name = configured ? readText(source.name) || "default" : "Not configured";
  const channel = readText(source && source.channel) || "not configured";
  const installed = readText(source && source.installed_version) || "unknown";
  const mode = readText(source && source.mode) || "development";
  const policy = readText(source && source.update_policy);
  const transport = readText(source && source.transport);
  const sourcePeer = readText(source && source.source_peer);
  const checksAllowed = Boolean(source && source.update_checks_allowed);
  setTextFields("source-status", configured ? mode : "Not configured");
  setHiddenFields("source-status", false);
  setTextFields("source-policy", policy);
  setTextFields("source-name", name);
  setTextFields("source-channel", channel);
  setTextFields("source-installed-version", installed);
  setTextFields("source-checks", checksAllowed ? "Allowed" : "Disabled");
  setTextFields(
    "source-transport",
    sourcePeer ? `${transport} Peer ${shortText(sourcePeer, 28)}` : transport,
  );
}

function readQueryParam(key) {
  const url = new URL(window.location.href);
  return (url.searchParams.get(key) || "").trim();
}

function readLaunchToken() {
  return new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
}

async function requestFreshPasskeyHomeToken(operation, request) {
  return requestHomePasskeyAuthority(apiHomeToken, homeParentOrigin, operation, request);
}

function requestHomePasskeyAuthority(homeToken, parentOrigin, operation, request) {
  if (!homeToken || window.top === window || !parentOrigin) {
    return Promise.reject(new Error("Open System from Home to verify your passkey."));
  }
  const requestId = window.crypto?.randomUUID?.()
    || `passkey-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      window.removeEventListener("message", onResult);
      reject(new Error("Passkey verification timed out."));
    }, 120_000);
    const onResult = (event) => {
      if (event.source !== window.top || event.origin !== parentOrigin) {
        return;
      }
      const result = event.data && typeof event.data === "object" ? event.data : null;
      if (result?.type !== "home:passkey-authority-result" || result.requestId !== requestId) {
        return;
      }
      window.clearTimeout(timeout);
      window.removeEventListener("message", onResult);
      const freshToken = readText(result.homeToken);
      if (freshToken) {
        resolve(freshToken);
        return;
      }
      reject(new Error(readText(result.error) || "Passkey verification failed."));
    };
    window.addEventListener("message", onResult);
    window.top.postMessage({
      type: "home:request-passkey-authority",
      requestId,
      homeToken,
      operation,
      request,
    }, parentOrigin);
  });
}

function shellHeaders(extra, authorityToken = apiHomeToken) {
  return Object.assign(
    authorityToken.length > 0 ? { "x-elastos-home-token": authorityToken } : {},
    extra || {},
  );
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function publicSystemError(value, fallback) {
  const message = readText(value && value.message ? value.message : value);
  if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
    return fallback;
  }
  return message;
}

function showError(error) {
  if (!errorNode) {
    return;
  }
  errorNode.hidden = false;
  errorNode.textContent = publicSystemError(error, "System could not be loaded.");
}
