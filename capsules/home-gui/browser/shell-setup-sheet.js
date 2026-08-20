import { shellState, targetById } from "./shell-core.js?v=home-20260814a";
import { openTarget } from "./shell-windows.js?v=home-20260814a";

/* First-run setup is Home chrome, not a new authority path.
   Host summary `identity.profile_readiness` decides whether the sheet exists.
   Profile first. Recovery Kit is an offer on this sheet — skippable.
   Recovery ceremony stays in System Security. Signed Profile stays in People.
   Chat may fail closed; Home holds the next act so the user never hunts. */

const PROFILE_READINESS_SCHEMA = "elastos.profile.readiness/v1";
const SETUP_HOLD_TARGETS = new Set(["chat-room"]);

let sheet = null;
let titleNode = null;
let leadNode = null;
let recoveryButton = null;
let skipRecoveryButton = null;
let profileButton = null;
let closeButton = null;
let bound = false;
let dismissedThisSession = false;

export function bindSetupSheet() {
  if (bound) {
    return;
  }
  sheet = document.querySelector("#setup-sheet");
  titleNode = document.querySelector("#setup-sheet-title");
  leadNode = document.querySelector("#setup-sheet-lead");
  recoveryButton = document.querySelector("#setup-sheet-recovery");
  skipRecoveryButton = document.querySelector("#setup-sheet-skip-recovery");
  profileButton = document.querySelector("#setup-sheet-profile");
  closeButton = document.querySelector("#setup-sheet-close");
  if (!sheet) {
    return;
  }
  bound = true;
  closeButton?.addEventListener("click", () => hideSetupSheet());
  skipRecoveryButton?.addEventListener("click", () => hideSetupSheet());
  recoveryButton?.addEventListener("click", () => openRecoveryAct());
  profileButton?.addEventListener("click", () => openProfileAct());
  sheet.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideSetupSheet();
    }
  });
}

export function homeSetupStatus(summary) {
  if (summary?.authority?.signed_in !== true) {
    return "signed_out";
  }
  const readiness = summary?.identity?.profile_readiness;
  if (!readiness || readiness.schema !== PROFILE_READINESS_SCHEMA) {
    return "unknown";
  }
  const status = typeof readiness.status === "string" ? readiness.status.trim() : "";
  if (status === "ready" || status === "setup_required" || status === "unavailable") {
    return status;
  }
  return "unknown";
}

export function homeSetupNeedsAct(summary) {
  const status = homeSetupStatus(summary);
  return status === "setup_required" || status === "unavailable";
}

export function setupSheetOpen() {
  return Boolean(sheet) && !sheet.hidden;
}

export function holdHomeSetupAct(targetId) {
  if (!SETUP_HOLD_TARGETS.has(targetId)) {
    return false;
  }
  if (!homeSetupNeedsAct(shellState.currentSummary)) {
    return false;
  }
  showSetupSheet();
  return true;
}

export function syncSetupSheet(previous, summary) {
  if (previous?.authority?.principal_id !== summary?.authority?.principal_id) {
    dismissedThisSession = false;
  }
  if (!homeSetupNeedsAct(summary)) {
    hideSetupSheet({ restoreFocus: false, rememberDismiss: false });
    return;
  }
  if (!dismissedThisSession) {
    showSetupSheet();
  }
}

export function hideSetupSheet({ restoreFocus = true, rememberDismiss = true } = {}) {
  if (!sheet || sheet.hidden) {
    return;
  }
  if (rememberDismiss) {
    dismissedThisSession = true;
  }
  sheet.hidden = true;
  sheet.inert = true;
  sheet.setAttribute("aria-hidden", "true");
  if (restoreFocus) {
    document.querySelector("#toolbar-home")?.focus();
  }
}

export function showSetupSheet() {
  if (!sheet || !homeSetupNeedsAct(shellState.currentSummary)) {
    return false;
  }
  renderSetupSheet(shellState.currentSummary);
  sheet.hidden = false;
  sheet.inert = false;
  sheet.setAttribute("aria-hidden", "false");
  (profileButton && !profileButton.disabled ? profileButton : closeButton)?.focus();
  return true;
}

function renderSetupSheet(summary) {
  const unavailable = homeSetupStatus(summary) === "unavailable";
  const setupName = typeof summary?.identity?.profile_setup_display_name === "string"
    ? summary.identity.profile_setup_display_name.trim()
    : "";
  if (titleNode) {
    titleNode.textContent = "Welcome to Home";
  }
  if (leadNode) {
    leadNode.textContent = unavailable
      ? "Profile could not be verified. Save a Recovery Kit, then create your Profile."
      : (setupName
        ? `Create your Profile as ${setupName}. A Recovery Kit is optional — save one now or skip.`
        : "Create your Profile. A Recovery Kit is optional — save one now or skip.");
  }
  if (recoveryButton) {
    recoveryButton.disabled = !targetById(summary, "system");
  }
  if (profileButton) {
    profileButton.disabled = !targetById(summary, "people");
  }
}

function openRecoveryAct() {
  if (!targetById(shellState.currentSummary, "system")) {
    return;
  }
  openTarget("system", { query: { settings: "security" } });
}

function openProfileAct() {
  if (!targetById(shellState.currentSummary, "people")) {
    return;
  }
  openTarget("people");
}
