import {
  bindHomeGuiInteractions,
  attachAuthorizedHomeGuiTarget,
  closeHomeGuiWindowForToken,
  openHomeGuiTarget,
  relaunchHomeGuiWindowForToken,
  restoreHomeGuiSession,
  setHomeGuiMounted,
  showHomeGuiDesktop,
  syncHomeGuiProjection,
} from "./home-gui.js?v=home-20260731a";
import {
  acceptHomeBrowserContextId,
  hasHomeBrowserContextId,
  setHomeGuiLaunchToken,
} from "./shell-core.js?v=home-20260725a";
import {
  isTrustedHomeGuiMessage,
  projectHomeGuiAuthority,
} from "./home-gui-authority.js?v=home-20260731a";

const route = new URL(window.location.href);
const fragment = new URLSearchParams(route.hash.replace(/^#/, ""));
const homeToken = fragment.get("home_token") || "";
const homeOrigin = route.searchParams.get("home_origin") || "";
const pendingRequests = new Map();
const consumedAuthorizedAttachmentReceipts = new Set();
const MAX_AUTHORIZED_ATTACHMENT_RECEIPTS = 128;
const AUTHORIZED_ATTACHMENT_SCHEMA =
  "elastos.home.authorized-target-attachment/v1";
const AUTHORIZED_ATTACHMENT_TARGET_TITLES = Object.freeze({
  "wallet-metamask": "MetaMask",
  "wallet-unisat": "UniSat",
});
let currentSummary = null;
let restoredSession = false;

if (!homeToken || !homeOrigin || window.parent === window) {
  throw new Error("Home GUI requires an isolated Home launch");
}
setHomeGuiLaunchToken(homeToken);

route.hash = "";
window.history.replaceState(null, "", `${route.pathname}${route.search}`);

function requestId() {
  if (window.crypto?.randomUUID) {
    return window.crypto.randomUUID();
  }
  if (window.crypto?.getRandomValues) {
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return `home-gui-${Array.from(
      bytes,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("")}`;
  }
  throw new Error("Home GUI requires browser crypto for request isolation");
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
  projectHomeGuiAuthority(document.body, summary);
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
  await restoreSessionWhenReady();
  document.body.dataset.homeStatus = "ready";
}

async function restoreSessionWhenReady() {
  if (
    restoredSession ||
    currentSummary?.authority?.signed_in !== true ||
    !hasHomeBrowserContextId()
  ) {
    return;
  }
  restoredSession = true;
  await restoreHomeGuiSession();
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
  if (command === "attach-authorized-target") {
    return attachAuthorizedHomeGuiTarget(
      consumeAuthorizedAttachmentDescriptor(message),
    );
  }
  if (command === "show-desktop") {
    showHomeGuiDesktop();
    return true;
  }
  return false;
}

function consumeAuthorizedAttachmentDescriptor(message) {
  if (!hasExactKeys(message, ["type", "command", "descriptor"])) {
    throw new Error("Home GUI rejected a malformed authorized attachment");
  }
  const descriptor = message.descriptor;
  if (
    !descriptor ||
    typeof descriptor !== "object" ||
    Array.isArray(descriptor) ||
    !hasExactKeys(descriptor, [
      "schema",
      "receipt_id",
      "target",
      "title",
      "attach_kind",
      "route",
    ])
  ) {
    throw new Error("Home GUI rejected an invalid authorized attachment");
  }
  const target = typeof descriptor.target === "string" ? descriptor.target : "";
  const title = AUTHORIZED_ATTACHMENT_TARGET_TITLES[target];
  const receiptId = typeof descriptor.receipt_id === "string"
    ? descriptor.receipt_id
    : "";
  const routeValue = typeof descriptor.route === "string" ? descriptor.route : "";
  let launchedRoute;
  try {
    launchedRoute = new URL(routeValue, homeOrigin);
  } catch (_error) {
    throw new Error("Home GUI rejected an invalid authorized route");
  }
  const routeQueryKeys = [...launchedRoute.searchParams.keys()];
  const routeFragment = new URLSearchParams(launchedRoute.hash.replace(/^#/, ""));
  const routeFragmentKeys = [...routeFragment.keys()];
  const launchedToken = routeFragment.get("home_token") || "";
  if (
    descriptor.schema !== AUTHORIZED_ATTACHMENT_SCHEMA ||
    !title ||
    descriptor.title !== title ||
    descriptor.attach_kind !== "iframe" ||
    !receiptId ||
    receiptId.length > 128 ||
    !/^[a-zA-Z0-9-]+$/.test(receiptId) ||
    !routeValue ||
    routeValue.length > 16_384 ||
    launchedRoute.origin !== homeOrigin ||
    launchedRoute.username ||
    launchedRoute.password ||
    launchedRoute.pathname !== `/apps/${target}/` ||
    routeQueryKeys.length !== 1 ||
    routeQueryKeys[0] !== "home_origin" ||
    launchedRoute.searchParams.get("home_origin") !== homeOrigin ||
    routeFragmentKeys.length !== 1 ||
    routeFragmentKeys[0] !== "home_token" ||
    !launchedToken ||
    launchedToken !== launchedToken.trim() ||
    launchedToken.length > 4_096
  ) {
    throw new Error("Home GUI rejected an invalid authorized attachment");
  }
  if (consumedAuthorizedAttachmentReceipts.has(receiptId)) {
    throw new Error("Home GUI rejected a replayed authorized attachment");
  }
  if (consumedAuthorizedAttachmentReceipts.size >= MAX_AUTHORIZED_ATTACHMENT_RECEIPTS) {
    throw new Error("Home GUI authorized attachment capacity reached");
  }
  consumedAuthorizedAttachmentReceipts.add(receiptId);
  return Object.freeze({
    target,
    title,
    attach_kind: "iframe",
    route: routeValue,
  });
}

function hasExactKeys(value, expectedKeys) {
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length &&
    actual.every((key, index) => key === expected[index]);
}

window.addEventListener("message", (event) => {
  if (!isTrustedHomeGuiMessage(event, window.parent, homeOrigin)) {
    return;
  }
  const message = event.data && typeof event.data === "object" ? event.data : null;
  if (!message) {
    return;
  }
  if (message.type === "home:shell-context") {
    if (acceptHomeBrowserContextId(message.browserContextId)) {
      restoreSessionWhenReady().catch((error) => {
        document.body.dataset.homeStatus = "error";
        console.error("home-gui session restore failed", error);
      });
    }
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
    Promise.resolve()
      .then(() => handleGuiCommand(message))
      .catch((error) => {
        console.error("home-gui command failed", error);
      });
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
