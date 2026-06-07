import { fetchJson } from "./shell-core.js?v=home-20260607d";

const unlockPanel = document.querySelector("#home-unlock");
const unlockCard = document.querySelector(".home-unlock-card");
const unlockTitle = document.querySelector("#home-unlock-title");
const unlockCopy = document.querySelector("#home-unlock-copy");
const unlockPrimary = document.querySelector("#home-unlock-primary");
const unlockSecondary = document.querySelector("#home-unlock-secondary");
const unlockStatus = document.querySelector("#home-unlock-status");
const unlockName = document.querySelector("#home-unlock-name");

let unlockMode = "signin";
let unlockPresentation = "modal";
let unlockCallback = null;
let busy = false;
let sessionRefreshInFlight = null;
let autoSignInAttempted = false;

export function isHomeAuthError(error) {
  const status = Number(error && error.status);
  return status === 401 || status === 403;
}

export async function showHomeUnlock(onUnlocked, options = {}) {
  unlockCallback = typeof onUnlocked === "function" ? onUnlocked : null;
  unlockPresentation = options && options.presentation === "prompt" ? "prompt" : "modal";
  if (!unlockPanel) {
    throw new Error("Home unlock surface is missing");
  }
  document.body.dataset.homeStatus = unlockPresentation === "prompt" ? "ready" : "locked";
  unlockPanel.dataset.mode = unlockPresentation;
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
    unlockMode = registered
      ? (guestRegistrationEnabled ? "signin_guest_enabled" : "signin")
      : "create";
    renderUnlockMode({ registered, guestRegistrationEnabled });
    setUnlockStatus(unlockStatusCopy(registered, guestRegistrationEnabled), "muted");
    if (registered) {
      startAutomaticPasskeySignIn({ guestRegistrationEnabled });
    }
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
  unlockPanel.hidden = true;
  unlockPanel.setAttribute("aria-hidden", "true");
  delete unlockPanel.dataset.mode;
  setUnlockNameVisible(false);
  setUnlockStatus("", "muted");
}

export function bindHomeUnlock() {
  unlockPrimary?.addEventListener("click", () => {
    if (unlockMode === "create" || unlockMode === "create_guest") {
      runPasskeyCreate().catch(reportUnlockError);
      return;
    }
    runPasskeySignIn().catch(reportUnlockError);
  });
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
      startAutomaticPasskeySignIn({ guestRegistrationEnabled: true });
      return;
    }
    runPasskeySignIn().catch(reportUnlockError);
  });
}

export function refreshHomeSession() {
  if (!sessionRefreshInFlight) {
    sessionRefreshInFlight = fetchJson("/api/auth/sessions/refresh", { method: "POST" })
      .finally(() => {
        sessionRefreshInFlight = null;
      });
  }
  return sessionRefreshInFlight;
}

export async function signOutHome() {
  const response = await fetch("/api/auth/sessions/sign-out", {
    method: "POST",
    headers: { "content-type": "application/json" },
  });
  if (response.ok || response.status === 401 || response.status === 403) {
    return null;
  }
  const detail = await response.text().catch(() => "");
  throw new Error(`request failed: ${response.status} ${response.statusText}${detail ? ` ${detail}` : ""}`);
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

async function startAutomaticPasskeySignIn({ guestRegistrationEnabled }) {
  if (autoSignInAttempted || busy || unlockMode === "unsupported") {
    return;
  }
  autoSignInAttempted = true;
  try {
    await runPasskeySignIn({ automatic: true });
  } catch (error) {
    if (unlockMode === "signin_guest_enabled" && guestRegistrationEnabled) {
      renderUnlockMode({ registered: true, guestRegistrationEnabled: true });
      setUnlockStatus("No passkey selected.", "muted");
      return;
    }
    reportUnlockError(error);
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
    await fetchJson("/api/auth/passkey/register/complete", {
      method: "POST",
      body: JSON.stringify({
        ceremony_id: begin.ceremony_id,
        response: serializeCreatedCredential(credential),
        display_name: displayName,
      }),
    });
    await unlockComplete();
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
    await fetchJson("/api/auth/passkey/authenticate/complete", {
      method: "POST",
      body: JSON.stringify({
        ceremony_id: begin.ceremony_id,
        response: serializeAssertionCredential(credential),
      }),
    });
    await unlockComplete();
  } finally {
    busy = false;
    setButtonsDisabled(false);
  }
}

async function unlockComplete() {
  setUnlockStatus("Opening Home", "success");
  if (unlockCallback) {
    await unlockCallback();
    return;
  }
  hideHomeUnlock();
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
