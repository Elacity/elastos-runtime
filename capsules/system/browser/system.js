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
const webspaceListNode = document.querySelector("#webspace-list");
const chainTableNode = document.querySelector("#chain-table");
const frameHomeToken = readQueryParam("home_token");
let apiHomeToken = frameHomeToken;
let chainNetworks = [];
let chainStatusById = new Map();
let chainLifecycleById = new Map();
let currentAccess = {};
let passkeyAuthorityActive = false;
let pendingRecoveryImport = null;
const DEFAULT_BACKGROUND_IMAGE_URL = "/apps/home/wallpaper.webp";
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

boot().catch((error) => {
  console.error("system boot failed", error);
  showError(error);
});

async function boot() {
  configureHandleEditor();
  configureAppearanceEditor();
  configureGuestAccess();
  configurePasskeyAccess();
  configureRecoveryAccess();
  configureChainAccess();
  await refreshSystemSummary();
  await refreshAccountList().catch(() => {});
  await refreshRecoveryStatus();
  await refreshChainNetworks();
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
  const storage = systemSummary.storage || {};
  const webspace = systemSummary.webspace || {};

  setField("device-did", shortDid(identity.device_did), "", identity.device_did);
  setHandle(identity.handle);
  setAccessPolicy(access);
  setPasskeyAuthority(authority);
  setAppearance(appearance);
  setRuntimeState(runtime);
  setStorageState(storage);
  setWebspaceState(webspace);
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
        "x-elastos-home-token": apiHomeToken,
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

function notifyHomeSummaryChanged() {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage({
      type: "home:refresh-summary",
      homeToken: frameHomeToken,
    }, window.location.origin);
  }
}

function setWebspaceState(webspace) {
  if (!webspaceListNode) {
    return;
  }
  const entries = Array.isArray(webspace && webspace.entries) ? webspace.entries : [];
  webspaceListNode.replaceChildren();
  if (entries.length === 0) {
    const empty = document.createElement("div");
    empty.className = "webspace-row webspace-row-empty";
    empty.textContent = "No capsules or providers discovered.";
    webspaceListNode.append(empty);
    return;
  }
  for (const entry of entries) {
    webspaceListNode.append(renderWebspaceEntry(entry));
  }
}

function renderWebspaceEntry(entry) {
  const row = document.createElement("details");
  row.className = "webspace-row";
  row.dataset.role = readText(entry.role) || "capsule";

  const summary = document.createElement("summary");
  summary.className = "webspace-summary";

  const icon = document.createElement("span");
  icon.className = "webspace-icon";
  icon.textContent = webspaceIcon(entry);

  const main = document.createElement("span");
  main.className = "webspace-main";
  const name = document.createElement("strong");
  name.textContent = webspaceName(entry);
  const uri = document.createElement("small");
  uri.textContent = readText(entry.uri) || `elastos://capsules/${readText(entry.id) || "unknown"}`;
  main.append(name, uri);

  const state = document.createElement("span");
  state.className = "webspace-state";
  const status = document.createElement("strong");
  status.textContent = webspaceStatus(entry);
  const backend = document.createElement("small");
  backend.textContent = readText(entry.backend) || "capsule";
  state.append(status, backend);

  summary.append(icon, main, state);
  row.append(summary, renderWebspaceDetails(entry));
  return row;
}

function renderWebspaceDetails(entry) {
  const details = document.createElement("div");
  details.className = "webspace-details";
  details.append(
    webspaceDetail("Role", `${readText(entry.role) || "unknown"} · ${readText(entry.capsule_type) || "unknown"}`),
    webspaceDetail("Authority", readText(entry.authority_boundary) || "Capability-scoped Runtime access."),
  );

  const capabilities = Array.isArray(entry.capabilities) ? entry.capabilities.map(readText).filter(Boolean) : [];
  if (capabilities.length > 0) {
    details.append(webspaceDetail("Requires", capabilities.join(", ")));
  }

  const operations = Array.isArray(entry.operations) ? entry.operations.map(readText).filter(Boolean) : [];
  if (operations.length > 0) {
    details.append(webspaceDetail("Operations", operations.join(", ")));
  }

  const route = readText(entry.route);
  if (route) {
    const action = document.createElement("button");
    action.type = "button";
    action.className = "webspace-open";
    action.textContent = "Open capsule";
    action.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      openCapsuleTarget(readText(entry.id));
    });
    details.append(action);
  }

  return details;
}

function webspaceDetail(label, value) {
  const item = document.createElement("span");
  item.className = "webspace-detail";
  const key = document.createElement("b");
  key.textContent = label;
  const text = document.createElement("span");
  text.textContent = value;
  item.append(key, text);
  return item;
}

function openCapsuleTarget(target) {
  const id = readText(target);
  if (!id || !window.parent || window.parent === window) {
    return;
  }
  window.parent.postMessage({
    type: "home:open-target",
    target: id,
  }, window.location.origin);
}

function webspaceName(entry) {
  const id = readText(entry && entry.id);
  if (!id) {
    return "Unknown capsule";
  }
  return id.split("-").map((part) => (
    part ? part.charAt(0).toUpperCase() + part.slice(1) : part
  )).join(" ");
}

function webspaceStatus(entry) {
  if (entry && entry.running === true) {
    return "Running";
  }
  const status = readText(entry && entry.status);
  return status ? status.charAt(0).toUpperCase() + status.slice(1) : "Installed";
}

function webspaceIcon(entry) {
  const role = readText(entry && entry.role);
  if (role === "provider") {
    return "P";
  }
  if (role === "shell") {
    return "H";
  }
  if (role === "viewer") {
    return "V";
  }
  if (role === "content") {
    return "C";
  }
  return "A";
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
  const endpoint = body && body.schema === "elastos.full-recovery-bundle.import.request/v1"
    ? "/api/auth/recovery/full-import"
    : "/api/auth/recovery/import";
  const response = await fetchJson(endpoint, {
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
  const request = {
    schema: "elastos.recovery-kit.import.request/v1",
    principal_id: principalId,
    localhost_root: localhostRoot,
    reassign_to_current_principal: Boolean(reassign && allowReassign),
  };
  if (importedSchema === "elastos.recovery-kit.package/v1") {
    request.package = imported;
    const password = recoveryDownloadPassword();
    if (password) {
      request.password = password;
    }
    return { request, reassign, kitPrincipal, kitRoot };
  }
  if (importedSchema === "elastos.recovery-kit/v1") {
    request.kit = imported;
    return { request, reassign, kitPrincipal, kitRoot };
  }
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
  const homeToken = await requestFreshPasskeyHomeToken();
  return fetchJson("/api/auth/recovery/full-export", {
    method: "POST",
    headers: shellHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({
      schema: "elastos.full-recovery-bundle.export.request/v1",
      principal_id: readText(status.principal_id),
      localhost_root: readText(status.localhost_root),
      label: "Recovery Kit",
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
  const text = readText(message);
  recoveryStatusNode.hidden = text.length === 0;
  recoveryStatusNode.textContent = text;
  recoveryStatusNode.dataset.tone = tone;
  recoveryStatusNode.classList.toggle("system-value-muted", tone !== "success");
}

function showRecoveryNote(message, tone) {
  if (!recoveryNoteNode) {
    return;
  }
  const text = readText(message);
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
      next.set(networkId, { tone: "muted", text: "Listed", detail: "Typed reads pending" });
      continue;
    }
    try {
      const data = await fetchProviderJson("/api/provider/chain/status", { network: networkId });
      next.set(networkId, chainStatusView(network, data));
    } catch (error) {
      next.set(networkId, {
        tone: "error",
        text: "Unavailable",
        detail: readText(error.message || error) || chainFailureNote(network),
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
      chainStatusById.set(chainId, { tone: "muted", text: "Listed", detail: "Typed reads pending" });
      return;
    }
    const data = await fetchProviderJson("/api/provider/chain/status", { network: chainId });
    chainStatusById.set(chainId, chainStatusView(network, data));
    await refreshChainLifecycle(chainId, false);
  } catch (error) {
    chainStatusById.set(chainId, {
      tone: "error",
      text: "Unavailable",
      detail: readText(error.message || error) || chainFailureNote(network),
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
      text: "Control off",
      detail: readText(error.message || error) || "Lifecycle unavailable",
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
    detail: "Sending operator-approved control request",
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
      text: "Control failed",
      detail: readText(error.message || error) || "Lifecycle request failed",
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
  const reason = readText(data && data.control_reason);
  return {
    tone: controlAvailable ? "success" : "muted",
    text: lifecycleLabel(state),
    detail: controlAvailable ? "Local controls enabled" : reason || "Remote or unmanaged node",
    action,
    state,
    control_available: controlAvailable,
    busy: false,
  };
}

function lifecycleLabel(state) {
  switch (readText(state)) {
    case "managed_local":
      return "Managed local";
    case "external_loopback":
      return "Local node";
    case "remote_backend":
      return "Remote provider";
    case "not_configured":
      return "Not configured";
    default:
      return "Lifecycle";
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
    return "Configure Bitcoin Core in chain-provider to enable BTC status.";
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
    empty.textContent = "No chains available.";
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
  const text = readText(message);
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
  const text = readText(message);
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

async function requestFreshPasskeyHomeToken() {
  if (!window.PublicKeyCredential) {
    throw new Error("Passkey verification is unavailable in this browser.");
  }
  const begin = await fetchJson("/api/auth/passkey/authenticate/begin", { method: "POST" });
  const credential = await navigator.credentials.get(toRequestOptions(begin.options));
  if (!credential) {
    throw new Error("Passkey verification was cancelled.");
  }
  const complete = await fetchJson("/api/auth/passkey/authenticate/complete", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      ceremony_id: begin.ceremony_id,
      response: serializeAssertionCredential(credential),
    }),
  });
  const homeToken = readText(complete.home_token);
  if (!homeToken) {
    throw new Error("Fresh passkey token was not issued.");
  }
  return homeToken;
}

function toRequestOptions(options) {
  const publicKey = { ...(options && options.publicKey ? options.publicKey : {}) };
  publicKey.challenge = base64UrlToBuffer(publicKey.challenge);
  publicKey.allowCredentials = (publicKey.allowCredentials || []).map((credential) => ({
    ...credential,
    id: base64UrlToBuffer(credential.id),
  }));
  return { publicKey };
}

function serializeAssertionCredential(credential) {
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: bufferToBase64Url(credential.response.authenticatorData),
      clientDataJson: bufferToBase64Url(credential.response.clientDataJSON),
      signature: bufferToBase64Url(credential.response.signature),
      userHandle: credential.response.userHandle ? bufferToBase64Url(credential.response.userHandle) : null,
    },
  };
}

function base64UrlToBuffer(value) {
  const input = readText(value).replace(/-/g, "+").replace(/_/g, "/");
  const padded = input + "=".repeat((4 - (input.length % 4)) % 4);
  const binary = window.atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes.buffer;
}

function bufferToBase64Url(value) {
  const bytes = new Uint8Array(value);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return window.btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function shellHeaders(extra) {
  return Object.assign(
    apiHomeToken.length > 0 ? { "x-elastos-home-token": apiHomeToken } : {},
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
