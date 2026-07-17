import {
  bindHomeGuiInteractions,
  closeHomeGuiWindowForToken,
  openHomeGuiTarget,
  relaunchHomeGuiWindowForToken,
  restoreHomeGuiSession,
  setHomeGuiMounted,
  showHomeGuiDesktop,
  syncHomeGuiProjection,
} from "./home-gui.js?v=home-20260715a";
import { setHomeGuiLaunchToken } from "./shell-core.js?v=home-20260715a";

const route = new URL(window.location.href);
const fragment = new URLSearchParams(route.hash.replace(/^#/, ""));
const homeToken = fragment.get("home_token") || "";
const homeOrigin = route.searchParams.get("home_origin") || "";
const pendingRequests = new Map();
let currentSummary = null;
let restoredSession = false;

if (!homeToken || !homeOrigin || window.parent === window) {
  throw new Error("Home GUI requires an isolated Home launch");
}
setHomeGuiLaunchToken(homeToken);

route.hash = "";
window.history.replaceState(null, "", `${route.pathname}${route.search}`);

function requestId() {
  return window.crypto?.randomUUID?.()
    || `home-gui-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function postToHome(message) {
  window.parent.postMessage({ ...message, homeToken }, homeOrigin);
}

function requestHome(type, payload = {}) {
  const id = requestId();
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      pendingRequests.delete(id);
      reject(new Error("Home did not answer the shell request"));
    }, 30_000);
    pendingRequests.set(id, { resolve, reject, timeout });
    postToHome({ type, requestId: id, ...payload });
  });
}

function settleRequest(message) {
  const id = typeof message?.requestId === "string" ? message.requestId : "";
  const pending = pendingRequests.get(id);
  if (!pending) {
    return false;
  }
  pendingRequests.delete(id);
  window.clearTimeout(pending.timeout);
  if (message.error) {
    const error = new Error(String(message.error));
    error.status = Number(message.status || 0);
    pending.reject(error);
  } else {
    pending.resolve(message.result);
  }
  return true;
}

async function applySummary(summary, options = {}) {
  const previous = currentSummary;
  currentSummary = summary;
  await syncHomeGuiProjection(previous, summary, {
    initialize: previous === null,
    principalChanged: previous?.authority?.principal_id !== summary?.authority?.principal_id,
    activeShellChanged: previous?.active_shell?.active !== summary?.active_shell?.active,
    activeShellIsHomeGui: true,
    activeShellMode: "desktop",
    homeGuiWasMounted: previous !== null,
    ...options,
  });
  if (!restoredSession && summary?.authority?.signed_in === true) {
    restoredSession = true;
    await restoreHomeGuiSession();
  }
  document.body.dataset.homeStatus = "ready";
}

function handleGuiCommand(message) {
  const command = message?.command;
  if (command === "close-window") {
    return closeHomeGuiWindowForToken(message.homeToken);
  }
  if (command === "relaunch-window") {
    return relaunchHomeGuiWindowForToken(message.homeToken);
  }
  if (command === "open-target") {
    return openHomeGuiTarget(message.target, { query: message.query || {} });
  }
  if (command === "show-desktop") {
    showHomeGuiDesktop();
    return true;
  }
  return false;
}

window.addEventListener("message", (event) => {
  if (event.source !== window.parent || event.origin !== homeOrigin) {
    return;
  }
  const message = event.data && typeof event.data === "object" ? event.data : null;
  if (!message) {
    return;
  }
  if (message.type === "home:shell-response") {
    settleRequest(message);
    return;
  }
  if (message.type === "home:shell-summary") {
    applySummary(message.summary).catch((error) => {
      document.body.dataset.homeStatus = "error";
      console.error("home-gui summary failed", error);
    });
    return;
  }
  if (message.type === "elastos:runtime-events" && Array.isArray(message.events)) {
    postToHome({ type: "home:refresh-summary" });
    return;
  }
  if (message.type === "home:gui-command") {
    handleGuiCommand(message);
  }
});

bindHomeGuiInteractions({
  activateHomeGui: () => Promise.resolve(showHomeGuiDesktop()),
  requestHomeUnlock: () => requestHome("home:request-unlock"),
  requestSummaryRefresh: () => {
    postToHome({ type: "home:refresh-summary" });
  },
  launchTarget: (target, query) => requestHome("home:launch-target", { target, query }),
  signOut: () => requestHome("home:sign-out"),
});
setHomeGuiMounted(true);
postToHome({ type: "home:shell-ready" });
