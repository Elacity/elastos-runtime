import {
  clearHomeAuthorityToken,
  fetchJson,
  setHomeAuthorityToken,
  trapTabWithin,
} from "./shell-core.js?v=home-20260719q";

const unlockPanel = document.querySelector("#home-unlock");
const unlockCard = document.querySelector(".home-unlock-card");
const unlockTitle = document.querySelector("#home-unlock-title");
const unlockCopy = document.querySelector("#home-unlock-copy");
const unlockClock = document.querySelector("#home-unlock-clock");
const unlockDate = document.querySelector("#home-unlock-date");
const unlockTime = document.querySelector("#home-unlock-time");
const unlockAccounts = document.querySelector("#home-unlock-accounts");
const unlockPrimary = document.querySelector("#home-unlock-primary");
const unlockSecondary = document.querySelector("#home-unlock-secondary");
const unlockAddGuest = document.querySelector("#home-unlock-add-guest");
const unlockStatus = document.querySelector("#home-unlock-status");
const unlockName = document.querySelector("#home-unlock-name");
const unlockActions = document.querySelector(".home-unlock-actions");

let unlockMode = "signin";
let unlockPresentation = "modal";
let unlockCallback = null;
let busy = false;
let sessionRefreshInFlight = null;
let passkeyAuthorityRequestInFlight = null;
let loginAccounts = [];
let selectedCredentialId = "";
let guestRegistrationEnabled = false;
let accountFocusIndex = 0;
let unlockClockTimer = 0;

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
  document.body.dataset.homeStatus = unlockPresentation === "prompt" ? "ready" : "locked";
  unlockPanel.dataset.mode = unlockPresentation;
  unlockPanel.dataset.surface = !forceNeutralSurface && document.body.dataset.homeShell === "desktop"
    ? "desktop"
    : "neutral";
  selectedCredentialId = "";
  loginAccounts = [];
  renderUnlockChecking();
  unlockPanel.hidden = false;
  unlockPanel.setAttribute("aria-hidden", "false");
  unlockCard?.setAttribute("aria-modal", "true");

  if (!window.PublicKeyCredential) {
    unlockMode = "unsupported";
    renderUnlockMode();
    setUnlockStatus("Passkeys are not available in this browser.", "error");
    return;
  }

  try {
    const status = await fetchJson("/api/auth/passkey/status");
    const registered = status.registered === true;
    guestRegistrationEnabled = status.guest_registration_enabled === true;
    loginAccounts = normalizeLoginAccounts(status.accounts);
    if (!registered) {
      unlockMode = "welcome";
    } else if (unlockPresentation === "prompt") {
      unlockMode = "prompt";
    } else {
      unlockMode = "picker";
    }
    renderUnlockMode();
    if (unlockMode === "picker" && loginAccounts.length === 0) {
      setUnlockStatus("Can’t load accounts. Retry after refreshing Home.", "error");
    } else {
      setUnlockStatus("", "muted");
    }
    if (unlockMode === "picker" && loginAccounts.length > 0) {
      focusAccountButton(0);
    } else {
      unlockPrimary?.focus();
    }
  } catch (error) {
    unlockMode = "picker";
    renderUnlockMode();
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
    window.setTimeout(() => {
      unlockPanel.classList.remove("home-unlock-leaving");
      finishHideHomeUnlock();
    }, 300);
    return;
  }
  finishHideHomeUnlock();
}

function finishHideHomeUnlock() {
  stopUnlockClock();
  unlockPanel.hidden = true;
  unlockPanel.setAttribute("aria-hidden", "true");
  unlockPanel.classList.remove("is-busy");
  delete unlockPanel.dataset.mode;
  delete unlockPanel.dataset.flow;
  delete unlockPanel.dataset.surface;
  selectedCredentialId = "";
  setUnlockNameVisible(false);
  setUnlockStatus("", "muted");
  if (unlockAccounts) {
    unlockAccounts.replaceChildren();
    unlockAccounts.hidden = true;
  }
  if (unlockCopy) {
    unlockCopy.hidden = false;
  }
  if (unlockClock) {
    unlockClock.hidden = true;
  }
}

export function bindHomeUnlock() {
  unlockPanel?.addEventListener("keydown", (event) => {
    if (unlockPanel.hidden) {
      return;
    }
    if (unlockMode === "picker" && handlePickerKeydown(event)) {
      return;
    }
    trapTabWithin(unlockCard, event);
  });
  unlockAccounts?.addEventListener("click", (event) => {
    const button = event.target?.closest?.("[data-credential-id]");
    if (!button || busy) {
      return;
    }
    const credentialId = readText(button.dataset.credentialId);
    if (!credentialId) {
      return;
    }
    selectedCredentialId = credentialId;
    syncAccountSelection();
    runPasskeySignIn({ credentialId }).catch(reportUnlockError);
  });
  unlockPrimary?.addEventListener("click", () => {
    if (unlockMode === "welcome") {
      unlockMode = "create";
      renderUnlockMode();
      unlockName?.focus();
      return;
    }
    if (unlockMode === "create" || unlockMode === "create_guest") {
      runPasskeyCreate().catch(reportUnlockError);
      return;
    }
    if (unlockMode === "prompt") {
      runPasskeySignIn().catch(reportUnlockError);
    }
  });
  unlockSecondary?.addEventListener("click", () => {
    if (unlockMode === "create_guest") {
      unlockMode = "picker";
      renderUnlockMode();
      focusAccountButton(accountFocusIndex);
      return;
    }
    if (unlockMode === "create") {
      unlockMode = "welcome";
      renderUnlockMode();
      unlockPrimary?.focus();
    }
  });
  unlockAddGuest?.addEventListener("click", () => {
    if (!guestRegistrationEnabled || busy) {
      return;
    }
    unlockMode = "create_guest";
    renderUnlockMode();
    unlockName?.focus();
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

export function requestPasskeyHomeAuthority() {
  if (!window.PublicKeyCredential) {
    return Promise.reject(new Error("Passkey verification is unavailable in this browser."));
  }
  if (!passkeyAuthorityRequestInFlight) {
    passkeyAuthorityRequestInFlight = (async () => {
      const begin = await fetchJson("/api/auth/passkey/authenticate/begin", {
        method: "POST",
        body: JSON.stringify({}),
      });
      const credential = await navigator.credentials.get(toRequestOptions(begin.options));
      if (!credential) {
        throw new Error("Passkey verification was cancelled.");
      }
      const response = await fetchJson("/api/auth/passkey/authenticate/complete", {
        method: "POST",
        body: JSON.stringify({
          ceremony_id: begin.ceremony_id,
          response: serializeAssertionCredential(credential),
        }),
      });
      const homeToken = typeof response?.home_token === "string"
        ? response.home_token.trim()
        : "";
      if (!homeToken) {
        throw new Error("Passkey verification did not return authority.");
      }
      setHomeAuthorityToken(homeToken);
      return homeToken;
    })().finally(() => {
      passkeyAuthorityRequestInFlight = null;
    });
  }
  return passkeyAuthorityRequestInFlight;
}

function renderUnlockChecking() {
  stopUnlockClock();
  if (unlockTitle) {
    unlockTitle.textContent = "Sign in";
  }
  if (unlockCopy) {
    unlockCopy.hidden = true;
    unlockCopy.textContent = "";
  }
  if (unlockClock) {
    unlockClock.hidden = true;
  }
  if (unlockPrimary) {
    unlockPrimary.textContent = "Use passkey";
    unlockPrimary.disabled = true;
  }
  if (unlockSecondary) {
    unlockSecondary.hidden = true;
  }
  if (unlockAddGuest) {
    unlockAddGuest.hidden = true;
  }
  if (unlockAccounts) {
    unlockAccounts.hidden = true;
    unlockAccounts.replaceChildren();
  }
  if (unlockActions) {
    unlockActions.hidden = false;
  }
  setUnlockNameVisible(false);
  setUnlockStatus("One moment.", "muted");
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

function tickUnlockClock() {
  const now = new Date();
  if (unlockDate) {
    unlockDate.textContent = formatUnlockDate(now);
  }
  if (unlockTime) {
    unlockTime.textContent = formatUnlockTime(now);
  }
}

function startUnlockClock() {
  if (!unlockClock) {
    return;
  }
  unlockClock.hidden = false;
  tickUnlockClock();
  if (unlockClockTimer) {
    window.clearInterval(unlockClockTimer);
  }
  unlockClockTimer = window.setInterval(tickUnlockClock, 1000);
}

function stopUnlockClock() {
  if (unlockClockTimer) {
    window.clearInterval(unlockClockTimer);
    unlockClockTimer = 0;
  }
  if (unlockClock) {
    unlockClock.hidden = true;
  }
}

function renderUnlockMode() {
  const creatingGuest = unlockMode === "create_guest";
  const creatingAdmin = unlockMode === "create";
  const welcoming = unlockMode === "welcome";
  const picking = unlockMode === "picker";
  const prompting = unlockMode === "prompt";
  const unsupported = unlockMode === "unsupported";

  if (unlockPanel) {
    unlockPanel.dataset.flow = unlockMode;
  }

  if (picking) {
    startUnlockClock();
  } else {
    stopUnlockClock();
  }

  if (unlockTitle) {
    if (welcoming) {
      unlockTitle.textContent = "Welcome to ElastOS";
    } else if (creatingGuest) {
      unlockTitle.textContent = "Add a guest";
    } else if (creatingAdmin) {
      unlockTitle.textContent = "Create your account";
    } else if (picking) {
      // Visible chrome is logo + clock; heading stays for screen readers.
      unlockTitle.textContent = "Choose an account";
    } else if (prompting) {
      unlockTitle.textContent = "Unlock to continue";
    } else {
      unlockTitle.textContent = "Sign in";
    }
  }

  if (unlockCopy) {
    if (welcoming) {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Create a passkey to unlock your data, apps and desktop.";
    } else if (creatingGuest) {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Guests get their own apps, files, and passkey.";
    } else if (creatingAdmin) {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Name this passkey, then create it on this device.";
    } else if (picking) {
      unlockCopy.hidden = true;
      unlockCopy.textContent = "";
    } else if (prompting) {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Use your passkey to continue.";
    } else if (unsupported) {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Passkeys are required to unlock Home.";
    } else {
      unlockCopy.hidden = false;
      unlockCopy.textContent = "Use your passkey to unlock your data, apps and desktop.";
    }
  }

  if (unlockAccounts) {
    if (picking) {
      renderAccountPicker();
      unlockAccounts.hidden = loginAccounts.length === 0;
    } else {
      unlockAccounts.hidden = true;
      unlockAccounts.replaceChildren();
    }
  }

  if (unlockActions) {
    unlockActions.hidden = picking;
  }

  if (unlockPrimary) {
    unlockPrimary.textContent = welcoming
      ? "Get started"
      : creatingGuest
        ? "Create passkey"
        : creatingAdmin
          ? "Create passkey"
          : "Continue";
    unlockPrimary.disabled = unsupported || busy;
    unlockPrimary.hidden = picking;
  }

  if (unlockSecondary) {
    const showBack = creatingGuest || creatingAdmin;
    unlockSecondary.hidden = !showBack;
    unlockSecondary.textContent = creatingGuest ? "Back to accounts" : "Back";
  }

  if (unlockAddGuest) {
    unlockAddGuest.hidden = !picking || !guestRegistrationEnabled;
  }

  setUnlockNameVisible(creatingAdmin || creatingGuest);
}

function renderAccountPicker() {
  if (!unlockAccounts) {
    return;
  }
  unlockAccounts.replaceChildren();
  loginAccounts.forEach((account, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "home-unlock-account";
    button.dataset.credentialId = account.credentialId;
    button.setAttribute("role", "radio");
    button.setAttribute("aria-checked", "false");
    button.tabIndex = index === accountFocusIndex ? 0 : -1;
    const roleLabel = account.role === "guest" ? "Guest" : "Admin";
    button.setAttribute("aria-label", `${account.displayName}, ${roleLabel}`);
    button.title = account.displayName;

    const avatar = document.createElement("span");
    avatar.className = "home-unlock-avatar";
    avatar.setAttribute("aria-hidden", "true");
    avatar.style.background = avatarColorForId(account.principalId);
    const monogram = document.createElement("span");
    monogram.className = "home-unlock-avatar-monogram";
    monogram.textContent = monogramForName(account.displayName);
    avatar.append(monogram);
    if (account.avatarCid) {
      const image = document.createElement("img");
      image.className = "home-unlock-avatar-image";
      image.alt = "";
      image.decoding = "async";
      image.src =
        `/api/auth/passkey/account-avatar?credential_id=${encodeURIComponent(account.credentialId)}&v=${encodeURIComponent(account.avatarCid)}`;
      image.addEventListener("load", () => {
        avatar.classList.add("has-photo");
      });
      image.addEventListener("error", () => {
        image.remove();
        avatar.classList.remove("has-photo");
      });
      avatar.append(image);
    }

    const name = document.createElement("span");
    name.className = "home-unlock-account-name";
    name.textContent = account.displayName;

    button.append(avatar, name);
    if (account.role === "guest") {
      const role = document.createElement("span");
      role.className = "home-unlock-account-role";
      role.textContent = "Guest";
      button.append(role);
    }
    unlockAccounts.append(button);
  });
  syncAccountSelection();
}

function syncAccountSelection() {
  const buttons = [...(unlockAccounts?.querySelectorAll("[data-credential-id]") || [])];
  buttons.forEach((button, index) => {
    const selected = readText(button.dataset.credentialId) === selectedCredentialId
      && Boolean(selectedCredentialId);
    button.classList.toggle("is-selected", selected);
    button.setAttribute("aria-checked", selected ? "true" : "false");
    button.tabIndex = index === accountFocusIndex ? 0 : -1;
  });
}

function focusAccountButton(index) {
  if (!loginAccounts.length || !unlockAccounts) {
    return;
  }
  accountFocusIndex = Math.max(0, Math.min(index, loginAccounts.length - 1));
  syncAccountSelection();
  const button = unlockAccounts.querySelectorAll("[data-credential-id]")[accountFocusIndex];
  button?.focus();
}

function handlePickerKeydown(event) {
  if (!loginAccounts.length) {
    return false;
  }
  if (event.key === "ArrowRight" || event.key === "ArrowDown") {
    event.preventDefault();
    focusAccountButton(accountFocusIndex + 1);
    return true;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
    event.preventDefault();
    focusAccountButton(accountFocusIndex - 1);
    return true;
  }
  if (event.key === "Enter" || event.key === " ") {
    const account = loginAccounts[accountFocusIndex];
    if (!account || busy) {
      return true;
    }
    event.preventDefault();
    selectedCredentialId = account.credentialId;
    syncAccountSelection();
    runPasskeySignIn({ credentialId: account.credentialId }).catch(reportUnlockError);
    return true;
  }
  if (event.key === "Escape" && unlockPanel.classList.contains("is-busy")) {
    event.preventDefault();
    return true;
  }
  return false;
}

async function runPasskeyCreate() {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  busy = true;
  setBusyUi(true);
  setUnlockStatus("Creating passkey…", "muted");
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
    await unlockComplete();
  } finally {
    busy = false;
    setBusyUi(false);
  }
}

async function runPasskeySignIn(options = {}) {
  if (busy || !window.PublicKeyCredential) {
    return;
  }
  const credentialId = readText(options.credentialId);
  busy = true;
  setBusyUi(true);
  setUnlockStatus("Waiting for passkey…", "muted");
  try {
    const beginBody = credentialId ? { credential_id: credentialId } : {};
    const begin = await fetchJson("/api/auth/passkey/authenticate/begin", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(beginBody),
    });
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
    await unlockComplete();
  } finally {
    busy = false;
    setBusyUi(false);
  }
}

async function unlockComplete() {
  setUnlockStatus("Opening Home…", "success");
  if (unlockCallback) {
    await unlockCallback();
    return;
  }
  hideHomeUnlock();
}

function reportUnlockError(error) {
  if (isPasskeyNotSelected(error)) {
    setUnlockStatus("Passkey cancelled. Choose an account to try again.", "muted");
    return;
  }
  setUnlockStatus(String(error.message || error), "error");
}

function setBusyUi(isBusy) {
  unlockPanel?.classList.toggle("is-busy", isBusy);
  if (unlockPrimary) {
    unlockPrimary.disabled = isBusy || unlockMode === "unsupported";
  }
  if (unlockSecondary) {
    unlockSecondary.disabled = isBusy;
  }
  if (unlockAddGuest) {
    unlockAddGuest.disabled = isBusy;
  }
  if (unlockName) {
    unlockName.disabled = isBusy || unlockName.hidden;
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

function normalizeLoginAccounts(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((entry) => ({
      principalId: readText(entry?.principal_id),
      displayName: readText(entry?.display_name) || "Account",
      role: readText(entry?.role).toLowerCase() || "guest",
      credentialId: readText(entry?.credential_id),
      lastUsedAt: Number(entry?.last_used_at) || 0,
      avatarCid: readText(entry?.avatar_cid),
    }))
    .filter((entry) => entry.principalId && entry.credentialId);
}

function monogramForName(name) {
  const parts = String(name || "")
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  if (parts.length === 0) {
    return "·";
  }
  const first = [...parts[0]][0] || "·";
  if (parts.length === 1) {
    return first.toUpperCase();
  }
  const second = [...parts[parts.length - 1]][0] || "";
  const mono = `${first}${second}`.toUpperCase();
  return /[A-Z0-9]/.test(mono.replace(/[^A-Z0-9]/g, "")) ? mono.replace(/[^A-Z0-9]/g, "") || "·" : "·";
}

function avatarColorForId(principalId) {
  let hash = 0;
  const text = String(principalId || "");
  for (let i = 0; i < text.length; i += 1) {
    hash = ((hash << 5) - hash) + text.charCodeAt(i);
    hash |= 0;
  }
  const hue = Math.abs(hash) % 360;
  return `linear-gradient(145deg, hsl(${hue} 52% 46%), hsl(${(hue + 28) % 360} 58% 36%))`;
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
