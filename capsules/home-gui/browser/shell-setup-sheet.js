import { shellState, targetById, saveShellLayoutState } from "./shell-core.js?v=home-20260813a";
import {
  forgetChromeNotification,
  rememberChromeNotification,
  setHomeSetupNotificationHandler,
} from "./shell-notifications.js?v=home-20260813a";
import { openTarget, openSystemRecoverySave } from "./shell-windows.js?v=home-20260813a";

const PROFILE_READINESS_SCHEMA = "elastos.profile.readiness/v1";
const RECOVERY_READINESS_SCHEMA = "elastos.recovery.readiness/v1";
const SETUP_HOLD_TARGETS = new Set(["chat-room"]);
const SETUP_REMINDER_ID = "home-setup:profile-and-recovery";

let sheet = null;
let card = null;
let titleNode = null;
let leadNode = null;
let recoveryButton = null;
let recoveryBody = null;
let recoveryStep = null;
let closeButton = null;
let bound = false;
let dismissedThisSession = false;
let recoveryActBusy = false;
let drag = null;
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
  closeButton = document.querySelector("#setup-sheet-close");
  if (!sheet) {
    return;
  }
  bound = true;
  closeButton?.addEventListener("click", () => hideSetupSheet());
  document.querySelector("#setup-sheet-later")?.addEventListener("click", () => hideSetupSheet());
  recoveryButton?.addEventListener("click", () => openRecoveryAct());
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
  return status !== "signed_out" && !setupFinished(summary);
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
  const profileStatus = homeSetupStatus(shellState.currentSummary);
  if (profileStatus === "ready" || profileStatus === "signed_out") {
    return false;
  }
  showSetupSheet();
  return true;
}

export function syncSetupSheet(previous, summary) {
  if (previous?.authority?.signed_in !== true && summary?.authority?.signed_in === true) {
    dismissedThisSession = shellState.shellLayoutState.setupReminderDismissed === true;
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
  if (setupSheetOpen()) renderSetupSheet(summary);
  // Recover already opens System import. Summary refresh must keep that form usable.
  if (homeSetupStatus(summary) !== "ready" || dismissedThisSession) {
    rememberSetupReminder();
  } else {
    showSetupSheet();
  }
}

function rememberSetupReminder() {
  if (!homeSetupNeedsAct(shellState.currentSummary)) return;
  const profileReady = homeSetupStatus(shellState.currentSummary) === "ready";
  const unavailable = homeSetupStatus(shellState.currentSummary) === "unavailable" ||
    homeRecoveryStatus(shellState.currentSummary) === "unavailable";
  rememberChromeNotification({
    id: SETUP_REMINDER_ID, kind: "home_setup",
    title: unavailable ? "Check Home setup" : profileReady ? "Back up your Home" : "Continue recovery",
    body: unavailable ? "Open System to check setup."
      : profileReady
      ? "Save a Recovery Kit in System when you are ready. Without it, losing this device can mean losing your identity and keys."
      : "Import your Recovery Kit in System to restore your Profile.",
  });
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
    shellState.shellLayoutState.setupReminderDismissed = true;
    saveShellLayoutState();
    rememberSetupReminder();
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
  renderSetupSheet(shellState.currentSummary);
  sheet.hidden = false;
  sheet.inert = false;
  sheet.setAttribute("aria-hidden", "false");
  if (!alreadyOpen && !yielded) {
    (recoveryButton && !recoveryButton.disabled ? recoveryButton : closeButton)?.focus();
  }
  return true;
}

function renderSetupSheet(summary) {
  const unavailable = homeSetupStatus(summary) === "unavailable" || homeRecoveryStatus(summary) === "unavailable";
  const complete = setupFinished(summary);
  const profileReady = homeSetupStatus(summary) === "ready";
  if (titleNode) titleNode.textContent = "Welcome to Home";
  if (leadNode) leadNode.textContent = complete ? "Home is set up."
    : unavailable ? "Open System to check setup."
    : profileReady ? "Your Home is ready. Choose when to save your Recovery Kit."
    : "Continue recovery with your Recovery Kit in System.";
  recoveryStep?.classList.toggle("is-complete", complete);
  recoveryStep?.classList.toggle("is-current", !complete && !unavailable);
  if (recoveryBody) recoveryBody.textContent = unavailable
    ? "Open System to check setup."
    : complete ? "Profile and Recovery Kit are ready."
    : profileReady ? "Without a current kit kept away from this device, losing the device can mean losing your identity and keys."
    : "Import your kit to restore your Profile. Your existing identity stays with that kit.";
  if (recoveryButton) {
    recoveryButton.textContent = complete ? "Ready" : unavailable ? "Open System"
      : profileReady ? "Save Recovery Kit" : "Continue recovery";
    recoveryButton.disabled = recoveryActBusy || complete || !targetById(summary, "system");
    recoveryButton.setAttribute("aria-busy", String(recoveryActBusy));
    recoveryButton.classList.toggle("el-button-primary", !complete);
  }
}

async function openRecoveryAct() {
  if (recoveryActBusy) return;
  const profileStatus = homeSetupStatus(shellState.currentSummary);
  const unavailable = profileStatus === "unavailable" || homeRecoveryStatus(shellState.currentSummary) === "unavailable";
  if (!targetById(shellState.currentSummary, "system")) {
    return;
  }
  yieldSetupSheet();
  if (profileStatus !== "ready" || unavailable) {
    const query = { settings: "security" };
    if (profileStatus === "setup_required" && !unavailable) query.recovery = "import";
    openTarget("system", { query });
    return;
  }
  recoveryActBusy = true;
  renderSetupSheet(shellState.currentSummary);
  let saved = false;
  try {
    saved = await openSystemRecoverySave();
  } finally {
    recoveryActBusy = false;
    renderSetupSheet(shellState.currentSummary);
    if (!saved && sheet && !sheet.hidden) {
      restoreSetupSheetOverlay();
      if (recoveryBody) recoveryBody.textContent = "Recovery Kit was not saved. Try again, or choose Later.";
      recoveryButton?.focus();
    }
  }
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
