import {
  clearHomeAuthorityToken,
  fetchJson,
  setHomeAuthorityToken,
} from "./shell-core.js?v=home-20260802a";

const unlockPanel = document.querySelector("#home-unlock");
const unlockFace = document.querySelector(".home-unlock-face");
const unlockCard = document.querySelector(".home-unlock-card");
const unlockTitle = document.querySelector("#home-unlock-title");
const unlockCopy = document.querySelector("#home-unlock-copy");
const unlockPrimary = document.querySelector("#home-unlock-primary");
const unlockSecondary = document.querySelector("#home-unlock-secondary");
const unlockStatus = document.querySelector("#home-unlock-status");
const unlockName = document.querySelector("#home-unlock-name");
const unlockDate = document.querySelector("#home-unlock-date");
const unlockClock = document.querySelector("#home-unlock-clock");
const unlockTime = document.querySelector("#home-unlock-time");
const unlockPerson = document.querySelector("#home-unlock-person");
const unlockPersonName = document.querySelector("#home-unlock-person-name");
const unlockMonogram = document.querySelector("#home-unlock-monogram");

let unlockMode = "signin";
let unlockPresentation = "modal";
let unlockCallback = null;
let busy = false;
let sessionRefreshInFlight = null;
let autoSignInAttempted = false;
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
  const forceNeutralSurface = options && options.surface === "neutral";
  if (!unlockPanel) {
    throw new Error("Home unlock surface is missing");
  }
  unlockPersonLabel = typeof options.personName === "string" ? options.personName.trim() : "";
  cancelUnlockLeave();
  document.body.dataset.homeStatus = unlockPresentation === "prompt" ? "ready" : "locked";
  unlockPanel.dataset.mode = unlockPresentation;
  unlockPanel.dataset.surface = forceNeutralSurface
    ? "neutral"
    : "lock-face";
  if (!forceNeutralSurface) {
    unlockPanel.dataset.flow = "picker";
  } else {
    delete unlockPanel.dataset.flow;
  }
  unlockPanel.style.setProperty(
    "--home-unlock-ground",
    'url("/apps/home-gui/wallpaper.webp")',
  );
  renderUnlockChecking();
  unlockPanel.hidden = false;
  unlockPanel.setAttribute("aria-hidden", "false");
  unlockCard?.setAttribute("aria-modal", "true");
  startUnlockClock();

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
    const accounts = Array.isArray(status.accounts) ? status.accounts : [];
    const accountName = typeof accounts[0]?.display_name === "string"
      ? accounts[0].display_name.trim()
      : "";
    if (accountName) {
      unlockPersonLabel = accountName;
    }
    unlockMode = registered
      ? (guestRegistrationEnabled ? "signin_guest_enabled" : "signin")
      : "create";
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
  const reducedMotion = typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!unlockPanel.hidden && !reducedMotion) {
    unlockPanel.classList.add("home-unlock-leaving");
    if (unlockLeaveTimer) {
      window.clearTimeout(unlockLeaveTimer);
    }
    unlockLeaveTimer = window.setTimeout(finishHideHomeUnlock, 320);
    return;
  }
  finishHideHomeUnlock();
}

function cancelUnlockLeave() {
  if (unlockLeaveTimer) {
    window.clearTimeout(unlockLeaveTimer);
    unlockLeaveTimer = 0;
  }
  unlockPanel?.classList.remove("home-unlock-leaving");
}

function finishHideHomeUnlock() {
  unlockLeaveTimer = 0;
  if (!unlockPanel) {
    return;
  }
  unlockPanel.classList.remove("home-unlock-leaving");
  unlockPanel.hidden = true;
  unlockPanel.setAttribute("aria-hidden", "true");
  delete unlockPanel.dataset.mode;
  delete unlockPanel.dataset.surface;
  delete unlockPanel.dataset.flow;
  setUnlockNameVisible(false);
  setUnlockStatus("", "muted");
  stopUnlockClock();
}

export function bindHomeUnlock() {
  const startUnlock = () => {
    if (unlockMode === "create" || unlockMode === "create_guest") {
      runPasskeyCreate().catch(reportUnlockError);
      return;
    }
    runPasskeySignIn().catch(reportUnlockError);
  };
  unlockPrimary?.addEventListener("click", startUnlock);
  unlockPerson?.addEventListener("click", startUnlock);
  unlockSecondary?.addEventListener("click", () => {
    if (unlockMode === "signin_guest_enabled") {
      unlockMode = "create_guest";
      renderUnlockMode({ registered: true, guestRegistrationEnabled: true });
      return;
    }
    if (unlockMode === "create_guest") {
      unlockMode = "signin_guest_enabled";
      autoSignInAttempted = false;
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
  autoSignInAttempted = false;
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
  setUnlockStatus("One moment.", "muted");
  if (unlockPanel?.dataset.surface === "lock-face") {
    if (unlockFace) {
      unlockFace.hidden = false;
    }
    if (unlockCard) {
      unlockCard.hidden = true;
    }
    startUnlockClock();
  }
}

function renderUnlockMode({ registered, guestRegistrationEnabled }) {
  const creatingGuest = unlockMode === "create_guest";
  const creatingAdmin = unlockMode === "create";
  const canCreate = creatingAdmin || creatingGuest;
  if (unlockTitle) {
    unlockTitle.textContent = creatingGuest ? "Create guest account" : (registered ? "Sign in" : "Set up Home");
  }
  if (unlockCopy) {
    if (creatingGuest) {
      unlockCopy.textContent = "Use a passkey to create your own guest account.";
    } else {
      unlockCopy.textContent = registered
        ? "Use your passkey to unlock your data, apps and desktop."
        : "Create the admin passkey for this Home.";
    }
  }
  if (unlockPrimary) {
    unlockPrimary.textContent = creatingGuest
      ? "Create guest passkey"
      : (registered ? "Use passkey" : "Create admin passkey");
    unlockPrimary.disabled = unlockMode === "unsupported";
  }
  if (unlockSecondary) {
    unlockSecondary.hidden = !registered || !guestRegistrationEnabled;
    unlockSecondary.textContent = creatingGuest ? "Back to sign in" : "Create guest account";
  }
  setUnlockNameVisible(canCreate);
  const showFace = registered && !creatingGuest && unlockMode !== "unsupported";
  if (unlockPanel && !unlockPanel.hidden) {
    unlockPanel.dataset.surface = showFace ? "lock-face" : "neutral";
    if (showFace) {
      unlockPanel.dataset.flow = "picker";
    } else {
      delete unlockPanel.dataset.flow;
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
  }
  if (unlockMonogram) {
    unlockMonogram.textContent = "e";
  }
  if (unlockPerson) {
    unlockPerson.setAttribute(
      "aria-label",
      unlockPersonLabel ? `Unlock ${unlockPersonLabel}` : "Unlock with passkey",
    );
  }
  if (showFace) {
    startUnlockClock();
  } else {
    stopUnlockClock();
  }
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

function formatUnlockDate(now) {
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(now);
  const day = new Intl.DateTimeFormat(undefined, { day: "numeric" }).format(now);
  const month = new Intl.DateTimeFormat(undefined, { month: "short" }).format(now);
  return `${weekday} ${day} ${month}`;
}

function formatUnlockTime(now) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  }).format(now);
}

function startUnlockClock() {
  if (!unlockDate && !unlockTime) {
    return;
  }
  const tick = () => {
    const now = new Date();
    if (unlockDate) {
      unlockDate.textContent = formatUnlockDate(now);
    }
    if (unlockTime) {
      unlockTime.textContent = formatUnlockTime(now);
    }
  };
  tick();
  if (!unlockClockTimer) {
    unlockClockTimer = window.setInterval(tick, 15_000);
  }
}

function stopUnlockClock() {
  if (unlockClockTimer) {
    window.clearInterval(unlockClockTimer);
    unlockClockTimer = 0;
  }
}

async function runPasskeyCreate() {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  busy = true;
  setButtonsDisabled(true);
  setUnlockStatus("Creating passkey", "muted");
  try {
    const displayName = readUnlockName();
    if (!displayName) {
      throw new Error("Enter a name for this passkey.");
    }
    const begin = await fetchJson("/api/auth/passkey/register/begin", { method: "POST" });
    begin.options.publicKey.user.name = displayName;
    begin.options.publicKey.user.displayName = displayName;
    const credential = await navigator.credentials.create(toCreationOptions(begin.options));
    if (!credential) {
      throw new Error("Passkey creation was cancelled.");
    }
    const response = await fetchJson("/api/auth/passkey/register/complete", {
      method: "POST",
      body: JSON.stringify({
        ceremony_id: begin.ceremony_id,
        response: serializeCreatedCredential(credential),
        display_name: displayName,
      }),
    });
    setHomeAuthorityToken(response?.home_token);
    await unlockComplete(response);
  } finally {
    busy = false;
    setButtonsDisabled(false);
  }
}

async function runPasskeySignIn(options = {}) {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  busy = true;
  setButtonsDisabled(true);
  setUnlockStatus(options.automatic ? "Choose your passkey." : "Waiting for passkey", "muted");
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

async function unlockComplete(response) {
  setUnlockStatus("Opening Home", "success");
  if (unlockCallback) {
    await unlockCallback(response);
    return;
  }
  hideHomeUnlock();
}

export function profileReadinessActionTarget(response) {
  const readiness = response && typeof response.profile_readiness === "object"
    ? response.profile_readiness
    : null;
  if (readiness?.schema !== "elastos.profile.readiness/v1") {
    return "system";
  }
  if (readiness.status === "setup_required") {
    return "people";
  }
  if (readiness.status === "unavailable") {
    return "system";
  }
  return readiness.status === "ready" ? "" : "system";
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
  if (unlockPrimary) {
    unlockPrimary.disabled = disabled || unlockMode === "unsupported";
  }
  if (unlockPerson) {
    unlockPerson.disabled = disabled || unlockMode === "unsupported";
  }
  if (unlockSecondary) {
    unlockSecondary.disabled = disabled && unlockMode !== "signin_guest_enabled";
  }
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
