import {
  activeShellRoot,
  activeShellFrame,
  homeShellBootMask,
  shellHostRecovery,
  shellHostRecoveryTitle,
  shellHostRecoveryCopy,
  shellHostRecoveryDetail,
  shellHostRecoveryHomeButton,
  shellHostRecoveryReloadButton,
  shellHostRecoverySignOutButton,
  HOME_GUI_SHELL_ID,
  HOME_SHELL_HOST_ID,
  SYSTEM_APP_ID,
  homeActiveShellName,
  shellState,
  fetchJson,
  targetById,
} from "./shell-core.js?v=home-20260719y";
import {
  bindHomeUnlock,
  clearHomeSessionLock,
  hideHomeUnlock,
  isHomeAuthError,
  isHomeSessionLocked,
  rememberHomeSessionLock,
  refreshHomeSession,
  requestPasskeyHomeAuthority,
  showHomeUnlock,
  signOutHome,
} from "./shell-auth.js?v=home-20260719y";

const SUMMARY_REFRESH_DEBOUNCE_MS = 150;
const SUMMARY_REFRESH_RETRY_MS = 700;
const HOME_EVENTS_WAIT_MS = 25_000;
const HOME_EVENTS_RETRY_MS = 2_000;
const HOME_EVENTS_HIDDEN_RETRY_MS = 30_000;
const HOME_EVENTS_STREAM_URL = "/api/apps/home/events/stream";
const SESSION_REFRESH_MS = 10 * 60 * 1000;
const ACTIVE_SHELL_HINT_KEY = "elastos.home.active-shell-hint";
const HOME_CLI_SHELL_ID = "home-cli";
const OPAQUE_CAPSULE_ORIGIN = "null";
const OPAQUE_FRAME_TARGET = "*";
const MAX_LAUNCHED_APP_CONTEXTS = 128;
const WALLET_CONNECTOR_TARGETS = Object.freeze(
  new Set(["wallet-metamask", "wallet-unisat", "wallet-walletconnect"]),
);
const SHELL_MESSAGE_OPEN_TARGET_SOURCES = Object.freeze({
  "archive-manager": new Set(["library"]),
  browser: new Set(["library"]),
  "chat-room": new Set(["library"]),
  "gba-emulator": new Set(["library"]),
  "home-cli": "visible-target",
  "home-gui": "visible-target",
  inbox: "visible-target",
  library: new Set(["archive-manager", "documents", "gba-emulator", "library"]),
  marketplace: "runtime-target",
  people: new Set(["chat-room"]),
  services: new Set(["browser", "chat-room"]),
  system: "visible-target",
  "wallet": WALLET_CONNECTOR_TARGETS,
});
const SHELL_MESSAGE_OPEN_URI_SOURCES = new Set(["documents", "chat-room"]);
const SHELL_MESSAGE_DELIVER_TARGET_SOURCES = Object.freeze({
  "chat-room": new Set(["documents"]),
  documents: new Set(["chat-room"]),
  library: new Set(["archive-manager", "browser", "chat-room"]),
});
const PASSKEY_AUTHORITY_TARGETS = new Set(["inbox", SYSTEM_APP_ID, "wallet"]);
const launchedAppContexts = new Map();

function hideHostBootMask() {
  if (!homeShellBootMask) {
    return;
  }
  homeShellBootMask.hidden = true;
  homeShellBootMask.setAttribute("aria-hidden", "true");
}

function showHostBootMask() {
  if (!homeShellBootMask) {
    return;
  }
  homeShellBootMask.hidden = false;
  homeShellBootMask.setAttribute("aria-hidden", "true");
}

async function switchToHomeGuiAndOpenTarget(context, target, options = {}) {
  await activateDesktopShell(context.homeToken);
  await openTargetFromHomeGui(target, options);
}

function enterHostAuthGate() {
  shellState.activeShellRootLaunchSeq += 1;
  shellState.activeShellRootTarget = "";
  shellState.activeShellRootRoute = "";
  rememberActiveShellHint(HOME_GUI_SHELL_ID);
  document.body.dataset.homeShell = "resolving";
  document.body.dataset.homeGui = "dormant";
  launchedAppContexts.clear();
  if (activeShellRoot) {
    activeShellRoot.hidden = true;
    activeShellRoot.dataset.target = "";
  }
  if (activeShellFrame) {
    activeShellFrame.hidden = true;
    activeShellFrame.removeAttribute("src");
    activeShellFrame.dataset.route = "";
    activeShellFrame.title = "Active Home shell";
  }
  hideShellHostRecovery();
  stopHomeEventChannel();
}

async function showHostAuthGate(options = {}) {
  // Session Lock (Control Centre) must keep the live desktop mounted so frost
  // can blur it. enterHostAuthGate() tears the shell down — that path is only
  // for cold boot / unsigned front door / hard re-auth.
  const frostLock =
    options.presentation === "prompt" && options.surface === "desktop";
  if (frostLock) {
    // Survive refresh: seat stays locked until passkey unlock or Sign out.
    rememberHomeSessionLock();
  } else {
    enterHostAuthGate();
  }
  const surface = frostLock || options.surface === "desktop" ? "desktop" : "neutral";
  const onUnlocked = frostLock
    ? async () => {
        clearHomeSessionLock();
        document.body.dataset.homeStatus = "ready";
        // Gate already dismissed in unlockComplete; refresh under the live desktop.
        try {
          await refreshHomeSession();
        } catch (error) {
          if (!isHomeAuthError(error)) {
            console.error("home session refresh after lock failed", error);
          }
        }
        try {
          await refreshShellSummary();
        } catch (error) {
          console.error("home summary refresh after lock failed", error);
        }
        startShellTimers();
      }
    : () => boot();
  const unlockReady = showHomeUnlock(onUnlocked, {
    ...options,
    surface,
  });
  hideHostBootMask();
  await unlockReady;
}

async function launchHomeTarget(target, query = {}, authority = null) {
  const body = {
    target,
    query: {
      ...query,
      home_origin: window.location.origin,
    },
  };
  if (authority) {
    body.authority = authority;
  }
  const launched = await fetchJson("/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify(body),
  });
  if (launched?.attach_kind === "iframe") {
    rememberLaunchedAppContext(launched);
  }
  return launched;
}

function requireHomeGuiActive(action) {
  if (
    activeShellTarget(shellState.currentSummary) !== HOME_GUI_SHELL_ID ||
    shellState.activeShellRootTarget !== HOME_GUI_SHELL_ID ||
    !activeShellFrame?.contentWindow
  ) {
    throw new Error(`${action} requires active ${HOME_GUI_SHELL_ID}`);
  }
}

async function openTargetFromHomeGui(target, options = {}) {
  requireHomeGuiActive("open target");
  postToActiveShell({
    type: "home:gui-command",
    command: "open-target",
    target,
    query: options.query || {},
  });
}

async function closeHomeGuiWindow(homeToken) {
  requireHomeGuiActive("close window");
  postToActiveShell({ type: "home:gui-command", command: "close-window", homeToken });
  launchedAppContexts.delete(homeToken);
}

async function relaunchHomeGuiTarget(homeToken) {
  requireHomeGuiActive("relaunch window");
  postToActiveShell({ type: "home:gui-command", command: "relaunch-window", homeToken });
  launchedAppContexts.delete(homeToken);
}

async function deliverMessageToHomeGuiTargetFrame(target, payload) {
  requireHomeGuiActive("deliver target message");
  const contexts = [...launchedAppContexts.values()].reverse();
  const context = contexts.find((candidate) => candidate.targetId === target && candidate.source);
  if (!context) {
    return false;
  }
  context.source.postMessage(payload, context.origin);
  return true;
}

function setHomeGuiMenuManifest(windowId, menus, homeToken = "") {
  requireHomeGuiActive("set menu manifest");
  // Bridge to the isolated Home GUI frame — never direct-import the GUI module.
  // The host does not know GUI window ids; it forwards the sender's launch
  // token and the GUI resolves its own window (menus stay self-declared UI).
  postToActiveShell({
    type: "home:gui-command",
    command: "set-menu-manifest",
    windowId,
    homeToken,
    menus,
  });
}

async function openHomeGuiTargetWithPayload(target, payload) {
  requireHomeGuiActive("open target with payload");
  if (await deliverMessageToHomeGuiTargetFrame(target, payload)) {
    return true;
  }
  postToActiveShell({ type: "home:gui-command", command: "open-target", target, query: {} });
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 150));
    if (await deliverMessageToHomeGuiTargetFrame(target, payload)) {
      return true;
    }
  }
  console.warn("home target did not become ready for payload", target);
  return false;
}

async function showHomeGuiDesktop() {
  requireHomeGuiActive("show desktop");
  postToActiveShell({ type: "home:gui-command", command: "show-desktop" });
}

/* ---- Connector popup relay ----
   Wallet connector popups are real-origin top-level windows: the extension
   injects there, but the gateway (correctly) refuses their token'd API calls,
   and the sandbox's implicit noopener plus BroadcastChannel origin
   partitioning leave popup and opaque sheet with no direct channel. The host
   shares the popup's real origin, so it bridges: popup stages arrive on a
   same-origin BroadcastChannel and are forwarded into the sheet frame that
   owns the matching launch token; sheet answers come back over the token-bound
   frame bridge and are rebroadcast. Stages carry addresses/signatures only —
   the launch token never crosses, and API calls stay inside the opaque sheet. */
const CONNECTOR_POPUP_CHANNEL = "elastos:connector-popup";
const CONNECTOR_POPUP_RELAY_TYPE = "elastos:connector-popup-relay";

const connectorPopupChannel = "BroadcastChannel" in window
  ? new BroadcastChannel(CONNECTOR_POPUP_CHANNEL)
  : null;

if (connectorPopupChannel) {
  connectorPopupChannel.onmessage = (event) => {
    const message = event.data || {};
    if (message.type !== CONNECTOR_POPUP_RELAY_TYPE || message.from !== "popup") {
      return;
    }
    const tokenTail = typeof message.tokenTail === "string" ? message.tokenTail : "";
    if (!tokenTail) {
      return;
    }
    for (const [token, context] of launchedAppContexts) {
      if (!token.endsWith(tokenTail) || !WALLET_CONNECTOR_TARGETS.has(context.targetId)) {
        continue;
      }
      if (context.source) {
        context.source.postMessage(message, OPAQUE_FRAME_TARGET);
      }
      return;
    }
  };
}

function relayConnectorSheetAnswerToPopup(context, data) {
  if (!connectorPopupChannel) {
    return;
  }
  const { type: _type, homeToken: _homeToken, ...payload } = data;
  connectorPopupChannel.postMessage({
    type: CONNECTOR_POPUP_RELAY_TYPE,
    from: "sheet",
    tokenTail: context.homeToken.slice(-32),
    ...payload,
  });
}

/* ---- Shell UI preferences (theme / dock auto-hide / accent) ----
   Opaque frames cannot reach localStorage, so the host — the only real-origin
   document — is the canonical store. System (deep settings) and the GUI
   (Control Centre) write through a token-gated message; the host persists and
   relays a gui-command so the GUI chrome and its app frames re-apply. Closed
   key set, values are short enums — cosmetic state only, no authority. */
const UI_PREFERENCE_KEYS = Object.freeze({
  theme: new Set(["auto", "light", "dark"]),
  accent: new Set(["blue", "purple", "pink", "red", "orange", "yellow", "green", "graphite"]),
  dockAutoHide: new Set(["on", "off"]),
  sounds: new Set(["on", "off"]),
  focusMode: new Set(["on", "off"]),
});
const UI_PREFERENCE_STORE_PREFIX = "elastos.ui.";

function readUiPreferences() {
  const preferences = {};
  for (const key of Object.keys(UI_PREFERENCE_KEYS)) {
    try {
      const value = window.localStorage?.getItem(`${UI_PREFERENCE_STORE_PREFIX}${key}`) || "";
      if (UI_PREFERENCE_KEYS[key].has(value)) {
        preferences[key] = value;
      }
    } catch (_error) {
      // Host storage unavailable — defaults apply.
    }
  }
  return preferences;
}

function writeUiPreference(key, value) {
  if (!UI_PREFERENCE_KEYS[key]?.has(value)) {
    return false;
  }
  try {
    window.localStorage?.setItem(`${UI_PREFERENCE_STORE_PREFIX}${key}`, value);
  } catch (_error) {
    // Still relay: the GUI applies for this session even without persistence.
  }
  postToActiveShell({
    type: "home:gui-command",
    command: "ui-preference",
    preferences: { [key]: value },
  });
  return true;
}

function pushUiPreferencesToActiveShell() {
  const preferences = readUiPreferences();
  if (Object.keys(preferences).length === 0) {
    return;
  }
  postToActiveShell({
    type: "home:gui-command",
    command: "ui-preference",
    preferences,
  });
}

function syncActiveShellProjection(summary, activeShellMode) {
  if (
    activeShellMode !== "locked" &&
    activeShellTarget(summary) === shellState.activeShellRootTarget
  ) {
    postToActiveShell({ type: "home:shell-summary", summary });
  }
}

function postToActiveShell(message) {
  const route = shellState.activeShellRootRoute || activeShellFrame?.dataset?.route || "";
  if (!route || !activeShellFrame?.contentWindow) {
    return false;
  }
  activeShellFrame.contentWindow.postMessage(message, OPAQUE_FRAME_TARGET);
  return true;
}

function replyToShellRequest(event, requestId, result, error = null) {
  if (!requestId || !event.source) {
    return;
  }
  event.source.postMessage({
    type: "home:shell-response",
    requestId,
    result,
    error: error ? (error.message || String(error)) : undefined,
    status: Number(error?.status || 0),
  }, OPAQUE_FRAME_TARGET);
}

function rememberLaunchedAppContext(launched) {
  const token = homeLaunchTokenFromRoute(launched?.route || "");
  if (!token || !launched?.target) {
    throw new Error("Runtime returned an incomplete isolated launch");
  }
  while (launchedAppContexts.size >= MAX_LAUNCHED_APP_CONTEXTS) {
    launchedAppContexts.delete(launchedAppContexts.keys().next().value);
  }
  launchedAppContexts.set(token, {
    targetId: launched.target,
    viewerId: typeof launched.viewer === "string" ? launched.viewer : "",
    origin: OPAQUE_CAPSULE_ORIGIN,
    source: null,
  });
}

function activeShellTarget(summary) {
  const active = typeof summary?.active_shell?.active === "string"
    ? summary.active_shell.active.trim()
    : "";
  return normalizedActiveShellName(active || HOME_GUI_SHELL_ID);
}

function activeShellCandidate(summary, target) {
  const candidates = Array.isArray(summary?.active_shell?.candidates)
    ? summary.active_shell.candidates
    : [];
  return candidates.find((candidate) => candidate?.name === target) || null;
}

function readActiveShellHint() {
  try {
    const value = window.localStorage.getItem(ACTIVE_SHELL_HINT_KEY);
    return typeof value === "string" ? value.trim() : "";
  } catch (_error) {
    return "";
  }
}

function activeShellBootHintTarget() {
  const target = readActiveShellHint();
  return target === HOME_CLI_SHELL_ID ? target : "";
}

function rememberActiveShellHint(target) {
  try {
    const canonicalTarget = normalizedActiveShellName(target);
    if (canonicalTarget && canonicalTarget !== HOME_GUI_SHELL_ID) {
      window.localStorage.setItem(ACTIVE_SHELL_HINT_KEY, canonicalTarget);
      document.documentElement.dataset.homeShellHint = "alternate";
      document.documentElement.dataset.homeShellBoot = "alternate";
      return;
    }
    window.localStorage.removeItem(ACTIVE_SHELL_HINT_KEY);
    delete document.documentElement.dataset.homeShellHint;
    delete document.documentElement.dataset.homeShellBoot;
  } catch (_error) {}
}

function normalizedActiveShellName(value) {
  return homeActiveShellName(value);
}

function preclaimActiveShellSwitch(active) {
  const target = normalizedActiveShellName(active);
  if (!target) {
    return false;
  }
  shellState.activeShellRootLaunchSeq += 1;
  showHostBootMask();
  hideShellHostRecovery();
  rememberActiveShellHint(target);
  document.body.dataset.homeShell = target === HOME_GUI_SHELL_ID ? "desktop" : "alternate";
  document.body.dataset.homeGui = target === HOME_GUI_SHELL_ID ? "mounted" : "dormant";
  if (activeShellRoot) {
    activeShellRoot.hidden = false;
    activeShellRoot.dataset.target = target;
  }
  shellState.activeShellRootTarget = target;
  shellState.activeShellRootRoute = "";
  if (activeShellFrame) {
    activeShellFrame.hidden = true;
    activeShellFrame.removeAttribute("src");
    activeShellFrame.dataset.route = "";
    activeShellFrame.title = target;
  }
  requestShellSummaryRefresh({ reason: "active-shell-applied", delay: 0 });
  return true;
}

function applyActiveShellBootHint() {
  const target = activeShellBootHintTarget();
  if (!target) {
    return;
  }
  showHostBootMask();
  document.documentElement.dataset.homeShellHint = "alternate";
  document.documentElement.dataset.homeShellBoot = "alternate";
  document.body.dataset.homeShell = "alternate";
  document.body.dataset.homeGui = "dormant";
  if (activeShellRoot) {
    activeShellRoot.hidden = false;
    activeShellRoot.dataset.target = target;
  }
  if (activeShellFrame) {
    activeShellFrame.hidden = true;
    activeShellFrame.removeAttribute("src");
    activeShellFrame.dataset.route = "";
  }
}

async function activateDesktopShell(homeToken = "") {
  if (activeShellTarget(shellState.currentSummary) !== HOME_GUI_SHELL_ID) {
    await fetchJson("/api/apps/home/active-shell", {
      method: "POST",
      headers: homeToken ? { "x-elastos-home-token": homeToken } : {},
      body: JSON.stringify({ active: HOME_GUI_SHELL_ID }),
    });
    await refreshShellSummary();
    return;
  }
  await showHomeGuiDesktop();
}

function shellDisplayName(target) {
  const shell = normalizedActiveShellName(target);
  if (shell === HOME_GUI_SHELL_ID) {
    return "Desktop";
  }
  if (shell === "home-cli") {
    return "Terminal";
  }
  return "Home view";
}

function shellHostRecoveryDetailText(error) {
  if (!error) {
    return "";
  }
  const message = typeof error === "string" ? error : (error.message || String(error));
  if (/session|token|unauthorized|forbidden|expired/i.test(message)) {
    return "Your session needs to be unlocked again.";
  }
  if (/fetch|network|connect|refused|offline/i.test(message)) {
    return "ElastOS is not responding on this device.";
  }
  return "A Home service failed while loading.";
}

function activeShellRootHomeToken() {
  return homeLaunchTokenFromRoute(
    shellState.activeShellRootRoute ||
      activeShellFrame?.dataset?.route ||
      activeShellFrame?.getAttribute("src") ||
      "",
  );
}

function hideShellHostRecovery() {
  if (shellHostRecovery) {
    shellHostRecovery.hidden = true;
  }
  if (shellHostRecoveryTitle) {
    shellHostRecoveryTitle.textContent = "Home didn't open";
  }
  if (shellHostRecoveryCopy) {
    shellHostRecoveryCopy.textContent = "";
  }
  if (shellHostRecoveryDetail) {
    shellHostRecoveryDetail.hidden = true;
    shellHostRecoveryDetail.textContent = "";
  }
  if (shellHostRecoveryHomeButton) {
    shellHostRecoveryHomeButton.disabled = false;
    shellHostRecoveryHomeButton.title = "Open Desktop";
  }
}

function showShellHostRecovery(target, error, options = {}) {
  const detail = shellHostRecoveryDetailText(error);
  const tokenAvailable = Boolean(activeShellRootHomeToken());
  document.body.dataset.homeGui = "dormant";
  hideHostBootMask();
  if (activeShellRoot) {
    activeShellRoot.hidden = false;
    activeShellRoot.dataset.target = target || "";
  }
  if (activeShellFrame) {
    activeShellFrame.hidden = true;
  }
  if (shellHostRecovery) {
    shellHostRecovery.hidden = false;
    shellHostRecovery.dataset.host = HOME_SHELL_HOST_ID;
    shellHostRecovery.dataset.target = target || "";
  }
  if (shellHostRecoveryTitle) {
    shellHostRecoveryTitle.textContent = options.title || `Could not start ${shellDisplayName(target)}`;
  }
  if (shellHostRecoveryCopy) {
    shellHostRecoveryCopy.textContent = options.copy || (
      tokenAvailable
        ? "Open Desktop or reload to try again. Your data is unchanged."
        : "Reload to try again. Your data is unchanged."
    );
  }
  if (shellHostRecoveryDetail) {
    shellHostRecoveryDetail.hidden = !detail;
    shellHostRecoveryDetail.textContent = detail;
  }
  if (shellHostRecoveryHomeButton) {
    shellHostRecoveryHomeButton.disabled = !tokenAvailable;
    shellHostRecoveryHomeButton.title = tokenAvailable
      ? "Open Desktop"
      : "Reload Home before opening Desktop.";
  }
}

function reloadHomeShellHost() {
  if (typeof window.location.reload === "function") {
    window.location.reload();
    return;
  }
  window.location.href = "/apps/home/";
}

async function recoverToHomeGui() {
  const homeToken = activeShellRootHomeToken();
  if (!homeToken) {
    showShellHostRecovery(shellState.activeShellRootTarget, "No shell launch token is available.", {
      title: "Desktop is unavailable",
      copy: "Reload to try again. Your data is unchanged.",
    });
    return;
  }
  if (shellHostRecoveryHomeButton) {
    shellHostRecoveryHomeButton.disabled = true;
  }
  try {
    await activateDesktopShell(homeToken);
  } catch (error) {
    console.error("home-gui recovery failed", error);
    showShellHostRecovery(shellState.activeShellRootTarget, error, {
      title: "Desktop didn't open",
      copy: "Reload to try again. Your data is unchanged.",
    });
  }
}

async function settleRootShellClose(context, data) {
  const activeShell = normalizedActiveShellName(data?.activeShell);
  if (activeShell === HOME_GUI_SHELL_ID) {
    try {
      await refreshShellSummary();
      if (activeShellTarget(shellState.currentSummary) === HOME_GUI_SHELL_ID) {
        return;
      }
    } catch (error) {
      console.warn("home shell close refresh failed", error);
    }
  }
  await activateDesktopShell(context.homeToken);
}

async function signOutFromShellHostRecovery() {
  if (shellHostRecoverySignOutButton) {
    shellHostRecoverySignOutButton.disabled = true;
  }
  try {
    await signOutHome();
    reloadHomeShellHost();
  } catch (error) {
    console.error("shell host sign out failed", error);
    showShellHostRecovery(shellState.activeShellRootTarget, error, {
      title: "Could not sign out",
      copy: `Try reloading Home, then sign out from ${HOME_GUI_SHELL_ID}.`,
    });
  } finally {
    if (shellHostRecoverySignOutButton) {
      shellHostRecoverySignOutButton.disabled = false;
    }
  }
}

async function signOutFromRootShell() {
  document.body.dataset.homeStatus = "booting";
  try {
    await signOutHome();
    reloadHomeShellHost();
  } catch (error) {
    console.error("home shell sign out failed", error);
    showShellHostRecovery(shellState.activeShellRootTarget, error, {
      title: "Could not sign out",
      copy: "Reload Home and try again.",
    });
  }
}

function clearActiveShellRoot({ resetHint = false } = {}) {
  if (resetHint) {
    rememberActiveShellHint(HOME_GUI_SHELL_ID);
  }
  document.body.dataset.homeShell = "resolving";
  document.body.dataset.homeGui = "dormant";
  shellState.activeShellRootTarget = "";
  shellState.activeShellRootRoute = "";
  if (activeShellRoot) {
    activeShellRoot.hidden = true;
    activeShellRoot.dataset.target = "";
  }
  if (activeShellFrame) {
    activeShellFrame.removeAttribute("src");
    activeShellFrame.dataset.route = "";
    activeShellFrame.title = "Active Home shell";
  }
  hideShellHostRecovery();
}

function showActiveShellError(target, error) {
  showShellHostRecovery(target, error);
}

function shouldDeferHomeGuiForBootHint(summary, options = {}) {
  if (activeShellTarget(summary) !== HOME_GUI_SHELL_ID) {
    return false;
  }
  if (options.deferHomeGuiForRuntimeSettle === true) {
    return true;
  }
  return options.deferHomeGuiForBootHint === true &&
    Boolean(activeShellBootHintTarget());
}

async function syncActiveShellRoot(summary, options = {}) {
  const target = activeShellTarget(summary);
  if (!homeSummarySignedIn(summary)) {
    clearActiveShellRoot({ resetHint: true });
    return "locked";
  }
  if (target === HOME_GUI_SHELL_ID && shouldDeferHomeGuiForBootHint(summary, options)) {
    showHostBootMask();
    clearActiveShellRoot();
    return "alternate";
  }

  const candidate = activeShellCandidate(summary, target);
  if (!candidate || candidate.launchable !== true) {
    clearActiveShellRoot();
    showActiveShellError(target, "The selected Home shell is not launchable");
    return "locked";
  }

  rememberActiveShellHint(target);
  showHostBootMask();
  document.body.dataset.homeShell = target === HOME_GUI_SHELL_ID ? "desktop" : "alternate";
  document.body.dataset.homeGui = target === HOME_GUI_SHELL_ID ? "mounted" : "dormant";
  if (activeShellRoot) {
    activeShellRoot.hidden = false;
    activeShellRoot.dataset.target = target;
  }
  hideShellHostRecovery();
  if (activeShellFrame) {
    activeShellFrame.title = candidate.title || target;
  }
  if (
    shellState.activeShellRootTarget === target &&
    shellState.activeShellRootRoute &&
    activeShellFrame?.dataset.route === shellState.activeShellRootRoute
  ) {
    activeShellFrame.hidden = false;
    hideHostBootMask();
    return target === HOME_GUI_SHELL_ID ? "desktop" : "alternate";
  }

  const launchSeq = shellState.activeShellRootLaunchSeq + 1;
  shellState.activeShellRootLaunchSeq = launchSeq;
  shellState.activeShellRootTarget = target;
  shellState.activeShellRootRoute = "";
  if (activeShellFrame) {
    activeShellFrame.removeAttribute("src");
    activeShellFrame.dataset.route = "";
  }
  try {
    const launched = await launchHomeTarget(target, { shell_mode: "root" });
    if (shellState.activeShellRootLaunchSeq !== launchSeq) {
      return target === HOME_GUI_SHELL_ID ? "desktop" : "alternate";
    }
    if (launched.attach_kind !== "iframe") {
      throw new Error(`unsupported shell attach kind: ${launched.attach_kind || "unknown"}`);
    }
    if (launched.target !== target) {
      throw new Error(`runtime launched ${launched.target || "unknown"} instead of ${target}`);
    }
    shellState.activeShellRootTarget = target;
    shellState.activeShellRootRoute = launched.route;
    if (activeShellFrame && activeShellFrame.dataset.route !== launched.route) {
      activeShellFrame.hidden = false;
      activeShellFrame.src = launched.route;
      activeShellFrame.dataset.route = launched.route;
    }
    hideHostBootMask();
  } catch (error) {
    console.error("active shell root launch failed", error);
    showActiveShellError(target, error);
  }
  return target === HOME_GUI_SHELL_ID ? "desktop" : "alternate";
}

function registerHomeServiceWorker() {
  // Home is network-first during active Runtime development. A stale service
  // worker can strand the shell on an old module graph while provider APIs are
  // live, so clear any registration left by older builds.
  if (!("serviceWorker" in navigator)) {
    return;
  }
  navigator.serviceWorker.getRegistrations()
    .then((registrations) => Promise.all(
      registrations.map((registration) => registration.unregister()),
    ))
    .catch((error) => {
      console.warn("home service worker cleanup failed", error);
    });
}

applyActiveShellBootHint();

boot().catch((error) => {
  document.body.dataset.homeStatus = "error";
  console.error("home boot failed", error);
  showShellHostRecovery(HOME_SHELL_HOST_ID, error, {
    title: "Home didn't open",
    copy: "Reload to try again. Your data is unchanged.",
  });
});

registerHomeServiceWorker();
bindHomeUnlock();

shellHostRecoveryHomeButton?.addEventListener("click", () => {
  recoverToHomeGui().catch((error) => {
    console.error("home-gui recovery failed", error);
  });
});

shellHostRecoveryReloadButton?.addEventListener("click", () => {
  reloadHomeShellHost();
});

shellHostRecoverySignOutButton?.addEventListener("click", () => {
  signOutFromShellHostRecovery().catch((error) => {
    console.error("shell host recovery sign out failed", error);
  });
});

window.addEventListener("message", (event) => {
  const data = event.data;
  const context = homeMessageContext(event, data);
  if (!context) {
    return;
  }
  if (data.type === "home:shell-ready") {
    if (context.kind === "shell-frame" && shellState.currentSummary) {
      postToActiveShell({ type: "home:shell-summary", summary: shellState.currentSummary });
    }
    if (context.kind === "shell-frame" && context.targetId === HOME_GUI_SHELL_ID) {
      pushUiPreferencesToActiveShell();
    }
    return;
  }
  if (data.type === "home:connector-popup-relay") {
    // Only a mounted wallet connector sheet may answer its own popup.
    if (context.kind !== "app-frame" || !WALLET_CONNECTOR_TARGETS.has(context.targetId)) {
      console.warn("home ignored unauthorized connector-popup-relay message", context.targetId);
      return;
    }
    relayConnectorSheetAnswerToPopup(context, data);
    return;
  }
  if (data.type === "home:ui-preference") {
    // Cosmetic shell preferences: System's Personalization pane and the GUI's
    // Control Centre are the only writers. Closed key/value sets.
    const trustedSystemApp = context.kind === "app-frame" && context.targetId === SYSTEM_APP_ID;
    const trustedGuiShell = context.kind === "shell-frame" && context.targetId === HOME_GUI_SHELL_ID;
    if (!trustedSystemApp && !trustedGuiShell) {
      console.warn("home ignored unauthorized ui-preference message", context.targetId);
      return;
    }
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    if (data.action === "read") {
      replyToShellRequest(event, requestId, readUiPreferences());
      return;
    }
    const key = typeof data.key === "string" ? data.key.trim() : "";
    const value = typeof data.value === "string" ? data.value.trim() : "";
    if (!writeUiPreference(key, value)) {
      console.warn("home ignored invalid ui-preference", key, value);
      if (requestId) {
        replyToShellRequest(event, requestId, null, new Error("Home rejected the preference"));
      }
      return;
    }
    if (requestId) {
      replyToShellRequest(event, requestId, true);
    }
    return;
  }
  if (data.type === "home:app-ready") {
    return;
  }
  if (data.type === "home:launch-target") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    const target = typeof data.target === "string" ? data.target.trim() : "";
    if (
      context.kind !== "shell-frame" ||
      ![HOME_GUI_SHELL_ID, HOME_CLI_SHELL_ID].includes(context.targetId) ||
      !requestId ||
      !target ||
      !canOpenTargetFromHomeMessage(context, target)
    ) {
      replyToShellRequest(event, requestId, null, new Error("Home denied the shell launch"));
      return;
    }
    const query = data.query && typeof data.query === "object" ? data.query : {};
    launchHomeTarget(target, query)
      .then((launched) => replyToShellRequest(event, requestId, launched))
      .catch((error) => replyToShellRequest(event, requestId, null, error));
    return;
  }
  if (data.type === "home:request-unlock") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    if (
      context.kind !== "shell-frame" ||
      ![HOME_GUI_SHELL_ID, HOME_CLI_SHELL_ID].includes(context.targetId)
    ) {
      replyToShellRequest(event, requestId, null, new Error("Home denied the unlock request"));
      return;
    }
    replyToShellRequest(event, requestId, true);
    showHostAuthGate({ presentation: "prompt", surface: "desktop" }).catch((error) => {
      console.error("home unlock failed", error);
    });
    return;
  }
  if (data.type === "home:refresh-summary") {
    requestShellSummaryRefresh({ reason: "child-message" });
    // Successful connector link — tell the isolated GUI to close its in-rail
    // ceremony sheet (projection only; refresh already ran). Token-bound:
    // the GUI verifies the token against its mounted sheet frame, so no
    // other child can dismiss a ceremony it does not own.
    if (context.kind === "app-frame" && context.homeToken) {
      postToActiveShell({
        type: "home:gui-command",
        command: "connector-summary-refresh",
        homeToken: context.homeToken,
      });
    }
    return;
  }
  if (data.type === "home:menu-manifest") {
    // Menus are self-declared UI, not authority: a window may only shape its
    // OWN menu bar entry. The host binds the manifest to the sender's launch
    // token; the isolated GUI resolves that token to its own window.
    if (context.kind !== "app-frame" || !context.homeToken) {
      console.warn("home ignored unauthorized menu-manifest message", context.targetId);
      return;
    }
    try {
      setHomeGuiMenuManifest(context.windowId || "", data.menus, context.homeToken);
    } catch (error) {
      console.error("home menu-manifest failed", error);
    }
    return;
  }
  if (data.type === "home:active-shell-applied") {
    const trustedSystemApp = context.kind === "app-frame" && context.targetId === SYSTEM_APP_ID;
    const trustedRootShell = context.kind === "shell-frame" &&
      [HOME_GUI_SHELL_ID, HOME_CLI_SHELL_ID].includes(context.targetId);
    if (!trustedSystemApp && !trustedRootShell) {
      console.warn("home ignored unauthorized active-shell-applied message", context.targetId);
      return;
    }
    preclaimActiveShellSwitch(data.activeShell);
    return;
  }
  if (data.type === "home:request-passkey-authority") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    const operation = typeof data.operation === "string" ? data.operation.trim() : "";
    const request = data.request && typeof data.request === "object" ? data.request : null;
    if (
      context.kind !== "app-frame"
      || !PASSKEY_AUTHORITY_TARGETS.has(context.targetId)
      || !requestId
      || requestId.length > 128
      || !operation
      || operation.length > 128
      || !request
      || JSON.stringify(request).length > 65_536
    ) {
      console.warn("home ignored unauthorized passkey request", context.targetId);
      return;
    }
    const reply = (payload) => {
      try {
        event.source?.postMessage({
          type: "home:passkey-authority-result",
          requestId,
          ...payload,
        }, OPAQUE_FRAME_TARGET);
      } catch (error) {
        console.error("home could not return passkey result", error);
      }
    };
    requestPasskeyHomeAuthority()
      .then(async () => {
        const launched = await launchHomeTarget(context.targetId, {}, { operation, request });
        const scopedToken = homeLaunchTokenFromRoute(launched?.route || "");
        if (!scopedToken) {
          throw new Error("Passkey verification did not return capsule authority.");
        }
        reply({ homeToken: scopedToken });
      })
      .catch((error) => reply({
        error: error instanceof Error ? error.message : "Passkey verification failed.",
      }));
    return;
  }
  if (data.type === "home:sign-out") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    if (
      context.kind !== "shell-frame" ||
      ![HOME_GUI_SHELL_ID, HOME_CLI_SHELL_ID].includes(context.targetId)
    ) {
      console.warn("home ignored unauthorized sign-out message", context.targetId);
      replyToShellRequest(event, requestId, null, new Error("Home denied the sign-out request"));
      return;
    }
    replyToShellRequest(event, requestId, true);
    signOutFromRootShell().catch((error) => {
      console.error("home shell sign out failed", error);
    });
    return;
  }
  if (data.type === "home:switch-shell-and-open-target") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    const target = typeof data.target === "string" ? data.target.trim() : "";
    if (
      context.kind !== "shell-frame" ||
      context.targetId !== HOME_CLI_SHELL_ID ||
      !requestId ||
      !target ||
      !canOpenTargetFromHomeMessage(context, target)
    ) {
      replyToShellRequest(event, requestId, null, new Error("Home denied the graphical launch"));
      return;
    }
    const query = data.query && typeof data.query === "object" ? data.query : {};
    switchToHomeGuiAndOpenTarget(context, target, { query })
      .then(() => replyToShellRequest(event, requestId, true))
      .catch((error) => replyToShellRequest(event, requestId, null, error));
    return;
  }
  if (data.type === "home:open-uri") {
    if (!canOpenUriFromHomeMessage(context)) {
      console.warn("home ignored unauthorized open-uri message", context.targetId);
      return;
    }
    const resolved = resolveOpenUri(data);
    if (!resolved) {
      console.warn("home could not resolve URI", data.uri);
      return;
    }
    if (!targetById(shellState.currentSummary, resolved.target)) {
      console.warn("home could not open URI because its viewer is not installed", data.uri);
      return;
    }
    openTargetFromHomeGui(resolved.target, { query: resolved.query }).catch((error) => {
      console.error("home open-uri failed", error);
    });
    return;
  }
  if (data.type === "home:deliver-to-target") {
    const target = typeof data.target === "string" ? data.target.trim() : "";
    if (!target || !canDeliverTargetFromHomeMessage(context, target)) {
      console.warn("home ignored unauthorized deliver-to-target message", context.targetId, target);
      return;
    }
    const payload = data.payload && typeof data.payload === "object" ? data.payload : null;
    if (!payload || typeof payload.type !== "string") {
      console.warn("home ignored malformed deliver-to-target payload", context.targetId, target);
      return;
    }
    deliverMessageToHomeGuiTargetFrame(target, payload)
      .then((delivered) => {
        if (!delivered) {
          console.warn("home could not deliver message to target", target);
        }
      })
      .catch((error) => {
        console.error("home deliver-to-target failed", error);
      });
    return;
  }
  if (data.type === "home:open-target-with-payload") {
    const target = typeof data.target === "string" ? data.target.trim() : "";
    if (!target || !canDeliverTargetFromHomeMessage(context, target)) {
      console.warn("home ignored unauthorized open-target-with-payload message", context.targetId, target);
      return;
    }
    const payload = data.payload && typeof data.payload === "object" ? data.payload : null;
    if (!payload || typeof payload.type !== "string") {
      console.warn("home ignored malformed open-target-with-payload payload", context.targetId, target);
      return;
    }
    openHomeGuiTargetWithPayload(target, payload)
      .then((opened) => {
        if (!opened) {
          console.warn("home could not open target with payload", target);
        }
      })
      .catch((error) => {
        console.error("home open-target-with-payload failed", error);
      });
    return;
  }
  if (data.type === "home:close-self") {
    if (context.kind === "shell-frame") {
      settleRootShellClose(context, data).catch((error) => {
        console.error("home shell close failed", error);
      });
      return;
    }
    if (context.kind !== "app-frame") {
      console.warn("home ignored unauthorized close-self message", context.targetId);
      return;
    }
    closeHomeGuiWindow(context.homeToken).catch((error) => {
      console.error("home close-self failed", error);
    });
    return;
  }
  if (data.type === "home:relaunch-self") {
    if (context.kind !== "app-frame" || !context.targetId) {
      console.warn("home ignored unauthorized relaunch-self message", context.targetId);
      return;
    }
    relaunchHomeGuiTarget(context.homeToken).catch((error) => {
      console.error("home relaunch-self failed", error);
    });
    return;
  }
  if (data.type !== "home:open-target") {
    return;
  }
  const target = typeof data.target === "string" ? data.target.trim() : "";
  if (!target) {
    return;
  }
  if (!canOpenTargetFromHomeMessage(context, target)) {
    console.warn("home ignored unauthorized open-target message", context.targetId, target);
    return;
  }
  const query = data.query && typeof data.query === "object" ? data.query : {};
  if (context.kind === "shell-frame") {
    if (context.targetId === HOME_GUI_SHELL_ID) {
      openTargetFromHomeGui(target, { query }).catch((error) => {
        console.error("home shell open-target failed", error);
      });
    } else {
      console.warn("home ignored non-graphical shell open-target message", context.targetId, target);
    }
    return;
  }
  openTargetFromHomeGui(target, { query }).catch((error) => {
    console.error("home open-target failed", error);
  });
});

function homeMessageContext(event, data) {
  if (!data || typeof data !== "object") {
    return null;
  }
  if (event.source === window) {
    return event.origin === window.location.origin
      ? { kind: "home", targetId: HOME_GUI_SHELL_ID }
      : null;
  }
  const homeToken = typeof data.homeToken === "string" ? data.homeToken.trim() : "";
  if (!homeToken) {
    return null;
  }
  if (activeShellFrame) {
    let shellFrameWindow = null;
    try {
      shellFrameWindow = activeShellFrame.contentWindow;
    } catch (_error) {
      shellFrameWindow = null;
    }
    if (shellFrameWindow === event.source) {
      if (event.origin !== OPAQUE_CAPSULE_ORIGIN) {
        return null;
      }
      const expectedToken = homeLaunchTokenFromRoute(
        activeShellFrame.dataset.route || activeShellFrame.getAttribute("src") || "",
      );
      if (!expectedToken || expectedToken !== homeToken) {
        return null;
      }
      const targetId = typeof activeShellRoot?.dataset?.target === "string"
        ? activeShellRoot.dataset.target
        : "";
      return { kind: "shell-frame", targetId, homeToken };
    }
  }
  const launched = launchedAppContexts.get(homeToken);
  if (!launched || launched.origin !== event.origin) {
    return null;
  }
  if (launched.source && launched.source !== event.source) {
    return null;
  }
  launched.source = event.source;
  return {
    kind: "app-frame",
    targetId: launched.targetId,
    viewerId: launched.viewerId || "",
    homeToken,
  };
}

function homeLaunchTokenFromRoute(route) {
  try {
    const url = new URL(route, window.location.href);
    return new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") || "";
  } catch (_error) {
    return "";
  }
}

function canOpenUriFromHomeMessage(context) {
  return context.kind === "home" || SHELL_MESSAGE_OPEN_URI_SOURCES.has(context.targetId);
}

function canOpenTargetFromHomeMessage(context, target) {
  if (context.kind === "home") {
    return true;
  }
  // Viewer-bound content (e.g. gba-ucity) speaks with its own target id but
  // carries no policy entry; it inherits its viewer's grants (gba-emulator),
  // still a closed set — never broader than the viewer itself.
  const policy = SHELL_MESSAGE_OPEN_TARGET_SOURCES[context.targetId] ||
    (context.viewerId ? SHELL_MESSAGE_OPEN_TARGET_SOURCES[context.viewerId] : undefined);
  if (!policy) {
    return false;
  }
  // Connector capsules are hidden from Home's visible targets by design, so
  // the visible-target policy can never authorize them. The GUI hosts the
  // wallet rail ceremony sheet, so it carries the wallet's connector-launch
  // authority — the same closed set the wallet capsule itself holds.
  if (context.targetId === HOME_GUI_SHELL_ID && WALLET_CONNECTOR_TARGETS.has(target)) {
    return true;
  }
  if (policy === "visible-target") {
    return !!targetById(shellState.currentSummary, target) &&
      normalizedActiveShellName(target) !== HOME_GUI_SHELL_ID;
  }
  if (policy === "runtime-target") {
    return normalizedActiveShellName(target) !== HOME_GUI_SHELL_ID;
  }
  return policy.has(target);
}

function canDeliverTargetFromHomeMessage(context, target) {
  if (context.kind !== "app-frame" && context.kind !== "shell-frame") {
    return false;
  }
  const policy = SHELL_MESSAGE_DELIVER_TARGET_SOURCES[context.targetId];
  return !!policy && policy.has(target);
}

function resolveOpenUri(data) {
  const uri = typeof data.uri === "string" ? data.uri.trim() : "";
  if (!uri.startsWith("elastos://")) {
    return null;
  }
  const peerInvite = resolvePeerInviteUri(uri);
  if (peerInvite) {
    return peerInvite;
  }
  const cid = uri.slice("elastos://".length).split(/[/?#]/)[0].trim();
  if (!cid) {
    return null;
  }
  const preferredViewer = typeof data.preferredViewer === "string" ? data.preferredViewer.trim() : "";
  if (preferredViewer === "documents" || preferredViewer === "") {
    return {
      target: "documents",
      query: {
        cid,
        uri,
        view: "read",
      },
    };
  }
  return null;
}

function resolvePeerInviteUri(uri) {
  try {
    const parsed = new URL(uri);
    const path = parsed.pathname.replace(/^\/+/, "");
    const isPeerInvite = parsed.hostname === "peer" && path === "invite";
    if (parsed.protocol === "elastos:" && isPeerInvite && parsed.searchParams.get("token")) {
      return {
        target: "chat-room",
        query: { invite: uri },
      };
    }
  } catch (_error) {
    return null;
  }
  return null;
}

async function boot() {
  document.body.dataset.homeStatus = "booting";
  let summary = null;
  const sessionLocked = isHomeSessionLocked();
  // Lock restore needs the desktop mounted under frost — never defer GUI away.
  const deferHomeGuiForBootHint =
    !sessionLocked && Boolean(activeShellBootHintTarget());
  try {
    await refreshHomeSession();
  } catch (error) {
    if (!isHomeAuthError(error)) {
      throw error;
    }
  }
  try {
    summary = await refreshShellSummary({ deferHomeGuiForBootHint });
  } catch (error) {
    if (isHomeAuthError(error)) {
      clearHomeSessionLock();
      await showHostAuthGate();
      return;
    }
    throw error;
  }
  if (!homeSummarySignedIn(summary)) {
    // Unsigned front door (cold boot or after Sign out): full account picker.
    // Compact prompt is only for mid-session re-auth over a live shell.
    clearHomeSessionLock();
    document.body.dataset.homeStatus = "ready";
    await showHostAuthGate();
    startShellTimers();
    return;
  }
  const runtimeReady = fetchJson("/api/apps/home/runtime/ensure", { method: "POST" })
    .catch((error) => {
      console.error("home runtime ensure failed", error);
      return null;
    });
  document.body.dataset.homeStatus = "ready";
  if (sessionLocked) {
    // Seat was locked before refresh — restore frost over the live desktop.
    await showHostAuthGate({ presentation: "prompt", surface: "desktop" });
    startShellTimers();
    return;
  }
  hideHomeUnlock();
  runtimeReady.then(() => refreshShellSummary()).catch((error) => {
    console.error("home summary refresh failed after runtime ensure", error);
  });
  startShellTimers();
}

function startShellTimers() {
  if (!shellState.sessionRefreshTimer) {
    shellState.sessionRefreshTimer = window.setInterval(() => {
      refreshSignedHomeSession();
    }, SESSION_REFRESH_MS);
  }
  if (!shellState.summaryVisibilityRefreshBound) {
    shellState.summaryVisibilityRefreshBound = true;
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) {
        return;
      }
      requestShellSummaryRefresh({ reason: "visibilitychange", delay: 0 });
      ensureHomeEventChannel();
    });
  }
}

function requestShellSummaryRefresh({ reason = "request", delay = SUMMARY_REFRESH_DEBOUNCE_MS } = {}) {
  window.clearTimeout(shellState.summaryRefreshDebounceTimer);
  shellState.summaryRefreshDebounceTimer = window.setTimeout(() => {
    if (document.hidden) {
      requestShellSummaryRefresh({ reason, delay: SUMMARY_REFRESH_RETRY_MS });
      return;
    }
    if (shellState.summaryRefreshInFlight) {
      requestShellSummaryRefresh({ reason, delay: SUMMARY_REFRESH_RETRY_MS });
      return;
    }
    shellState.summaryRefreshInFlight = true;
    refreshShellSummary().catch((error) => {
      if (isHomeAuthError(error)) {
        // Mid-session summary 401: compact re-auth, never the family picker.
        showHostAuthGate({ presentation: "prompt" }).catch((unlockError) => {
          console.error("home unlock failed", unlockError);
        });
        return;
      }
      console.error(`home summary refresh failed (${reason})`, error);
    })
      .finally(() => {
        shellState.summaryRefreshInFlight = false;
      });
  }, delay);
}

function homeSummarySignedIn(summary) {
  return summary?.authority?.signed_in === true;
}

function homeSummaryHasProofBoundSession(summary) {
  return (
    homeSummarySignedIn(summary) &&
    typeof summary?.authority?.proof_binding_id === "string" &&
    summary.authority.proof_binding_id.trim() !== ""
  );
}

function refreshSignedHomeSession() {
  if (!homeSummaryHasProofBoundSession(shellState.currentSummary)) {
    return Promise.resolve(null);
  }
  return refreshHomeSession()
    .then(() => refreshShellSummary())
    .catch((error) => {
      if (isHomeAuthError(error)) {
        showHostAuthGate({ presentation: "prompt" }).catch((unlockError) => {
          console.error("home unlock failed", unlockError);
        });
        return null;
      }
      console.error("home session refresh failed", error);
      return null;
    });
}

async function refreshShellSummary({
  deferHomeGuiForBootHint = false,
  deferHomeGuiForRuntimeSettle = false,
} = {}) {
  const summary = await fetchJson("/api/apps/home/summary");
  shellState.currentSummary = summary;
  shellState.requestSummaryRefresh = refreshShellSummary;
  document.body.dataset.homeAuthority = homeSummarySignedIn(summary) ? "signed" : "unsigned";

  if (homeSummarySignedIn(summary)) {
    ensureHomeEventChannel();
  } else {
    stopHomeEventChannel();
  }

  const activeShellMode = await syncActiveShellRoot(summary, {
    deferHomeGuiForBootHint,
    deferHomeGuiForRuntimeSettle,
  });
  syncActiveShellProjection(summary, activeShellMode);
  return summary;
}

function ensureHomeEventChannel() {
  if (
    !homeSummarySignedIn(shellState.currentSummary)
  ) {
    return;
  }
  if (window.EventSource && !shellState.homeEventsStreamFailed) {
    ensureHomeEventStream();
    return;
  }
  if (shellState.homeEventsInFlight) {
    return;
  }
  window.clearTimeout(shellState.homeEventsTimer);
  shellState.homeEventsTimer = window.setTimeout(pollHomeEvents, 0);
}

function stopHomeEventChannel() {
  shellState.homeEventsCursor = "";
  shellState.homeEventsInFlight = false;
  shellState.homeEventsStreamFailed = false;
  if (shellState.homeEventsSource) {
    shellState.homeEventsSource.close();
    shellState.homeEventsSource = null;
  }
  window.clearTimeout(shellState.homeEventsTimer);
  shellState.homeEventsTimer = null;
}

function ensureHomeEventStream() {
  if (shellState.homeEventsSource) {
    return;
  }
  window.clearTimeout(shellState.homeEventsTimer);
  const source = new EventSource(HOME_EVENTS_STREAM_URL, { withCredentials: true });
  shellState.homeEventsSource = source;
  source.addEventListener("runtime-events", (event) => {
    try {
      handleHomeEventsPayload(JSON.parse(event.data || "{}"), { broadcastInitial: true });
    } catch (error) {
      console.warn("home event stream returned invalid payload", error);
    }
  });
  source.onerror = () => {
    if (!homeSummarySignedIn(shellState.currentSummary)) {
      stopHomeEventChannel();
      return;
    }
    shellState.homeEventsStreamFailed = true;
    source.close();
    if (shellState.homeEventsSource === source) {
      shellState.homeEventsSource = null;
    }
    scheduleHomeEventPoll(HOME_EVENTS_RETRY_MS);
  };
}

async function pollHomeEvents() {
  if (!homeSummarySignedIn(shellState.currentSummary)) {
    stopHomeEventChannel();
    return;
  }
  if (document.hidden) {
    scheduleHomeEventPoll(HOME_EVENTS_HIDDEN_RETRY_MS);
    return;
  }
  shellState.homeEventsInFlight = true;
  const params = new URLSearchParams({
    wait_ms: String(HOME_EVENTS_WAIT_MS),
  });
  if (shellState.homeEventsCursor) {
    params.set("cursor", shellState.homeEventsCursor);
  }
  const hadCursor = Boolean(shellState.homeEventsCursor);
  try {
    const payload = await fetchJson(`/api/apps/home/events?${params.toString()}`);
    handleHomeEventsPayload(payload, { broadcastInitial: hadCursor });
    scheduleHomeEventPoll(Number(payload.retry_after_ms || HOME_EVENTS_RETRY_MS));
  } catch (error) {
    if (isHomeAuthError(error)) {
      stopHomeEventChannel();
      showHostAuthGate({ presentation: "prompt" }).catch((unlockError) => {
        console.error("home unlock failed", unlockError);
      });
      return;
    }
    console.warn("home event channel failed", error);
    scheduleHomeEventPoll(HOME_EVENTS_RETRY_MS);
  } finally {
    shellState.homeEventsInFlight = false;
  }
}

function handleHomeEventsPayload(payload, { broadcastInitial = true } = {}) {
  if (payload?.schema !== "elastos.home.events/v1") {
    throw new Error("Home updates could not be read.");
  }
  const hadCursor = Boolean(shellState.homeEventsCursor);
  shellState.homeEventsCursor = String(payload.cursor || "");
  const events = Array.isArray(payload.events) ? payload.events : [];
  if ((hadCursor || broadcastInitial || events.length > 0) && events.length > 0) {
    broadcastHomeRuntimeEvents(events);
    if (homeEventsRequireShellSummary(events)) {
      requestShellSummaryRefresh({ reason: "runtime-events", delay: 0 });
    }
  }
}

function homeEventsRequireShellSummary(events) {
  return events.some((event) => {
    const scope = typeof event?.scope === "string" ? event.scope : "";
    const kind = typeof event?.kind === "string" ? event.kind : "";
    return (
      scope === "home" ||
      scope === "inbox" ||
      scope === "wallet" ||
      scope === "people" ||
      kind === "home.summary.changed" ||
      kind === "inbox.changed" ||
      kind === "wallet.requests.changed" ||
      kind === "people.changed" ||
      kind === "home.desktop.changed"
    );
  });
}

function scheduleHomeEventPoll(delayMs) {
  window.clearTimeout(shellState.homeEventsTimer);
  shellState.homeEventsTimer = window.setTimeout(
    pollHomeEvents,
    Math.max(250, Number(delayMs) || HOME_EVENTS_RETRY_MS),
  );
}

function broadcastHomeRuntimeEvents(events) {
  const message = {
    type: "elastos:runtime-events",
    schema: "elastos.home.runtime-events/v1",
    events,
  };
  postToActiveShell(message);
  for (const context of launchedAppContexts.values()) {
    if (context.source) {
      context.source.postMessage(message, OPAQUE_FRAME_TARGET);
    }
  }
}
