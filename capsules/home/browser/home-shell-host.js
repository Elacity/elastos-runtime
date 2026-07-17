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
} from "./shell-core.js?v=home-20260717b";
import {
  bindHomeUnlock,
  hideHomeUnlock,
  isHomeAuthError,
  refreshHomeSession,
  showHomeUnlock,
  signOutHome,
} from "./shell-auth.js?v=home-20260717b";

const SUMMARY_REFRESH_DEBOUNCE_MS = 150;
const SUMMARY_REFRESH_AFTER_INTERACTION_MS = 700;
const HOME_EVENTS_WAIT_MS = 25_000;
const HOME_EVENTS_RETRY_MS = 2_000;
const HOME_EVENTS_HIDDEN_RETRY_MS = 30_000;
const HOME_EVENTS_STREAM_URL = "/api/apps/home/events/stream";
const SESSION_REFRESH_MS = 10 * 60 * 1000;
const ACTIVE_SHELL_HINT_KEY = "elastos.home.active-shell-hint";
const HOME_CLI_SHELL_ID = "home-cli";
const HOME_GUI_MODULE_URL = import.meta.url.startsWith("file:")
  ? new URL("../../home-gui/browser/home-gui.js?v=home-20260717b", import.meta.url).href
  : new URL("../home-gui/home-gui.js?v=home-20260717b", import.meta.url).href;
const SHELL_MESSAGE_OPEN_TARGET_SOURCES = Object.freeze({
  "archive-manager": new Set(["library"]),
  browser: new Set(["library"]),
  "chat-room": new Set(["library"]),
  "home-cli": "visible-target",
  inbox: "visible-target",
  library: new Set(["archive-manager", "documents", "library"]),
  marketplace: "runtime-target",
  services: new Set(["browser", "chat-room"]),
  system: "visible-target",
  "wallet": new Set(["wallet-metamask", "wallet-unisat"]),
});
const SHELL_MESSAGE_OPEN_URI_SOURCES = new Set(["documents", "chat-room"]);
const SHELL_MESSAGE_DELIVER_TARGET_SOURCES = Object.freeze({
  "chat-room": new Set(["documents"]),
  documents: new Set(["chat-room"]),
  library: new Set(["archive-manager", "browser", "chat-room"]),
});
let homeGuiModule = null;
let homeGuiLoadPromise = null;

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

function markHomeGuiDormant(options = {}) {
  document.body.dataset.homeGui = "dormant";
  document.body.dataset.homeShell = "alternate";
  if (options.closeWindows === true) {
    homeGuiModule?.retireHomeGuiSurface?.({ closeWindows: true });
  }
  shellState.homeGuiMounted = false;
}

async function openTargetFromRootShell(context, target, options = {}) {
  await activateDesktopShell(context.homeToken);
  await openTargetFromHomeGui(target, options);
}

function enterHostAuthGate() {
  shellState.activeShellRootLaunchSeq += 1;
  shellState.activeShellRootTarget = "";
  shellState.activeShellRootRoute = "";
  rememberActiveShellHint(HOME_GUI_SHELL_ID);
  dormantHomeGui({ closeWindows: true });
  document.body.dataset.homeShell = "resolving";
  document.body.dataset.homeGui = "dormant";
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

async function ensureHomeGuiModule() {
  if (homeGuiModule) {
    return homeGuiModule;
  }
  if (!homeGuiLoadPromise) {
    homeGuiLoadPromise = import(HOME_GUI_MODULE_URL)
      .then((module) => {
        homeGuiModule = module;
        module.bindHomeGuiInteractions({
          activateHomeGui: () => activateDesktopShell().catch((error) => {
            console.error("home shell activation failed", error);
            showShellHostRecovery(activeShellTarget(shellState.currentSummary), error, {
              title: `Could not switch to ${HOME_GUI_SHELL_ID}`,
              copy: "Reload Home or sign out.",
            });
          }),
          requestHomeUnlock: () => showHostAuthGate({ presentation: "prompt", surface: "neutral" }),
          requestSummaryRefresh: () => refreshShellSummary(),
          signOut: signOutHome,
        });
        return module;
      })
      .catch((error) => {
        homeGuiLoadPromise = null;
        throw error;
      });
  }
  return homeGuiLoadPromise;
}

function dormantHomeGui(options = {}) {
  const closeWindows = options.closeWindows === true;
  if (homeGuiModule?.setHomeGuiMounted) {
    homeGuiModule.setHomeGuiMounted(false, { closeWindows });
    return;
  }
  markHomeGuiDormant({ closeWindows });
}

async function mountHomeGui() {
  const module = await ensureHomeGuiModule();
  module.setHomeGuiMounted(true);
  hideHostBootMask();
  return module;
}

function closeHomeGuiWindowsForTarget(target) {
  if (homeGuiModule?.closeHomeGuiWindowsForTarget) {
    homeGuiModule.closeHomeGuiWindowsForTarget(target);
  }
}

function homeGuiHasWindows() {
  return homeGuiModule?.homeGuiHasWindows?.() === true;
}

function homeGuiInteractionActive() {
  return homeGuiModule?.homeGuiInteractionActive?.() === true;
}

function requireHomeGuiActive(action) {
  if (
    activeShellTarget(shellState.currentSummary) !== HOME_GUI_SHELL_ID ||
    document.body.dataset.homeShell === "alternate" ||
    document.body.dataset.homeGui === "dormant"
  ) {
    throw new Error(`${action} requires active ${HOME_GUI_SHELL_ID}`);
  }
}

async function openTargetFromHomeGui(target, options = {}) {
  requireHomeGuiActive("open target");
  const module = await ensureHomeGuiModule();
  module.openHomeGuiTarget(target, options);
}

async function closeHomeGuiWindowById(windowId) {
  requireHomeGuiActive("close window");
  const module = await ensureHomeGuiModule();
  module.closeHomeGuiWindow(windowId);
}

async function relaunchHomeGuiTarget(windowId, target) {
  requireHomeGuiActive("relaunch window");
  const module = await ensureHomeGuiModule();
  module.relaunchHomeGuiTarget(windowId, target);
}

async function deliverMessageToHomeGuiTargetFrame(target, payload) {
  requireHomeGuiActive("deliver target message");
  const module = await ensureHomeGuiModule();
  return module.deliverMessageToHomeGuiTargetFrame(target, payload);
}

async function setHomeGuiMenuManifest(windowId, menus) {
  requireHomeGuiActive("set menu manifest");
  const module = await ensureHomeGuiModule();
  module.setHomeGuiMenuManifest(windowId, menus);
}

async function openHomeGuiTargetWithPayload(target, payload) {
  requireHomeGuiActive("open target with payload");
  const module = await ensureHomeGuiModule();
  return module.openHomeGuiTargetWithPayload(target, payload);
}

async function showHomeGuiDesktop() {
  const module = await mountHomeGui();
  module.showHomeGuiDesktop();
}

async function restoreHomeGuiSession() {
  const module = await mountHomeGui();
  return module.restoreHomeGuiSession();
}

async function syncHomeGuiProjection(previous, summary, options = {}) {
  if (options.activeShellMode === "locked") {
    return;
  }
  if (options.activeShellMode === "alternate") {
    dormantHomeGui();
    return;
  }
  const module = await mountHomeGui();
  module.syncHomeGuiProjection(previous, summary, options);
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
  if (target === HOME_GUI_SHELL_ID) {
    shellState.activeShellRootTarget = "";
    shellState.activeShellRootRoute = "";
    rememberActiveShellHint(HOME_GUI_SHELL_ID);
    requestShellSummaryRefresh({ reason: "active-shell-applied", delay: 0 });
    return true;
  }
  rememberActiveShellHint(target);
  dormantHomeGui({ closeWindows: true });
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
  dormantHomeGui();
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
  return normalizedActiveShellName(target) === HOME_GUI_SHELL_ID
    ? HOME_GUI_SHELL_ID
    : (target || "unknown shell");
}

function shellHostRecoveryDetailText(error) {
  if (!error) {
    return "";
  }
  if (typeof error === "string") {
    return error;
  }
  return error.message || String(error);
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
    shellHostRecoveryTitle.textContent = "Shell unavailable";
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
    shellHostRecoveryHomeButton.title = `Switch back to ${HOME_GUI_SHELL_ID}`;
  }
}

function showShellHostRecovery(target, error, options = {}) {
  const detail = shellHostRecoveryDetailText(error);
  const tokenAvailable = Boolean(activeShellRootHomeToken());
  dormantHomeGui();
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
        ? `Switch back to ${HOME_GUI_SHELL_ID}, reload Home, or sign out.`
        : "Reload Home or sign out. Switching shells requires an explicit shell launch token."
    );
  }
  if (shellHostRecoveryDetail) {
    shellHostRecoveryDetail.hidden = !detail;
    shellHostRecoveryDetail.textContent = detail;
  }
  if (shellHostRecoveryHomeButton) {
    shellHostRecoveryHomeButton.disabled = !tokenAvailable;
    shellHostRecoveryHomeButton.title = tokenAvailable
      ? `Switch back to ${HOME_GUI_SHELL_ID}`
      : "No shell launch token is available.";
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
      title: `Cannot switch to ${HOME_GUI_SHELL_ID}`,
      copy: "Reload Home or sign out. The host will not use the ambient cookie to change shells.",
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
      title: `Could not switch to ${HOME_GUI_SHELL_ID}`,
      copy: "Reload Home or sign out.",
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

async function clearActiveShellRoot({ mountGui = true, clearHint = false } = {}) {
  if (mountGui) {
    await mountHomeGui();
    rememberActiveShellHint(HOME_GUI_SHELL_ID);
  }
  if (!mountGui && clearHint) {
    rememberActiveShellHint(HOME_GUI_SHELL_ID);
  }
  if (!mountGui) {
    dormantHomeGui();
    document.body.dataset.homeShell = "resolving";
  }
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
    await clearActiveShellRoot({ mountGui: false, clearHint: true });
    return "locked";
  }
  if (target === HOME_GUI_SHELL_ID) {
    if (shouldDeferHomeGuiForBootHint(summary, options)) {
      showHostBootMask();
      await clearActiveShellRoot({ mountGui: false });
      return "alternate";
    }
    await clearActiveShellRoot();
    return "desktop";
  }

  const candidate = activeShellCandidate(summary, target);
  if (!candidate || candidate.launchable !== true) {
    await clearActiveShellRoot();
    return "desktop";
  }

  rememberActiveShellHint(target);
  showHostBootMask();
  const closeStaleGuiWindows = shellState.homeGuiMounted === true || homeGuiHasWindows();
  dormantHomeGui({ closeWindows: closeStaleGuiWindows });
  if (activeShellRoot) {
    activeShellRoot.hidden = false;
    activeShellRoot.dataset.target = target;
  }
  hideShellHostRecovery();
  if (activeShellFrame) {
    activeShellFrame.title = candidate.title || target;
  }
  closeHomeGuiWindowsForTarget(target);

  if (
    shellState.activeShellRootTarget === target &&
    shellState.activeShellRootRoute &&
    activeShellFrame?.dataset.route === shellState.activeShellRootRoute
  ) {
    activeShellFrame.hidden = false;
    hideHostBootMask();
    return "alternate";
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
    const launched = await fetchJson("/api/apps/home/launch", {
      method: "POST",
      body: JSON.stringify({
        target,
        query: { shell_mode: "root" },
      }),
    });
    if (shellState.activeShellRootLaunchSeq !== launchSeq) {
      return "alternate";
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
  return "alternate";
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
    title: "Home unavailable",
    copy: "Reload Home or sign out.",
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
  if (data.type === "home:refresh-summary") {
    requestShellSummaryRefresh({ reason: "child-message" });
    return;
  }
  if (data.type === "home:menu-manifest") {
    // Menus are self-declared UI, not authority: a window may only shape its
    // OWN menu bar entry, so the manifest binds to the sender's window id.
    if (context.kind !== "app-frame" || !context.windowId) {
      console.warn("home ignored unauthorized menu-manifest message", context.targetId);
      return;
    }
    setHomeGuiMenuManifest(context.windowId, data.menus).catch((error) => {
      console.error("home menu-manifest failed", error);
    });
    return;
  }
  if (data.type === "home:active-shell-applied") {
    if (context.kind !== "app-frame" || context.targetId !== SYSTEM_APP_ID) {
      console.warn("home ignored unauthorized active-shell-applied message", context.targetId);
      return;
    }
    preclaimActiveShellSwitch(data.activeShell);
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
    if (context.kind !== "app-frame" || !context.windowId) {
      console.warn("home ignored unauthorized close-self message", context.targetId);
      return;
    }
    closeHomeGuiWindowById(context.windowId).catch((error) => {
      console.error("home close-self failed", error);
    });
    return;
  }
  if (data.type === "home:relaunch-self") {
    if (context.kind !== "app-frame" || !context.windowId || !context.targetId) {
      console.warn("home ignored unauthorized relaunch-self message", context.targetId);
      return;
    }
    const target = context.targetId;
    relaunchHomeGuiTarget(context.windowId, target).catch((error) => {
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
    openTargetFromRootShell(context, target, { query }).catch((error) => {
      console.error("home shell open-target failed", error);
      showShellHostRecovery(context.targetId, error, {
        title: `Could not open ${target}`,
        copy: "Reload Home or sign out.",
      });
    });
    return;
  }
  openTargetFromHomeGui(target, { query }).catch((error) => {
    console.error("home open-target failed", error);
  });
});

function homeMessageContext(event, data) {
  if (event.origin !== window.location.origin || !data || typeof data !== "object") {
    return null;
  }
  if (event.source === window) {
    return { kind: "home", targetId: HOME_GUI_SHELL_ID };
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
  return homeGuiModule?.homeGuiMessageContextForSource?.(event.source, homeToken) || null;
}

function homeLaunchTokenFromRoute(route) {
  try {
    return new URL(route, window.location.href).searchParams.get("home_token") || "";
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
    summary = await refreshShellSummary({
      initialize: true,
      deferHomeGuiForBootHint,
    });
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
  runtimeReady.then(async () => {
    const refreshed = await refreshShellSummary();
    if (activeShellTarget(refreshed) === HOME_GUI_SHELL_ID) {
      await restoreHomeGuiSession();
    }
  }).catch((error) => {
    console.error("home summary refresh failed after runtime ensure", error);
  });
  refreshSignedHomeSession();

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
  const nextDelay = homeGuiInteractionActive()
    ? Math.max(delay, SUMMARY_REFRESH_AFTER_INTERACTION_MS)
    : delay;
  shellState.summaryRefreshDebounceTimer = window.setTimeout(() => {
    if (document.hidden || homeGuiInteractionActive()) {
      requestShellSummaryRefresh({ reason, delay: SUMMARY_REFRESH_AFTER_INTERACTION_MS });
      return;
    }
    if (shellState.summaryRefreshInFlight) {
      requestShellSummaryRefresh({ reason, delay: SUMMARY_REFRESH_AFTER_INTERACTION_MS });
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
  }, nextDelay);
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
  initialize = false,
  deferHomeGuiForBootHint = false,
  deferHomeGuiForRuntimeSettle = false,
} = {}) {
  const summary = await fetchJson("/api/apps/home/summary");
  const previous = shellState.currentSummary;
  shellState.currentSummary = summary;
  shellState.requestSummaryRefresh = refreshShellSummary;
  document.body.dataset.homeAuthority = homeSummarySignedIn(summary) ? "signed" : "unsigned";

  const principalChanged = browserStatePrincipal(previous) !== browserStatePrincipal(summary);

  if (homeSummarySignedIn(summary)) {
    ensureHomeEventChannel();
  } else {
    stopHomeEventChannel();
  }

  const activeShellChanged = activeShellTarget(previous) !== activeShellTarget(summary);
  const homeGuiWasMounted = shellState.homeGuiMounted === true;
  const activeShellMode = await syncActiveShellRoot(summary, {
    deferHomeGuiForBootHint,
    deferHomeGuiForRuntimeSettle,
  });
  await syncHomeGuiProjection(previous, summary, {
    initialize,
    principalChanged,
    activeShellChanged,
    activeShellIsHomeGui: activeShellTarget(summary) === HOME_GUI_SHELL_ID,
    activeShellMode,
    homeGuiWasMounted,
  });
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
    throw new Error("Home event channel returned an invalid schema.");
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
  homeGuiModule?.broadcastHomeGuiRuntimeEvents?.(events);
}

function browserStatePrincipal(summary) {
  return typeof summary?.browser_state?.principal_id === "string"
    ? summary.browser_state.principal_id
    : "";
}
