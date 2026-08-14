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
  homeAuthorityTokenValue,
  targetById,
} from "./shell-core.js?v=home-20260807a";
import {
  bindHomeUnlock,
  hideHomeUnlock,
  isHomeAuthError,
  refreshHomeSession,
  requestPasskeyStepUp,
  showHomeUnlock,
  signOutHome,
} from "./shell-auth.js?v=home-20260807a";
import {
  handleHomeWalletConnectorEffect,
  WALLET_CONNECTOR_EFFECT_TYPE,
} from "./home-wallet-connector-host.js?v=home-20260807a";
import {
  createHomeClipboardFrameState,
  createHomeClipboardHost,
  createHomeClipboardPrompt,
  homeClipboardTargetSupported,
} from "./home-clipboard-host.js?v=home-20260807a";
import { loadOrCreateHomeBrowserContextId } from "./home-browser-context.js?v=home-20260807a";
import {
  createHomeSpendPrompt,
  SPEND_DECLINED_MESSAGE,
} from "./home-spend-prompt.js?v=home-20260807a";

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
const BROWSER_AUTHORITY_RENEWAL_GUI_TIMEOUT_MS = 30_000;
const BROWSER_AUTHORITY_RENEWAL_TIMEOUT_MS = 35_000;
const BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE =
  "elastos.home.browser-authority-renew.request/v1";
const BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE =
  "elastos.home.browser-authority-renew.result/v1";
const HOME_GUI_AUTHORIZED_ATTACHMENT_SCHEMA =
  "elastos.home.authorized-target-attachment/v1";
const WALLET_CONNECTOR_TARGET_TITLES = Object.freeze({
  "wallet-metamask": "MetaMask",
  "wallet-unisat": "UniSat",
});
const WALLET_CONNECTOR_TARGETS = new Set(
  Object.keys(WALLET_CONNECTOR_TARGET_TITLES),
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
  // The storefront hands a bought-and-pinned asset off to its runtime viewer; it opens nothing else.
  "marketplace-content": new Set(["ddrm-viewer", "elacity-player", "library"]),
  people: new Set(["chat-room"]),
  services: new Set(["browser", "chat-room"]),
  system: "visible-target",
  "wallet": new Set(["wallet-metamask", "wallet-unisat"]),
});
// Object.freeze is shallow: the nested library Set stays mutable, so owned-asset
// opens from Library into the DRM viewer/player can extend the source gate here
// without reshaping the frozen declaration above (kept literal for auditability).
SHELL_MESSAGE_OPEN_TARGET_SOURCES.library.add("ddrm-viewer").add("elacity-player");
const SHELL_MESSAGE_OPEN_URI_SOURCES = new Set(["documents", "chat-room"]);
const SHELL_MESSAGE_DELIVER_TARGET_SOURCES = Object.freeze({
  "chat-room": new Set(["documents"]),
  documents: new Set(["chat-room"]),
  library: new Set(["archive-manager", "browser", "chat-room"]),
});
const PASSKEY_STEP_UP_TARGETS = new Set(["inbox", SYSTEM_APP_ID, "wallet"]);
// The node-signed money verbs, and who may ask Home to run one.
//
// These are NOT on the `PASSKEY_STEP_UP_TARGETS` bridge and must not be: that bridge mints a
// step-up under the REQUESTING frame's own launch token, and the gateway binds a money-verb
// assertion to the launch that authenticates the spend. `/api/market/buy` and `/api/create/mint`
// accept only a `home` launch token presented from the EXACT Home origin
// (gateway_home_token.rs::require_home_token_launch + require_exact_home_browser_origin), so an
// opaque capsule frame — every app window and the Home GUI shell frame alike — can neither hold
// the right token nor present the right origin. Widening the actor list server-side would hand a
// capsule frame spend authority; instead Home brokers the whole verb here, under its own
// authority, and the requesting frame never sees a step-up token at all.
const MONEY_VERB_ROUTES = Object.freeze({
  "market.buy": "/api/market/buy",
  "create.mint": "/api/create/mint",
});
const MONEY_VERB_REQUEST_TYPE = "elastos.home.money-verb.request/v1";
// Only frames with a REAL caller today. `creator` is deliberately absent: the Create portal
// reaches chain through the capability bus (encrypt-provider -> publish-provider -> wallet
// approval), so nothing in `capsules/creator/browser/creator.js` posts `/api/create/mint`. Listing it
// would enlarge the set of frames that can make Home raise a confirmation prompt and start a
// step-up ceremony — a prompt-fatigue and social-engineering surface — for no capability today.
// Re-adding it is this one line when the Create portal ships a caller.
const MONEY_VERB_APP_SOURCES = new Set(["marketplace-content"]);
const MONEY_VERB_SHELL_SOURCES = new Set([HOME_GUI_SHELL_ID]);
const launchedAppContexts = new Map();
const pendingBrowserAuthorityRenewals = new Map();
const homeSpendPrompt = createHomeSpendPrompt({
  root: document.querySelector("#home-spend-prompt"),
  title: document.querySelector("#home-spend-title"),
  summary: document.querySelector("#home-spend-summary"),
  terms: document.querySelector("#home-spend-terms"),
  allowButton: document.querySelector("#home-spend-allow"),
  cancelButton: document.querySelector("#home-spend-cancel"),
});
const homeClipboardPrompt = createHomeClipboardPrompt({
  root: document.querySelector("#home-clipboard-prompt"),
  title: document.querySelector("#home-clipboard-title"),
  copy: document.querySelector("#home-clipboard-copy"),
  allowButton: document.querySelector("#home-clipboard-allow"),
  cancelButton: document.querySelector("#home-clipboard-cancel"),
});
const homeClipboardHost = createHomeClipboardHost({
  prompt: homeClipboardPrompt,
});

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

async function launchWalletConnectorFromTrustedSource(context, target) {
  if (
    context.kind !== "app-frame" ||
    context.targetId !== "wallet" ||
    !WALLET_CONNECTOR_TARGETS.has(target)
  ) {
    throw new Error("Home denied the Wallet connector launch");
  }
  requireHomeGuiActive("launch Wallet connector");
  const launched = await launchHomeTarget(target);
  const descriptor = walletConnectorAttachmentDescriptor(target, launched);
  requireHomeGuiActive("attach Wallet connector");
  if (!postToActiveShell({
    type: "home:gui-command",
    command: "attach-authorized-target",
    descriptor,
  })) {
    throw new Error("Home GUI did not accept the Wallet connector attachment");
  }
}

function retireLaunchedAppContext(homeToken) {
  const context = launchedAppContexts.get(homeToken);
  if (!context) {
    return false;
  }
  cancelBrowserAuthorityRenewalsForToken(homeToken);
  homeClipboardHost.retireFrame(context.clipboardState);
  launchedAppContexts.delete(homeToken);
  return true;
}

function clearLaunchedAppContexts() {
  clearPendingBrowserAuthorityRenewals();
  for (const context of launchedAppContexts.values()) {
    homeClipboardHost.retireFrame(context.clipboardState);
  }
  launchedAppContexts.clear();
}

function enterHostAuthGate() {
  shellState.activeShellRootLaunchSeq += 1;
  shellState.activeShellRootTarget = "";
  shellState.activeShellRootRoute = "";
  rememberActiveShellHint(HOME_GUI_SHELL_ID);
  document.body.dataset.homeShell = "resolving";
  document.body.dataset.homeGui = "dormant";
  clearLaunchedAppContexts();
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
  enterHostAuthGate();
  const unlockReady = showHomeUnlock(() => boot(), {
    ...options,
    surface: "neutral",
  });
  hideHostBootMask();
  await unlockReady;
}

async function launchHomeTarget(target, query = {}) {
  const body = {
    target,
    query: {
      ...query,
      home_origin: window.location.origin,
    },
  };
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
  retireLaunchedAppContext(homeToken);
}

function postBrowserAuthorityRenewalResult(record, result) {
  try {
    record.source.postMessage(
      {
        type: BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE,
        requestId: record.requestId,
        homeToken: record.oldHomeToken,
        browserInstance: record.browserInstance,
        ok: result.ok,
        freshHomeToken: result.freshHomeToken,
        reason: result.reason,
      },
      OPAQUE_FRAME_TARGET,
    );
  } catch (error) {
    console.warn("home could not return Browser authority renewal", error);
  }
}

function cancelBrowserAuthorityRenewalsForToken(homeToken) {
  for (const [requestId, record] of pendingBrowserAuthorityRenewals) {
    if (record.oldHomeToken !== homeToken) {
      continue;
    }
    window.clearTimeout(record.timeout);
    pendingBrowserAuthorityRenewals.delete(requestId);
  }
}

function clearPendingBrowserAuthorityRenewals() {
  for (const record of pendingBrowserAuthorityRenewals.values()) {
    window.clearTimeout(record.timeout);
  }
  pendingBrowserAuthorityRenewals.clear();
}

function settleBrowserAuthorityRenewal(record, result) {
  if (pendingBrowserAuthorityRenewals.get(record.requestId) !== record) {
    return false;
  }
  window.clearTimeout(record.timeout);
  pendingBrowserAuthorityRenewals.delete(record.requestId);
  if (result.ok) {
    retireLaunchedAppContext(record.oldHomeToken);
  }
  postBrowserAuthorityRenewalResult(record, result);
  return true;
}

function startBrowserAuthorityRenewal(event, context, message) {
  const requestId =
    typeof message.requestId === "string" ? message.requestId.trim() : "";
  const browserInstance =
    typeof message.browserInstance === "string"
      ? message.browserInstance.trim()
      : "";
  if (
    context.kind !== "app-frame" ||
    context.targetId !== "browser" ||
    !hasExactMessageKeys(message, [
      "type",
      "requestId",
      "homeToken",
      "browserInstance",
    ]) ||
    message.type !== BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE ||
    !requestId ||
    requestId !== message.requestId ||
    requestId.length > 128 ||
    !browserInstance ||
    browserInstance !== message.browserInstance ||
    browserInstance.length > 256 ||
    context.browserInstance !== browserInstance ||
    context.source !== event.source
  ) {
    return false;
  }
  const pendingForFrame = [...pendingBrowserAuthorityRenewals.values()].find(
    (record) =>
      record.oldHomeToken === context.homeToken &&
      record.browserInstance === browserInstance &&
      record.source === event.source,
  );
  if (pendingForFrame) {
    if (pendingForFrame.requestId !== requestId) {
      postBrowserAuthorityRenewalResult(
        {
          requestId,
          oldHomeToken: context.homeToken,
          browserInstance,
          source: event.source,
        },
        {
          ok: false,
          freshHomeToken: "",
          reason: "renewal_in_flight",
        },
      );
    }
    return true;
  }
  if (pendingBrowserAuthorityRenewals.has(requestId)) {
    return false;
  }
  const record = {
    requestId,
    oldHomeToken: context.homeToken,
    browserInstance,
    source: event.source,
    timeout: 0,
  };
  record.timeout = window.setTimeout(() => {
    settleBrowserAuthorityRenewal(record, {
      ok: false,
      freshHomeToken: "",
      reason: "gui_timeout",
    });
  }, BROWSER_AUTHORITY_RENEWAL_TIMEOUT_MS);
  pendingBrowserAuthorityRenewals.set(requestId, record);
  try {
    requireHomeGuiActive("renew Browser authority");
    if (!postToActiveShell({
      type: "home:gui-command",
      command: "renew-browser-authority",
      requestId,
      oldHomeToken: context.homeToken,
      browserInstance,
      expiresAt: Date.now() + BROWSER_AUTHORITY_RENEWAL_GUI_TIMEOUT_MS,
    })) {
      throw new Error("Home GUI did not accept Browser authority renewal");
    }
  } catch (_error) {
    settleBrowserAuthorityRenewal(record, {
      ok: false,
      freshHomeToken: "",
      reason: "gui_unavailable",
    });
  }
  return true;
}

function handleBrowserAuthorityRenewalResult(context, message) {
  if (
    context.kind !== "shell-frame" ||
    context.targetId !== HOME_GUI_SHELL_ID ||
    !hasExactMessageKeys(message, [
      "type",
      "requestId",
      "oldHomeToken",
      "browserInstance",
      "ok",
      "freshHomeToken",
      "reason",
      "homeToken",
    ]) ||
    message.type !== BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE
  ) {
    return false;
  }
  const record = pendingBrowserAuthorityRenewals.get(message.requestId);
  if (
    !record ||
    message.oldHomeToken !== record.oldHomeToken ||
    message.browserInstance !== record.browserInstance ||
    typeof message.ok !== "boolean" ||
    typeof message.freshHomeToken !== "string" ||
    message.freshHomeToken.length > 4_096 ||
    typeof message.reason !== "string" ||
    message.reason.length > 128
  ) {
    return false;
  }
  if (!message.ok) {
    if (message.freshHomeToken || !message.reason) {
      return false;
    }
    return settleBrowserAuthorityRenewal(record, {
      ok: false,
      freshHomeToken: "",
      reason: message.reason,
    });
  }
  const oldContext = launchedAppContexts.get(record.oldHomeToken);
  const freshContext = launchedAppContexts.get(message.freshHomeToken);
  if (
    message.reason !== "" ||
    !message.freshHomeToken ||
    message.freshHomeToken === record.oldHomeToken ||
    oldContext?.targetId !== "browser" ||
    oldContext.browserInstance !== record.browserInstance ||
    oldContext.source !== record.source ||
    freshContext?.targetId !== "browser" ||
    freshContext.browserInstance !== record.browserInstance
  ) {
    return false;
  }
  return settleBrowserAuthorityRenewal(record, {
    ok: true,
    freshHomeToken: message.freshHomeToken,
    reason: "",
  });
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
    retireLaunchedAppContext(launchedAppContexts.keys().next().value);
  }
  launchedAppContexts.set(token, {
    targetId: launched.target,
    browserInstance:
      launched.target === "browser"
        ? browserInstanceFromRoute(launched.route || "")
        : "",
    origin: OPAQUE_CAPSULE_ORIGIN,
    parentOrigin: window.location.origin,
    source: null,
    clipboardState:
      homeClipboardTargetSupported(launched.target)
        ? createHomeClipboardFrameState()
        : null,
    walletEffectState: {
      inFlight: false,
      requestIds: new Set(),
    },
  });
}

function walletConnectorAttachmentDescriptor(target, launched) {
  const route = typeof launched?.route === "string" ? launched.route : "";
  let resolved;
  try {
    resolved = new URL(route, window.location.href);
  } catch (_error) {
    throw new Error("Runtime returned an invalid Wallet connector route");
  }
  const routeQueryKeys = [...resolved.searchParams.keys()];
  const routeFragment = new URLSearchParams(resolved.hash.replace(/^#/, ""));
  const routeFragmentKeys = [...routeFragment.keys()];
  const homeLaunchToken = routeFragment.get("home_token") || "";
  if (
    launched?.target !== target ||
    launched?.attach_kind !== "iframe" ||
    (launched?.launch_status && launched.launch_status !== "launched") ||
    !route ||
    route.length > 16_384 ||
    resolved.origin !== window.location.origin ||
    resolved.username ||
    resolved.password ||
    resolved.pathname !== `/apps/${target}/` ||
    routeQueryKeys.length !== 1 ||
    routeQueryKeys[0] !== "home_origin" ||
    resolved.searchParams.get("home_origin") !== window.location.origin ||
    routeFragmentKeys.length !== 1 ||
    routeFragmentKeys[0] !== "home_token" ||
    !homeLaunchToken ||
    homeLaunchToken !== homeLaunchToken.trim() ||
    homeLaunchToken.length > 4_096
  ) {
    throw new Error("Runtime returned an invalid Wallet connector launch");
  }
  return {
    schema: HOME_GUI_AUTHORIZED_ATTACHMENT_SCHEMA,
    receipt_id: walletConnectorAttachmentReceiptId(),
    target,
    title: WALLET_CONNECTOR_TARGET_TITLES[target],
    attach_kind: "iframe",
    route,
  };
}

function walletConnectorAttachmentReceiptId() {
  if (typeof window.crypto?.randomUUID === "function") {
    return window.crypto.randomUUID();
  }
  if (typeof window.crypto?.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  }
  throw new Error("Home requires browser crypto for connector attachment isolation");
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
  clearLaunchedAppContexts();
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
    clearLaunchedAppContexts();
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
    clearLaunchedAppContexts();
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
  clearLaunchedAppContexts();
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
  clearLaunchedAppContexts();
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
    if (context.kind === "shell-frame") {
      if (context.targetId === HOME_GUI_SHELL_ID) {
        let browserContextId = "";
        try {
          browserContextId = loadOrCreateHomeBrowserContextId(
            window.localStorage,
            window.crypto,
          );
        } catch (_error) {
          browserContextId = "";
        }
        if (browserContextId) {
          postToActiveShell({
            type: "home:shell-context",
            browserContextId,
          });
        }
      }
      if (shellState.currentSummary) {
        postToActiveShell({ type: "home:shell-summary", summary: shellState.currentSummary });
      }
    }
    return;
  }
  if (data.type === "home:app-ready") {
    if (
      context.kind === "app-frame" &&
      context.clipboardState &&
      hasExactMessageKeys(data, ["type", "homeToken"])
    ) {
      homeClipboardHost.resetFrame(context.clipboardState, context);
    }
    return;
  }
  if (homeClipboardHost.handle(event, context, data)) {
    return;
  }
  if (handleHomeWalletConnectorEffect(event, context, data)) {
    return;
  }
  if (data.type === BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE) {
    if (!startBrowserAuthorityRenewal(event, context, data)) {
      console.warn("home ignored invalid Browser authority renewal request");
    }
    return;
  }
  if (data.type === BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE) {
    if (!handleBrowserAuthorityRenewalResult(context, data)) {
      console.warn("home ignored invalid Browser authority renewal result");
    }
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
    showHostAuthGate({ presentation: "prompt", surface: "neutral" }).catch((error) => {
      console.error("home unlock failed", error);
    });
    return;
  }
  if (data.type === "home:refresh-summary") {
    requestShellSummaryRefresh({ reason: "child-message" });
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
  if (data.type === MONEY_VERB_REQUEST_TYPE) {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    const operation = typeof data.operation === "string" ? data.operation.trim() : "";
    const request = data.request
      && typeof data.request === "object"
      && !Array.isArray(data.request)
      ? data.request
      : null;
    if (
      !canRunMoneyVerbFromHomeMessage(context)
      || !hasExactMessageKeys(data, ["type", "requestId", "homeToken", "operation", "request"])
      || !requestId
      || requestId.length > 128
      || !Object.prototype.hasOwnProperty.call(MONEY_VERB_ROUTES, operation)
      || !request
      || !isBoundedStepUpRequest(request)
    ) {
      console.warn("home ignored unauthorized money verb", context.targetId);
      return;
    }
    runMoneyVerb(operation, request)
      .then((result) => replyToShellRequest(event, requestId, result))
      .catch((error) => replyToShellRequest(event, requestId, null, error));
    return;
  }
  if (data.type === "elastos.home.passkey-step-up.request/v1") {
    const requestId = typeof data.requestId === "string" ? data.requestId.trim() : "";
    const operation = typeof data.operation === "string" ? data.operation.trim() : "";
    const request = data.request
      && typeof data.request === "object"
      && !Array.isArray(data.request)
      ? data.request
      : null;
    if (
      context.kind !== "app-frame"
      || !PASSKEY_STEP_UP_TARGETS.has(context.targetId)
      || !hasExactMessageKeys(data, ["type", "requestId", "homeToken", "operation", "request"])
      || !requestId
      || requestId.length > 128
      || !operation
      || operation.length > 128
      || !request
      || !isBoundedStepUpRequest(request)
    ) {
      console.warn("home ignored unauthorized passkey request", context.targetId);
      return;
    }
    const reply = (payload) => {
      try {
        event.source?.postMessage({
          type: "elastos.home.passkey-step-up.result/v1",
          requestId,
          ...payload,
        }, OPAQUE_FRAME_TARGET);
      } catch (error) {
        console.error("home could not return passkey result", error);
      }
    };
    requestPasskeyStepUp(context.homeToken, operation, request)
      .then((stepUpToken) => reply({ stepUpToken }))
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
  if (
    context.kind === "app-frame" &&
    context.targetId === "wallet" &&
    WALLET_CONNECTOR_TARGETS.has(target)
  ) {
    if (!hasExactMessageKeys(data, ["type", "target", "homeToken"])) {
      console.warn("home ignored malformed Wallet connector launch", target);
      return;
    }
    launchWalletConnectorFromTrustedSource(context, target).catch((error) => {
      console.error("home Wallet connector launch failed", error);
    });
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
  const walletConnectorEffect = data.type === WALLET_CONNECTOR_EFFECT_TYPE;
  const tokenValue = walletConnectorEffect
    ? data.connectorToken
    : data.homeToken;
  const homeToken = typeof tokenValue === "string" ? tokenValue.trim() : "";
  if (!homeToken || (walletConnectorEffect && tokenValue !== homeToken)) {
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
  if (!launched.source) {
    if (
      data.type !== "home:app-ready" ||
      !hasExactMessageKeys(data, ["type", "homeToken"])
    ) {
      return null;
    }
    launched.source = event.source;
  } else if (launched.source !== event.source) {
    return null;
  }
  return {
    kind: "app-frame",
    targetId: launched.targetId,
    homeToken,
    origin: launched.origin,
    parentOrigin: launched.parentOrigin,
    source: launched.source,
    clipboardState: launched.clipboardState,
    walletEffectState: launched.walletEffectState,
    browserInstance: launched.browserInstance,
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

function browserInstanceFromRoute(route) {
  try {
    return new URL(route, window.location.href).searchParams.get("browser_instance") || "";
  } catch (_error) {
    return "";
  }
}

function hasExactMessageKeys(message, expectedKeys) {
  const actual = Object.keys(message).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length
    && actual.every((key, index) => key === expected[index]);
}

// Run one money verb on behalf of an authorized frame.
//
// Three things must be true before this node signs, and all three are enforced below:
// the caller is an admitted frame (checked at the message boundary), the USER agreed to these
// exact terms in Home's own chrome (3), and the authenticator confirmed it (the step-up).
//
// Two invariants the gateway enforces and this must match exactly:
//  1. The step-up assertion is bound to (operation, sha256(body minus `step_up_token`)) — so the
//     ceremony signs over `request` verbatim and the POST body is `request` plus the token, with
//     nothing added, dropped, or reordered in between.
//  2. Authority is the SameSite=Strict Home session cookie the browser attaches to this
//     same-origin POST. `fetchJson` deliberately sends `x-elastos-home-token` only on
//     `/api/apps/home/launch`: a header here would be a second launch-token authority, and
//     `gateway_home_token.rs::home_launch_token_credential` hard-fails on any disagreement.
async function runMoneyVerb(operation, request) {
  const route = MONEY_VERB_ROUTES[operation];
  if (!route) {
    throw new Error("Home refused an unrecognized money verb");
  }
  let appToken = homeAuthorityTokenValue();
  if (!appToken) {
    await refreshHomeSession();
    appToken = homeAuthorityTokenValue();
  }
  if (!appToken) {
    throw new Error("Home is not signed in — unlock Home before spending");
  }
  // 3. The user agreed to THESE terms. The step-up ceremony proves a human touched the
  //    authenticator; it names no terms, so on its own it cannot distinguish the purchase the
  //    user saw from one a compromised frame substituted. Home therefore renders the intent in
  //    its OWN chrome first (home-spend-prompt.js — every field of `request`, textContent only)
  //    and requires an explicit confirm. This runs BEFORE the ceremony so a declined spend never
  //    prompts the authenticator at all.
  if (!(await homeSpendPrompt.confirm(operation, request))) {
    throw new Error(SPEND_DECLINED_MESSAGE);
  }
  const stepUpToken = await requestPasskeyStepUp(appToken, operation, request);
  return fetchJson(route, {
    method: "POST",
    body: JSON.stringify({ ...request, step_up_token: stepUpToken }),
  });
}

function canRunMoneyVerbFromHomeMessage(context) {
  if (context.kind === "app-frame") {
    return MONEY_VERB_APP_SOURCES.has(context.targetId);
  }
  if (context.kind === "shell-frame") {
    return MONEY_VERB_SHELL_SOURCES.has(context.targetId);
  }
  return false;
}

function isBoundedStepUpRequest(request) {
  try {
    const serialized = JSON.stringify(request);
    return typeof serialized === "string"
      && new TextEncoder().encode(serialized).byteLength <= 65_536;
  } catch (_error) {
    return false;
  }
}

function canOpenUriFromHomeMessage(context) {
  return context.kind === "home" || SHELL_MESSAGE_OPEN_URI_SOURCES.has(context.targetId);
}

function canOpenTargetFromHomeMessage(context, target) {
  if (context.kind === "home") {
    return true;
  }
  const policy = SHELL_MESSAGE_OPEN_TARGET_SOURCES[context.targetId];
  if (!policy) {
    return false;
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
  const deferHomeGuiForBootHint = Boolean(activeShellBootHintTarget());
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
      await showHostAuthGate();
      return;
    }
    throw error;
  }
  if (!homeSummarySignedIn(summary)) {
    document.body.dataset.homeStatus = "ready";
    await showHostAuthGate({ presentation: "prompt" });
    startShellTimers();
    return;
  }
  const runtimeReady = fetchJson("/api/apps/home/runtime/ensure", { method: "POST" })
    .catch((error) => {
      console.error("home runtime ensure failed", error);
      return null;
    });
  document.body.dataset.homeStatus = "ready";
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
        showHostAuthGate().catch((unlockError) => {
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
