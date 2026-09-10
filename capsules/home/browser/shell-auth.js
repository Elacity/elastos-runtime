import {
  clearHomeAuthorityToken,
  fetchJson,
  setHomeAuthorityToken,
} from "./shell-core.js?v=home-20260802a";

const unlockPanel = document.querySelector("#home-unlock");
const unlockFace = document.querySelector(".home-unlock-face");
const unlockCard = document.querySelector(".home-unlock-card");
const unlockDate = document.querySelector("#home-unlock-date");
const unlockTime = document.querySelector("#home-unlock-time");
const unlockTitle = document.querySelector("#home-unlock-title");
const unlockCopy = document.querySelector("#home-unlock-copy");
const unlockPerson = document.querySelector("#home-unlock-person");
const unlockPersonName = document.querySelector("#home-unlock-person-name");
const unlockMonogram = document.querySelector("#home-unlock-monogram");
const unlockPrimary = document.querySelector("#home-unlock-primary");
const unlockSecondary = document.querySelector("#home-unlock-secondary");
const unlockStatus = document.querySelector("#home-unlock-status");
const unlockName = document.querySelector("#home-unlock-name");
const ownerToken = document.querySelector("#home-owner-token");
const ownerTokenLabel = document.querySelector("#home-owner-token-label");
const enrollmentChoice = document.querySelector("#home-enrollment-choice");
const enrollmentCreate = document.querySelector("#home-enrollment-create");
const enrollmentRecover = document.querySelector("#home-enrollment-recover");
const enrollmentDismiss = document.querySelector("#home-enrollment-dismiss");
const PENDING_REGISTRATION_KEY = "elastos.home.pending-registration/v1";
const PENDING_REGISTRATION_MS = 12 * 60 * 60 * 1000;
const MAX_PENDING_REGISTRATION_CHARS = 65536;
let pendingRegistration = null;
let pendingRegistrationInvalid = false;
let enrollmentPurpose = "create";
let guestRegistrationAvailable = false;

let unlockMode = "signin";
let unlockPresentation = "modal";
let unlockCallback = null;
let busy = false;
let sessionRefreshInFlight = null;
let unlockClockTimer = 0;
let unlockLeaveTimer = 0;
let unlockPersonLabel = "";

export function isHomeAuthError(error) {
  const status = Number(error && error.status);
  return status === 401 || status === 403;
}

export async function showHomeUnlock(onUnlocked, options = {}) {
  unlockCallback = typeof onUnlocked === "function" ? onUnlocked : null;
  unlockPresentation = options && options.presentation === "prompt" ? "prompt" : "modal";
  unlockPersonLabel = readUnlockPersonLabel(options && options.personName);
  if (!unlockPanel) {
    throw new Error("Home unlock surface is missing");
  }
  cancelUnlockLeave();
  document.body.dataset.homeStatus = unlockPresentation === "prompt" ? "ready" : "locked";
  unlockPanel.dataset.mode = unlockPresentation;
  unlockPanel.dataset.surface = "neutral";
  delete unlockPanel.dataset.flow;
  unlockPanel.style.removeProperty("--home-unlock-ground");
  renderUnlockChecking();
  unlockPanel.hidden = false;
  unlockPanel.setAttribute("aria-hidden", "false");
  unlockCard?.setAttribute("aria-modal", "true");

  if (!window.PublicKeyCredential) {
    unlockMode = "unsupported";
    renderUnlockMode({ registered: true, guestRegistrationEnabled: false });
    setUnlockStatus("Passkeys are not available in this browser.", "error");
    return;
  }

  try {
    const status = await fetchJson("/api/auth/passkey/status");
    const registered = status.registered === true;
    const guestRegistrationEnabled = status.guest_registration_enabled === true;
    guestRegistrationAvailable = guestRegistrationEnabled;
    unlockMode = status.owner_setup_pending === true ? "resume_owner" : registered
      ? (guestRegistrationEnabled ? "signin_guest_enabled" : "signin")
      : "create";
    try {
      pendingRegistration = readPendingRegistration();
      if (pendingRegistration) {
        enrollmentPurpose = pendingRegistration.intent.purpose;
        if (unlockName) unlockName.value = pendingRegistration.intent.public_name || "";
        if (pendingRegistration.response) unlockMode = "resume_registration";
        else if (registered && unlockMode !== "resume_owner") unlockMode = "create_guest";
      }
    } catch (error) {
      if (pendingRegistrationInvalid) unlockMode = registered && status.owner_setup_pending !== true ? "create_guest" : "resume_owner";
      renderUnlockMode({ registered, guestRegistrationEnabled });
      setUnlockStatus(error.message, "error");
      return;
    }
    renderUnlockMode({ registered, guestRegistrationEnabled });
    setUnlockStatus(unlockStatusCopy(registered, guestRegistrationEnabled), "muted");
  } catch (error) {
    unlockMode = "signin";
    renderUnlockMode({ registered: true, guestRegistrationEnabled: false });
    setUnlockStatus(String(error.message || error), "error");
  }
}

export function hideHomeUnlock() {
  if (!unlockPanel) {
    return;
  }
  const finish = () => {
    unlockPanel.hidden = true;
    unlockPanel.setAttribute("aria-hidden", "true");
    unlockPanel.classList.remove("home-unlock-leaving");
    delete unlockPanel.dataset.mode;
    delete unlockPanel.dataset.surface;
    delete unlockPanel.dataset.flow;
    unlockPanel.style.removeProperty("--home-unlock-ground");
    setUnlockNameVisible(false);
    setUnlockStatus("", "muted");
    stopUnlockClock();
  };
  if (unlockPanel.hidden || prefersReducedMotion()) {
    finish();
    return;
  }
  cancelUnlockLeave();
  unlockPanel.classList.add("home-unlock-leaving");
  unlockLeaveTimer = window.setTimeout(() => {
    unlockLeaveTimer = 0;
    finish();
  }, 320);
}

export function bindHomeUnlock() {
  for (const [control, purpose] of [[enrollmentCreate, "create"], [enrollmentRecover, "recover"]]) {
    control?.addEventListener("change", () => {
      if (busy || pendingRegistration || !control.checked) return;
      enrollmentPurpose = purpose;
      renderEnrollmentChoice();
    });
  }
  enrollmentDismiss?.addEventListener("click", () => {
    if (busy || !pendingRegistrationInvalid) return;
    try {
      window.sessionStorage.removeItem(PENDING_REGISTRATION_KEY);
      pendingRegistration = null;
      pendingRegistrationInvalid = false;
      showHomeUnlock(unlockCallback).catch(reportUnlockError);
    } catch (_) {
      setUnlockStatus("Home cannot clear setup retry data. Check browser storage and try again.", "error");
    }
  });
  const startUnlock = () => {
    if (["create", "create_guest", "resume_owner", "resume_registration"].includes(unlockMode)) {
      runPasskeyCreate().catch(reportUnlockError);
      return;
    }
    runPasskeySignIn().catch(reportUnlockError);
  };
  unlockPrimary?.addEventListener("click", startUnlock);
  unlockPerson?.addEventListener("click", startUnlock);
  unlockSecondary?.addEventListener("click", () => {
    if (unlockMode === "resume_registration") {
      unlockMode = guestRegistrationAvailable ? "signin_guest_enabled" : "signin";
      renderUnlockMode({ registered: true, guestRegistrationEnabled: guestRegistrationAvailable });
      return;
    }
    if (unlockMode === "signin_guest_enabled") {
      unlockMode = "create_guest";
      renderUnlockMode({ registered: true, guestRegistrationEnabled: true });
      return;
    }
    if (unlockMode === "create_guest") {
      unlockMode = "signin_guest_enabled";
      renderUnlockMode({ registered: true, guestRegistrationEnabled: true });
      return;
    }
    runPasskeySignIn().catch(reportUnlockError);
  });
}

export function refreshHomeSession() {
  if (!sessionRefreshInFlight) {
    sessionRefreshInFlight = fetchJson("/api/auth/sessions/refresh", { method: "POST" })
      .then((response) => {
        setHomeAuthorityToken(response?.home_token);
        return response;
      })
      .catch((error) => {
        clearHomeAuthorityToken();
        throw error;
      })
      .finally(() => {
        sessionRefreshInFlight = null;
      });
  }
  return sessionRefreshInFlight;
}

export async function signOutHome() {
  try {
    const response = await fetch("/api/auth/sessions/sign-out", {
      method: "POST",
      headers: { "content-type": "application/json" },
    });
    if (response.ok || response.status === 401 || response.status === 403) {
      return null;
    }
    const detail = await response.text().catch(() => "");
    throw new Error(`request failed: ${response.status} ${response.statusText}${detail ? ` ${detail}` : ""}`);
  } finally {
    clearHomeAuthorityToken();
  }
}

export async function requestPasskeyStepUp(appToken, operation, request) {
  if (!window.PublicKeyCredential) {
    throw new Error("Passkey verification is unavailable in this browser.");
  }
  let ceremonyId = "";
  try {
    const begin = await fetchJson("/api/auth/passkey-step-up/begin", {
      method: "POST",
      body: JSON.stringify({
        schema: "elastos.auth.passkey-step-up.begin.request/v1",
        app_token: appToken,
        operation,
        request,
      }),
    });
    ceremonyId = readText(begin?.ceremony_id);
    if (
      begin?.schema !== "elastos.auth.passkey-step-up.begin.result/v1"
      || !ceremonyId
      || !begin?.options
    ) {
      throw new Error("Passkey verification returned an invalid challenge.");
    }
    const credential = await navigator.credentials.get(toRequestOptions(begin.options));
    if (!credential) {
      throw new Error("Passkey verification was cancelled.");
    }
    const response = await fetchJson("/api/auth/passkey-step-up/complete", {
      method: "POST",
      body: JSON.stringify({
        schema: "elastos.auth.passkey-step-up.complete.request/v1",
        ceremony_id: ceremonyId,
        response: serializeAssertionCredential(credential),
      }),
    });
    const stepUpToken = readText(response?.step_up_token);
    if (
      response?.schema !== "elastos.auth.passkey-step-up.complete.result/v1"
      || !stepUpToken
    ) {
      throw new Error("Passkey verification did not return step-up proof.");
    }
    ceremonyId = "";
    return stepUpToken;
  } catch (error) {
    if (ceremonyId) {
      await fetchJson("/api/auth/passkey-step-up/cancel", {
        method: "POST",
        body: JSON.stringify({
          schema: "elastos.auth.passkey-step-up.cancel.request/v1",
          ceremony_id: ceremonyId,
        }),
      }).catch(() => {});
    }
    throw error;
  }
}

function renderUnlockChecking() {
  if (unlockTitle) {
    unlockTitle.textContent = "Sign in";
  }
  if (unlockCopy) {
    unlockCopy.textContent = "Use your passkey to unlock your data, apps and desktop.";
  }
  if (unlockPrimary) {
    unlockPrimary.textContent = "Use passkey";
    unlockPrimary.disabled = true;
  }
  if (unlockSecondary) {
    unlockSecondary.hidden = true;
  }
  setUnlockNameVisible(false);
  if (unlockFace) {
    unlockFace.hidden = unlockPanel?.dataset.surface !== "lock-face";
  }
  if (unlockCard) {
    unlockCard.hidden = unlockPanel?.dataset.surface === "lock-face";
  }
  if (unlockPanel?.dataset.surface === "lock-face") {
    startUnlockClock();
  } else {
    stopUnlockClock();
  }
  setUnlockStatus("One moment.", "muted");
}

function renderUnlockMode({ registered, guestRegistrationEnabled }) {
  const resumingOwner = unlockMode === "resume_owner";
  const resumingRegistration = unlockMode === "resume_registration";
  const creatingGuest = unlockMode === "create_guest";
  const creatingAdmin = unlockMode === "create" || resumingOwner;
  const canCreate = (creatingAdmin || creatingGuest) && !resumingOwner;
  const showFace = registered && !creatingGuest && !resumingOwner && !resumingRegistration && unlockMode !== "unsupported";
  if (unlockTitle) {
    unlockTitle.textContent = resumingOwner || resumingRegistration ? "Resume setup" : creatingGuest ? "Create guest account" : (registered ? "Sign in" : "Set up Home");
  }
  if (unlockCopy) {
    if (resumingOwner) {
      unlockCopy.textContent = "Finish the passkey setup already verified by this Home.";
    } else if (creatingGuest) {
      unlockCopy.textContent = "Use a passkey to create your own guest account.";
    } else {
      unlockCopy.textContent = registered
        ? "Use your passkey to unlock your data, apps and desktop."
        : "Create the admin passkey for this Home.";
    }
  }
  if (unlockPrimary) {
    unlockPrimary.textContent = resumingOwner ? "Resume Home setup" : creatingGuest
      ? "Create guest passkey"
      : (registered ? "Use passkey" : "Create admin passkey");
    unlockPrimary.disabled = unlockMode === "unsupported";
  }
  if (unlockSecondary) {
    unlockSecondary.hidden = !registered || !guestRegistrationEnabled && !resumingRegistration;
    unlockSecondary.textContent = creatingGuest || resumingRegistration ? "Back to sign in" : "Create guest account";
  }
  if (unlockPanel) {
    unlockPanel.dataset.surface = showFace ? "lock-face" : "neutral";
    if (showFace) {
      unlockPanel.dataset.flow = "picker";
      unlockPanel.style.setProperty("--home-unlock-ground", 'url("/apps/home-gui/wallpaper.webp")');
    } else {
      delete unlockPanel.dataset.flow;
      unlockPanel.style.removeProperty("--home-unlock-ground");
    }
  }
  if (unlockFace) {
    unlockFace.hidden = !showFace;
  }
  if (unlockCard) {
    unlockCard.hidden = showFace;
  }
  if (unlockPersonName) {
    unlockPersonName.textContent = unlockPersonLabel;
    unlockPersonName.hidden = !unlockPersonLabel;
  }
  if (unlockMonogram) {
    unlockMonogram.textContent = "e";
  }
  if (unlockPerson) {
    unlockPerson.disabled = unlockMode === "unsupported";
    const personLabel = unlockPersonLabel
      ? `Use passkey for ${unlockPersonLabel}`
      : "Use passkey";
    unlockPerson.setAttribute("aria-label", personLabel);
    unlockPerson.title = personLabel;
  }
  if (showFace) {
    startUnlockClock();
  } else {
    stopUnlockClock();
  }
  if (enrollmentChoice) enrollmentChoice.hidden = !(canCreate || resumingOwner || resumingRegistration);
  renderEnrollmentChoice();
  const publicOwnerSetup = creatingAdmin && window.location.protocol === "https:";
  if (ownerToken) {
    ownerToken.hidden = !publicOwnerSetup;
    if (!publicOwnerSetup) ownerToken.value = "";
  }
  if (ownerTokenLabel) ownerTokenLabel.hidden = !publicOwnerSetup;
}

function unlockStatusCopy(registered, guestRegistrationEnabled) {
  if (!registered) {
    return "First passkey becomes admin.";
  }
  if (unlockPresentation === "prompt") {
    return "";
  }
  return "";
}

function boundedString(value, max) {
  return typeof value === "string" && value.length > 0 && value.length <= max
    && new TextEncoder().encode(value).length <= max;
}

function exactKeys(value, keys) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(",");
}

function validatePendingRegistration(pending) {
  const validName = name => boundedString(name, 64)
    && new TextEncoder().encode(name).length <= 64
    && !/[\x00-\x1f\x7f-\x9f/\\]/.test(name) && name === name.trim().replace(/\s+/g, " ");
  const intent = pending?.intent;
  const validIntent = exactKeys(intent, ["purpose"]) && intent.purpose === "recover"
    || exactKeys(intent, ["purpose", "public_name"]) && intent.purpose === "create" && validName(intent.public_name);
  const response = pending?.response;
  const validResponse = response === null || exactKeys(response, ["id", "rawId", "type", "response"])
    && response.type === "public-key" && boundedString(response.id, 8192) && boundedString(response.rawId, 8192)
    && exactKeys(response.response, ["clientDataJson", "attestationObject"])
    && boundedString(response.response.clientDataJson, 4096) && boundedString(response.response.attestationObject, 32768);
  if (!exactKeys(pending, ["schema", "created_at", "expires_at", "intent", "ceremony_id", "response"])
    || pending.schema !== PENDING_REGISTRATION_KEY || !validIntent || !validResponse
    || !(pending.ceremony_id === null || boundedString(pending.ceremony_id, 128))
    || response !== null && pending.ceremony_id === null
    || !Number.isSafeInteger(pending.created_at) || pending.created_at < 0 || pending.created_at > Date.now()
    || !Number.isSafeInteger(pending.expires_at) || pending.expires_at - pending.created_at !== PENDING_REGISTRATION_MS) {
    pendingRegistrationInvalid = true;
    throw new Error("Saved Home setup is invalid. Dismiss it, then use your existing passkey or start setup.");
  }
  if (pending.expires_at <= Date.now()) {
    pendingRegistrationInvalid = true;
    throw new Error("Saved Home setup expired. Dismiss it, then use your existing passkey or start setup.");
  }
  return pending;
}

function readPendingRegistration() {
  let raw;
  try { raw = window.sessionStorage.getItem(PENDING_REGISTRATION_KEY); }
  catch (_) { throw new Error("Home cannot read setup retry data. Check browser storage and try again."); }
  if (raw === null) return null;
  if (typeof raw !== "string" || raw.length > MAX_PENDING_REGISTRATION_CHARS) {
    pendingRegistrationInvalid = true;
    throw new Error("Saved Home setup is invalid. Dismiss it before starting another attempt.");
  }
  let pending;
  try { pending = JSON.parse(raw); }
  catch (_) {
    pendingRegistrationInvalid = true;
    throw new Error("Saved Home setup is invalid. Dismiss it before starting another attempt.");
  }
  return validatePendingRegistration(pending);
}

function savePendingRegistration(pending) {
  validatePendingRegistration(pending);
  const raw = JSON.stringify(pending);
  if (raw.length > MAX_PENDING_REGISTRATION_CHARS) throw new Error("Home setup retry data is too large.");
  try { window.sessionStorage.setItem(PENDING_REGISTRATION_KEY, raw); }
  catch (_) { throw new Error("Home cannot save setup retry data. Check browser storage and retry this attempt."); }
}

function renderEnrollmentChoice() {
  const enrolling = ["create", "create_guest", "resume_owner", "resume_registration"].includes(unlockMode);
  const purpose = pendingRegistration?.intent.purpose || enrollmentPurpose;
  if (enrollmentCreate) { enrollmentCreate.checked = purpose === "create"; enrollmentCreate.disabled = busy || !!pendingRegistration; }
  if (enrollmentRecover) { enrollmentRecover.checked = purpose === "recover"; enrollmentRecover.disabled = busy || !!pendingRegistration; }
  if (enrollmentDismiss) enrollmentDismiss.hidden = !pendingRegistrationInvalid;
  setUnlockNameVisible(enrolling && purpose === "create");
  if (unlockName && pendingRegistration) unlockName.disabled = true;
  if (enrolling && unlockCopy) unlockCopy.textContent = purpose === "recover"
    ? "Create a passkey, then choose your Recovery Kit in System. Your existing identity will be restored from the kit."
    : "Create a passkey and Profile with the name people will see.";
  if (enrolling && pendingRegistration && unlockPrimary) unlockPrimary.textContent = "Resume setup";
  if (enrolling && pendingRegistration && unlockCopy) unlockCopy.textContent = purpose === "recover"
    ? "Resume your saved Recover setup, then choose your Recovery Kit in System."
    : `Resume your saved Create setup for ${pendingRegistration.intent.public_name}.`;
}

async function runPasskeyCreate() {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  busy = true;
  setButtonsDisabled(true);
  setUnlockStatus("Creating passkey", "muted");
  try {
    if (!pendingRegistration) pendingRegistration = readPendingRegistration();
    const displayName = readUnlockName();
    if (!pendingRegistration && enrollmentPurpose === "create" && !displayName) {
      unlockName?.focus();
      throw new Error("Enter the display name people will see.");
    }
    if (!pendingRegistration && enrollmentPurpose === "create" && (new TextEncoder().encode(displayName).length > 64
      || /[\x00-\x1f\x7f-\x9f/\\]/.test(displayName))) {
      unlockName?.focus();
      throw new Error("Use a shorter display name without slashes or control characters.");
    }
    if (!pendingRegistration) {
      const now = Date.now();
      pendingRegistration = { schema: PENDING_REGISTRATION_KEY, created_at: now,
        expires_at: now + PENDING_REGISTRATION_MS,
        intent: enrollmentPurpose === "recover" ? { purpose: "recover" } : { purpose: "create", public_name: displayName },
        ceremony_id: null, response: null };
    }
    const pending = pendingRegistration;
    savePendingRegistration(pending);
    renderEnrollmentChoice();
    if (!pending.response) {
      const enrollmentHeaders = { "content-type": "application/json" };
      if ((unlockMode === "create" || unlockMode === "resume_owner") && ownerToken?.value) {
        enrollmentHeaders["x-elastos-owner-enrollment"] = ownerToken.value.trim();
      }
      const beginPromise = fetchJson("/api/auth/passkey/register/begin", { method: "POST", headers: enrollmentHeaders, body: JSON.stringify({ intent: pending.intent }) });
      if (ownerToken) ownerToken.value = "";
      let begin;
      try {
        begin = await beginPromise;
      } catch (error) {
        if (error.status === 422) {
          try { window.sessionStorage.removeItem(PENDING_REGISTRATION_KEY); }
          catch (_) { throw new Error("Home cannot clear setup retry data. Check browser storage and retry."); }
          pendingRegistration = null;
          if (pending.intent.purpose === "create") throw new Error("Choose a different display name.");
        }
        throw error;
      }
      if (begin?.schema !== "elastos.auth.passkey.register.begin/v1" || !boundedString(begin.ceremony_id, 128)) {
        throw new Error("Home setup returned an invalid response.");
      }
      pending.ceremony_id = begin.ceremony_id;
      savePendingRegistration(pending);
      if (begin.options !== null) {
        if (!begin.options?.publicKey?.user) throw new Error("Home setup returned an invalid response.");
        const label = pending.intent.public_name || "ElastOS Home";
        begin.options.publicKey.user.name = label;
        begin.options.publicKey.user.displayName = label;
        let credential;
        try {
          credential = await navigator.credentials.create(toCreationOptions(begin.options));
          if (!credential) throw new Error("Passkey creation was cancelled.");
        } catch (error) {
          // No attestation or completion was sent; an explicit retry may begin
          // again with the same intent and original retention deadline.
          pending.ceremony_id = null;
          savePendingRegistration(pending);
          throw error;
        }
        pending.response = serializeCreatedCredential(credential);
        savePendingRegistration(pending);
      } else if (unlockMode !== "resume_owner") {
        throw new Error("Home setup returned an invalid response.");
      }
    }
    const completion = { ceremony_id: pending.ceremony_id, intent: pending.intent };
    if (pending.response) completion.response = pending.response;
    const response = await fetchJson("/api/auth/passkey/register/complete", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(completion),
    });
    if (response?.schema !== "elastos.auth.passkey.verify/v2"
      || ![response.principal_id, response.session_id, response.home_token].every(value => boundedString(value, 32768))) {
      throw new Error("Home setup returned an invalid completion.");
    }
    try { window.sessionStorage.removeItem(PENDING_REGISTRATION_KEY); }
    catch (_) { throw new Error("Home cannot clear setup retry data. Check browser storage and retry."); }
    pendingRegistration = null;
    setHomeAuthorityToken(response?.home_token);
    await unlockComplete(response, { enrollmentPurpose: pending.intent.purpose });
  } finally {
    busy = false;
    setButtonsDisabled(false);
    renderEnrollmentChoice();
  }
}

async function runPasskeySignIn() {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  busy = true;
  setButtonsDisabled(true);
  setUnlockStatus("Choose your passkey.", "muted");
  try {
    const begin = await fetchJson("/api/auth/passkey/authenticate/begin", { method: "POST" });
    const credential = await navigator.credentials.get(toRequestOptions(begin.options));
    if (!credential) {
      throw new Error("Passkey sign-in was cancelled.");
    }
    const response = await fetchJson("/api/auth/passkey/authenticate/complete", {
      method: "POST",
      body: JSON.stringify({
        ceremony_id: begin.ceremony_id,
        response: serializeAssertionCredential(credential),
      }),
    });
    setHomeAuthorityToken(response?.home_token);
    await unlockComplete(response);
  } finally {
    busy = false;
    setButtonsDisabled(false);
  }
}

async function unlockComplete(response, flow = null) {
  setUnlockStatus("Opening Home", "success");
  if (unlockCallback) {
    await unlockCallback(response, flow);
    return;
  }
  hideHomeUnlock();
}

export function profileReadinessActionTarget(response) {
  const readiness = response && typeof response.profile_readiness === "object"
    ? response.profile_readiness
    : null;
  if (readiness?.schema !== "elastos.profile.readiness/v1" || readiness.status !== "ready") {
    return "system";
  }
  return "";
}

function reportUnlockError(error) {
  if (isPasskeyNotSelected(error) && unlockMode === "signin_guest_enabled") {
    renderUnlockMode({ registered: true, guestRegistrationEnabled: true });
    setUnlockStatus("No passkey selected.", "muted");
    return;
  }
  setUnlockStatus(String(error.message || error), "error");
}

function setButtonsDisabled(disabled) {
  if (unlockPerson) {
    unlockPerson.disabled = disabled || unlockMode === "unsupported";
  }
  if (unlockPrimary) {
    unlockPrimary.disabled = disabled || unlockMode === "unsupported";
  }
  if (unlockSecondary) {
    unlockSecondary.disabled = disabled;
  }
  if (enrollmentDismiss) enrollmentDismiss.disabled = disabled;
}

function isPasskeyNotSelected(error) {
  const name = String(error && error.name || "");
  const message = String(error && error.message || error || "");
  return name === "NotAllowedError"
    || message.includes("timed out or was not allowed")
    || message.includes("Passkey sign-in was cancelled");
}

function setUnlockNameVisible(visible) {
  if (!unlockName) {
    return;
  }
  unlockName.hidden = !visible;
  unlockName.disabled = !visible;
  for (const id of ["home-unlock-name-label", "home-unlock-name-hint"]) {
    const element = document.getElementById(id);
    if (element) element.hidden = !visible;
  }
  if (visible) {
    unlockName.placeholder = "Your name";
  }
}

function readUnlockName() {
  return String(unlockName?.value || "")
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
}

function setUnlockStatus(message, tone) {
  if (!unlockStatus) {
    return;
  }
  unlockStatus.textContent = message;
  unlockStatus.hidden = !message;
  unlockStatus.dataset.tone = tone || "muted";
}

function readUnlockPersonLabel(value) {
  return String(value || "")
    .trim()
    .replace(/\s+/g, " ")
    .slice(0, 64);
}

function cancelUnlockLeave() {
  if (unlockLeaveTimer) {
    window.clearTimeout(unlockLeaveTimer);
    unlockLeaveTimer = 0;
  }
  unlockPanel?.classList.remove("home-unlock-leaving");
}

function prefersReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
}

function startUnlockClock() {
  updateUnlockClock();
  if (unlockClockTimer) {
    return;
  }
  unlockClockTimer = window.setInterval(updateUnlockClock, 30_000);
}

function stopUnlockClock() {
  if (!unlockClockTimer) {
    return;
  }
  window.clearInterval(unlockClockTimer);
  unlockClockTimer = 0;
}

function updateUnlockClock() {
  const now = new Date();
  if (unlockDate) {
    unlockDate.textContent = formatUnlockDate(now);
  }
  if (unlockTime) {
    unlockTime.textContent = formatUnlockTime(now);
  }
}

function formatUnlockDate(value) {
  return new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
  }).format(value);
}

function formatUnlockTime(value) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  }).format(value);
}

function toCreationOptions(options) {
  const publicKey = { ...(options && options.publicKey ? options.publicKey : {}) };
  publicKey.challenge = base64UrlToBuffer(publicKey.challenge);
  publicKey.user = {
    ...publicKey.user,
    id: base64UrlToBuffer(publicKey.user && publicKey.user.id),
  };
  publicKey.excludeCredentials = (publicKey.excludeCredentials || []).map((credential) => ({
    ...credential,
    id: base64UrlToBuffer(credential.id),
  }));
  return { publicKey };
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

function serializeCreatedCredential(credential) {
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJson: bufferToBase64Url(credential.response.clientDataJSON),
      attestationObject: bufferToBase64Url(credential.response.attestationObject),
    },
  };
}

function serializeAssertionCredential(credential) {
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJson: bufferToBase64Url(credential.response.clientDataJSON),
      authenticatorData: bufferToBase64Url(credential.response.authenticatorData),
      signature: bufferToBase64Url(credential.response.signature),
      userHandle: credential.response.userHandle
        ? bufferToBase64Url(credential.response.userHandle)
        : null,
    },
  };
}

function base64UrlToBuffer(value) {
  const text = readText(value);
  const padded = `${text.replace(/-/g, "+").replace(/_/g, "/")}${"=".repeat((4 - (text.length % 4)) % 4)}`;
  const binary = window.atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

function bufferToBase64Url(buffer) {
  const bytes = new Uint8Array(buffer || new ArrayBuffer(0));
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return window.btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}
