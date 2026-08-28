import { shellState, targetById } from "./shell-core.js?v=home-20260813a";
import {
  forgetChromeNotification,
  rememberChromeNotification,
  setHomeSetupNotificationHandler,
} from "./shell-notifications.js?v=home-20260813a";
import { openTarget } from "./shell-windows.js?v=home-20260813a";

const PROFILE_READINESS_SCHEMA = "elastos.profile.readiness/v1";
const RECOVERY_READINESS_SCHEMA = "elastos.recovery.readiness/v1";
const SETUP_HOLD_TARGETS = new Set(["chat-room"]);
const SETUP_REMINDER_ID = "home-setup:recovery-then-profile";

let sheet = null;
let card = null;
let titleNode = null;
let leadNode = null;
let recoveryButton = null;
let recoveryBody = null;
let recoveryStep = null;
let profileButton = null;
let profileBody = null;
let profileStep = null;
let closeButton = null;
let bound = false;
let dismissedThisSession = false;
let drag = null;
let lastRecoveryReady = false;
let finishedHideTimer = null;

export function bindSetupSheet() {
  if (bound) {
    return;
  }
  sheet = document.querySelector("#setup-sheet");
  card = document.querySelector(".setup-sheet-card");
  titleNode = document.querySelector("#setup-sheet-title");
  leadNode = document.querySelector("#setup-sheet-lead");
  recoveryButton = document.querySelector("#setup-sheet-recovery");
  recoveryBody = document.querySelector("#setup-sheet-recovery-body");
  recoveryStep = document.querySelector("#setup-sheet-step-recovery");
  profileButton = document.querySelector("#setup-sheet-profile");
  profileBody = document.querySelector("#setup-sheet-profile-body");
  profileStep = document.querySelector("#setup-sheet-step-profile");
  closeButton = document.querySelector("#setup-sheet-close");
  if (!sheet) {
    return;
  }
  bound = true;
  closeButton?.addEventListener("click", () => hideSetupSheet());
  recoveryButton?.addEventListener("click", () => openRecoveryAct());
  profileButton?.addEventListener("click", () => openProfileAct());
  setHomeSetupNotificationHandler(() => {
    dismissedThisSession = false;
    showSetupSheet();
  });
  card?.addEventListener("pointerdown", onSetupCardPointerDown);
  window.addEventListener("pointermove", onSetupCardPointerMove);
  window.addEventListener("pointerup", onSetupCardPointerUp);
  window.addEventListener("pointercancel", onSetupCardPointerUp);
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
  return typedReadinessStatus(summary?.identity?.profile_readiness, PROFILE_READINESS_SCHEMA);
}

export function homeRecoveryStatus(summary) {
  if (summary?.authority?.signed_in !== true) {
    return "signed_out";
  }
  return typedReadinessStatus(summary?.identity?.recovery_readiness, RECOVERY_READINESS_SCHEMA);
}

function typedReadinessStatus(readiness, schema) {
  if (!readiness || readiness.schema !== schema) {
    return "unavailable";
  }
  const status = typeof readiness.status === "string" ? readiness.status.trim() : "";
  if (status === "ready" || status === "setup_required" || status === "unavailable") {
    return status;
  }
  return "unavailable";
}

export function homeSetupNeedsAct(summary) {
  const status = homeSetupStatus(summary);
  return status !== "ready" && status !== "signed_out";
}

function setupFinished(summary) {
  return homeSetupStatus(summary) === "ready" && homeRecoveryStatus(summary) === "ready";
}

export function setupSheetOpen() {
  return Boolean(sheet) && !sheet.hidden;
}

function scheduleFinishedSetupSheetHide() {
  if (finishedHideTimer !== null) {
    return;
  }
  const holdMs = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 1400;
  finishedHideTimer = window.setTimeout(() => {
    finishedHideTimer = null;
    hideSetupSheet({ restoreFocus: true, rememberDismiss: false });
  }, holdMs);
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
  if (previous?.authority?.signed_in !== true && summary?.authority?.signed_in === true) {
    dismissedThisSession = false;
  }
  if (!homeSetupNeedsAct(summary)) {
    forgetChromeNotification(SETUP_REMINDER_ID);
    if (setupFinished(summary) && setupSheetOpen()) {
      renderSetupSheet(summary);
      scheduleFinishedSetupSheetHide();
      return;
    }
    hideSetupSheet({ restoreFocus: false, rememberDismiss: false });
    return;
  }
  if (!dismissedThisSession) {
    showSetupSheet();
  }
}

export function hideSetupSheet({ restoreFocus = true, rememberDismiss = true } = {}) {
  if (finishedHideTimer !== null) {
    window.clearTimeout(finishedHideTimer);
    finishedHideTimer = null;
  }
  if (!sheet || sheet.hidden) {
    return;
  }
  if (rememberDismiss) {
    dismissedThisSession = true;
    if (homeSetupNeedsAct(shellState.currentSummary)) {
      rememberChromeNotification({
        id: SETUP_REMINDER_ID,
        kind: "home_setup",
        title: "Finish Home setup",
        body: "Save a Recovery Kit, then create your Profile.",
      });
    }
  }
  drag = null;
  restoreSetupSheetOverlay();
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
  const alreadyOpen = !sheet.hidden;
  const yielded = sheet.classList.contains("is-yielded");
  const unavailable = homeSetupStatus(shellState.currentSummary) === "unavailable" ||
    homeRecoveryStatus(shellState.currentSummary) === "unavailable";
  renderSetupSheet(shellState.currentSummary);
  sheet.hidden = false;
  sheet.inert = false;
  sheet.setAttribute("aria-hidden", "false");
  if (!alreadyOpen && !yielded) {
    const next = unavailable || homeRecoveryStatus(shellState.currentSummary) !== "ready"
      ? recoveryButton
      : profileButton;
    (next && !next.disabled ? next : closeButton)?.focus();
  }
  return true;
}

function renderSetupSheet(summary) {
  const profileStatus = homeSetupStatus(summary);
  const recoveryStatus = homeRecoveryStatus(summary);
  const unavailable = profileStatus === "unavailable" || recoveryStatus === "unavailable";
  const recoveryReady = recoveryStatus === "ready";
  const profileReady = profileStatus === "ready";
  const setupName = typeof summary?.identity?.profile_setup_display_name === "string"
    ? summary.identity.profile_setup_display_name.trim()
    : "";
  if (titleNode) {
    titleNode.textContent = "Welcome to Home";
  }
  if (leadNode) {
    leadNode.textContent = unavailable
      ? "Setup could not be verified. Open System to check Recovery Kit, then create your Profile."
      : (profileReady && recoveryReady
        ? "Home is set up."
        : (recoveryReady
          ? (setupName
            ? `Recovery Kit is saved. Create your Profile as ${setupName}.`
            : "Recovery Kit is saved. Create your Profile.")
          : (setupName
            ? `Save a Recovery Kit, then create your Profile as ${setupName}.`
            : "Save a Recovery Kit, then create your Profile.")));
  }
  recoveryStep?.classList.toggle("is-complete", recoveryReady);
  recoveryStep?.classList.toggle("is-current", !recoveryReady && !unavailable);
  profileStep?.classList.toggle("is-complete", profileReady);
  profileStep?.classList.toggle("is-current", recoveryReady && !profileReady && !unavailable);
  if (recoveryBody) {
    recoveryBody.textContent = unavailable
      ? "Open System."
      : recoveryReady
        ? "Saved. This Home can now create your Profile."
        : "Required first. Download a kit in System → Security so this Home can create your Profile.";
  }
  if (recoveryButton) {
    recoveryButton.textContent = unavailable
      ? "Open System"
      : recoveryReady
        ? "Saved"
        : "Save kit";
    recoveryButton.disabled = unavailable
      ? !targetById(summary, "system")
      : recoveryReady || !targetById(summary, "system");
    recoveryButton.classList.toggle("el-button-primary", unavailable || !recoveryReady);
  }
  if (profileBody) {
    profileBody.textContent = profileReady
      ? "Created. People and Chat can use this name."
      : "The name people see. Chat stays closed until this exists.";
  }
  if (profileButton) {
    profileButton.textContent = profileReady ? "Created" : "Create Profile";
    profileButton.disabled = unavailable || profileReady || !recoveryReady || !targetById(summary, "people");
    profileButton.classList.toggle("el-button-primary", !unavailable && recoveryReady && !profileReady);
  }
  if (recoveryReady && !lastRecoveryReady && profileButton && !profileButton.disabled) {
    profileButton.focus();
  }
  lastRecoveryReady = recoveryReady;
}

function openRecoveryAct() {
  if (!targetById(shellState.currentSummary, "system")) {
    return;
  }
  yieldSetupSheet();
  openTarget("system", { query: { settings: "security" } });
}

function openProfileAct() {
  if (!targetById(shellState.currentSummary, "people")) {
    return;
  }
  yieldSetupSheet();
  openTarget("people");
}

function yieldSetupSheet() {
  if (!sheet) {
    return;
  }
  sheet.classList.add("is-yielded");
  sheet.setAttribute("aria-modal", "false");
}

function restoreSetupSheetOverlay() {
  if (!sheet) {
    return;
  }
  sheet.classList.remove("is-yielded");
  sheet.setAttribute("aria-modal", "true");
  if (card) {
    card.classList.remove("is-dragging");
    card.style.left = "";
    card.style.top = "";
    card.style.right = "";
    card.style.position = "";
  }
}

function onSetupCardPointerDown(event) {
  if (!sheet || !card || sheet.hidden || !sheet.classList.contains("is-yielded")) {
    return;
  }
  if (event.button !== 0) {
    return;
  }
  if (event.target.closest("button, a, input, textarea, select, label")) {
    return;
  }
  const rect = card.getBoundingClientRect();
  drag = {
    offsetX: event.clientX - rect.left,
    offsetY: event.clientY - rect.top,
    width: rect.width,
    height: rect.height,
  };
  card.classList.add("is-dragging");
  event.preventDefault();
}

function onSetupCardPointerMove(event) {
  if (!drag || !card) {
    return;
  }
  const maxX = Math.max(8, window.innerWidth - drag.width - 8);
  const maxY = Math.max(8, window.innerHeight - drag.height - 8);
  const x = Math.min(Math.max(8, event.clientX - drag.offsetX), maxX);
  const y = Math.min(Math.max(8, event.clientY - drag.offsetY), maxY);
  card.style.position = "fixed";
  card.style.left = `${x}px`;
  card.style.top = `${y}px`;
  card.style.right = "auto";
}

function onSetupCardPointerUp() {
  if (!drag || !card) {
    return;
  }
  drag = null;
  card.classList.remove("is-dragging");
}
