import { createBrowserLocationController } from "./browser-location.js?v=browser-20260523b";
import {
  DEFAULT_URL,
  createRuntimeApi,
  isBrowserErrorUrl,
  normalizeUrl,
  sameBrowserStreamTarget,
  visibleAddressForUrl,
} from "./browser-runtime-api.js?v=browser-20260907c";
import {
  createBrowserClipboardBridge,
} from "./browser-clipboard.js?v=browser-20260725b";
import {
  createHomeClipboardClient,
} from "/apps/home/home-clipboard-client.js?v=home-20260726a";
import {
  createRuntimePageCleanupController,
  runtimePageOwner,
  sameRuntimePageOwner,
} from "./browser-page-cleanup.js?v=browser-20260727a";
import { selkiesMessagesForInput } from "./browser-input.js?v=browser-20260520e";
import { bindBrowserInputSurface } from "./browser-input-surface.js?v=browser-20260725b";
import {
  browserMetricsText,
  friendlyOpenError,
  isAuthoritySessionError,
  isMissingRuntimePageError,
  requestedDisplayMode,
} from "./browser-status.js?v=browser-20260907c";
import { createBrowserRemoteDisplay } from "./browser-remote-display.js?v=browser-20260915d";
import { renderServiceSelection } from "./browser-service-selection.js?v=browser-20260907c";
import { createOperatorApprovalController, mountOperatorApproval } from "./browser-operator-approval.js";

const STATUS_TTL_MS = 4200;
const PAGE_STATUS_INTERVAL_MS = 2_500;
const PAGE_STATUS_FIRST_POLL_MS = 1200;
const PAGE_STATUS_AFTER_INPUT_DELAY_MS = 650;
const PAGE_STATUS_AFTER_SCROLL_DELAY_MS = 1200;
const PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS = [650, 1800, 3500, 6500];
const PAGE_HEARTBEAT_INTERVAL_MS = 60_000;
const BROWSER_OPEN_POLL_INTERVAL_MS = 1_200;
const BROWSER_OPEN_POLL_TIMEOUT_MS = 5 * 60_000;
const LIBRARY_FILE_PICKER_MAX_BYTES = 16 * 1024 * 1024;
const PRODUCT_DISPLAY_MODE = "webrtc_remote_display";
const GUARANTEE_MECHANISM_MICROVM = "mechanism_microvm";
const GUARANTEE_OPERATOR_RBI = "operator_rbi";
const GUARANTEE_POLICY_WEBVIEW = "policy_webview";
const LOCAL_EXIT_LABEL = "This device";
const LOCAL_EXIT_SUMMARY = "Use this device's Exit Node for Browser traffic.";
const DEFAULT_ENGINE_LABEL = "Automatic";
const DEFAULT_ENGINE_SUMMARY = "Runtime chooses a compatible Browser Engine.";
const BROWSER_WINDOW_CLOSE_REQUEST_TYPE =
  "elastos.browser.window-close.request/v1";
const BROWSER_WINDOW_CLOSE_RESULT_TYPE =
  "elastos.browser.window-close.result/v1";
const BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE =
  "elastos.home.browser-authority-renew.request/v1";
const BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE =
  "elastos.home.browser-authority-renew.result/v1";
const BROWSER_AUTHORITY_RENEWAL_ACK_TIMEOUT_MS = 40_000;
const BROWSER_AUTHORITY_RENEWAL_RETRY_DELAYS_MS = Object.freeze([
  1_200,
  3_000,
  10_000,
  30_000,
]);
const HOME_GUI_OPAQUE_ORIGIN = "null";
const params = new URLSearchParams(window.location.search);
const browserInstanceId = params.get("browser_instance") || "";
const launchToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeParentOrigin = params.get("home_origin") || "";
const debugMetrics =
  params.get("debug") === "1" || params.get("metrics") === "1";
const { fetchJson, requireViewer } = createRuntimeApi({ launchToken });
const homeClipboard = createHomeClipboardClient({
  targetId: "browser",
  homeOrigin: homeParentOrigin,
  homeToken: launchToken,
});
homeClipboard.start();

const form = document.querySelector("#browser-form");
const addressInput = document.querySelector("#browser-url");
const backButton = document.querySelector("#browser-back");
const forwardButton = document.querySelector("#browser-forward");
const refreshButton = document.querySelector("#browser-refresh");
const profileResetButton = document.querySelector("#browser-profile-reset");
const settingsButton = document.querySelector("#browser-settings");
const settingsPanel = document.querySelector("#browser-settings-panel");
const settingsCloseButton = document.querySelector("#browser-settings-close");
const engineSelect = document.querySelector("#browser-engine");
const engineSummaryNode = document.querySelector("#browser-engine-summary");
const exitSelect = document.querySelector("#browser-exit");
const exitSummaryNode = document.querySelector("#browser-exit-summary");
const statusNode = document.querySelector("#browser-status");
const renderPanel = document.querySelector("#browser-render-panel");
const remoteVideo = document.querySelector("#browser-remote-display");
const keyboardCapture = document.querySelector("#browser-keyboard-capture");
const renderEmpty = document.querySelector("#browser-render-empty");
const metricsNode = document.querySelector("#browser-metrics");

let currentPage = null;
let currentPageGeneration = 0;
let nextPageGeneration = 1;
const MAX_PENDING_BROWSER_INPUTS = 128;
let browserInputQueue = null;
let restoredViewerOwner = null;
let currentView = null;
let currentDisplayMode = "";
let currentDisplayInput = "runtime_route";
let currentDisplayInputProtocol = "elastos_json";
let statusTimer = 0;
let canGoBack = false;
let canGoForward = false;
let pageStatusTimer = 0;
let pageStatusRefreshTimers = [];
let pageHeartbeatTimer = 0;
let lastPageStatus = null;
let unloadCleanupStarted = false;
let recoverableDisplayAttachStarted = false;
let displayAttachRetryIdentity = null;
let remoteDisplay = null;
let relaunchRequested = false;
let browserAuthorityRenewal = null;
let browserAuthorityRenewalRetryTimer = 0;
let browserAuthorityRenewalAttempts = 0;
let lastRequestedUrl = DEFAULT_URL;
let lastLibraryFilePickerRequestId = "";
const libraryPickerDocumentNonce = crypto.randomUUID();
let libraryPickerRequest = null;
let browserSummary = null;
let browserSummaryPromise = null;
let runtimeOpenInFlight = 0;
let unsettledRuntimeOpen = null;
let runtimeOwnershipTerminallyAbsent = false;
let homeWindowCloseInFlight = false;
let homeWindowTerminalCloseConfirmed = false;
let pendingHomeWindowCloseDelivery = null;
let selectedBrowserEngineId = params.get("browser_engine_id") || params.get("adapter_id") || "";
let currentBrowserEngineId = "";
let selectedRemoteExitId = params.get("remote_exit_id") || "";
let currentRemoteExitId = "";

function setSettingsOpen(open) {
  if (!settingsPanel || !settingsButton) {
    return;
  }
  settingsPanel.hidden = !open;
  settingsButton.setAttribute("aria-expanded", open ? "true" : "false");
}

function focusRemoteInput() {
  const target = keyboardCapture || renderPanel;
  target.focus({ preventScroll: true });
  if (keyboardCapture) {
    keyboardCapture.value = "";
    keyboardCapture.setSelectionRange?.(0, 0);
  }
}

const browserLocation = createBrowserLocationController({
  addressInput,
  updateNavState,
});
const {
  clearAddressDraft,
  getCurrentUrl,
  isAddressEditing,
  markAddressDraftEdited,
  resetAddressToCurrent,
  setCurrentUrl,
  syncBrowserLocation,
} = browserLocation;

function showStatus(message, { sticky = false } = {}) {
  window.clearTimeout(statusTimer);
  statusNode.replaceChildren();
  const textNode = document.createElement("span");
  textNode.className = "browser-status-message";
  textNode.textContent = message;
  statusNode.append(textNode);
  const canCopy = Boolean(sticky && message && homeClipboard.canRequest());
  statusNode.dataset.copyable = canCopy ? "true" : "false";
  if (canCopy) {
    const copyButton = document.createElement("button");
    copyButton.className = "browser-status-copy";
    copyButton.type = "button";
    copyButton.textContent = "Copy";
    copyButton.setAttribute("aria-label", "Copy Browser status message");
    copyButton.addEventListener("click", async (event) => {
      event.preventDefault();
      event.stopPropagation();
      try {
        await homeClipboard.writeText(message);
        copyButton.textContent = "Copied";
      } catch {
        copyButton.textContent = "Copy failed";
      }
      window.setTimeout(() => {
        copyButton.textContent = "Copy";
      }, 1200);
    });
    statusNode.append(copyButton);
  }
  statusNode.dataset.visible = "true";
  if (!sticky) {
    statusTimer = window.setTimeout(() => {
      statusNode.dataset.visible = "false";
    }, STATUS_TTL_MS);
  }
}

function browserAuthorityRenewalRequestId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return window.crypto.randomUUID();
  }
  if (window.crypto && typeof window.crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return `browser-renew-${Array.from(
      bytes,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("")}`;
  }
  throw new Error("Browser requires browser crypto for authority renewal");
}

function clearBrowserAuthorityRenewalRequest() {
  if (!browserAuthorityRenewal) {
    return;
  }
  window.clearTimeout(browserAuthorityRenewal.timeout);
  browserAuthorityRenewal = null;
}

function postBrowserAuthorityRenewalRequest(reason) {
  const requestId = browserAuthorityRenewalRequestId();
  const record = {
    requestId,
    reason,
    timeout: 0,
  };
  browserAuthorityRenewalAttempts = Math.min(
    browserAuthorityRenewalAttempts + 1,
    BROWSER_AUTHORITY_RENEWAL_RETRY_DELAYS_MS.length,
  );
  record.timeout = window.setTimeout(() => {
    if (browserAuthorityRenewal === record) {
      settleBrowserAuthorityRenewal(false, "host_timeout");
    }
  }, BROWSER_AUTHORITY_RENEWAL_ACK_TIMEOUT_MS);
  browserAuthorityRenewal = record;
  window.top.postMessage(
    {
      type: BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE,
      requestId,
      homeToken: launchToken,
      browserInstance: browserInstanceId,
    },
    homeParentOrigin,
  );
  return true;
}

function scheduleBrowserAuthorityRenewalRetry(reason) {
  const retryIndex = Math.max(
    0,
    Math.min(
      browserAuthorityRenewalAttempts - 1,
      BROWSER_AUTHORITY_RENEWAL_RETRY_DELAYS_MS.length - 1,
    ),
  );
  const delay = BROWSER_AUTHORITY_RENEWAL_RETRY_DELAYS_MS[retryIndex];
  browserAuthorityRenewalRetryTimer = window.setTimeout(() => {
    browserAuthorityRenewalRetryTimer = 0;
    postBrowserAuthorityRenewalRequest(reason);
  }, delay);
  return true;
}

function settleBrowserAuthorityRenewal(ok, reason) {
  const request = browserAuthorityRenewal;
  if (!request) {
    return false;
  }
  clearBrowserAuthorityRenewalRequest();
  if (ok) {
    showStatus("Browser session refreshed. Reopening…", { sticky: true });
    return true;
  }
  scheduleBrowserAuthorityRenewalRetry(request.reason || reason);
  return true;
}

function isHomeBrowserAuthorityRenewalResult(event) {
  const message = event.data;
  const request = browserAuthorityRenewal;
  if (
    !request ||
    event.origin !== homeParentOrigin ||
    event.source !== window.top ||
    !hasExactMessageKeys(message, [
      "type",
      "requestId",
      "homeToken",
      "browserInstance",
      "ok",
      "freshHomeToken",
      "reason",
    ]) ||
    message.type !== BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE ||
    message.requestId !== request.requestId ||
    message.homeToken !== launchToken ||
    message.browserInstance !== browserInstanceId ||
    typeof message.ok !== "boolean" ||
    typeof message.freshHomeToken !== "string" ||
    message.freshHomeToken.length > 4_096 ||
    typeof message.reason !== "string" ||
    message.reason.length > 128
  ) {
    return false;
  }
  return message.ok
    ? Boolean(
        message.freshHomeToken &&
          message.freshHomeToken !== launchToken &&
          message.reason === "",
      )
    : message.freshHomeToken === "" && Boolean(message.reason);
}

function handleHomeBrowserAuthorityRenewalResult(event) {
  if (!isHomeBrowserAuthorityRenewalResult(event)) {
    return false;
  }
  return settleBrowserAuthorityRenewal(event.data.ok, event.data.reason);
}

function requestHomeRelaunch(reason) {
  if (
    relaunchRequested ||
    !window.parent ||
    window.parent === window ||
    !launchToken ||
    !browserInstanceId ||
    !homeParentOrigin
  ) {
    return false;
  }
  relaunchRequested = true;
  browserAuthorityRenewalAttempts = 0;
  showStatus(reason || "Browser session expired. Reopening from Home...", {
    sticky: true,
  });
  try {
    return postBrowserAuthorityRenewalRequest(
      reason || "browser_authority_expired",
    );
  } catch (_error) {
    clearBrowserAuthorityRenewalRequest();
    relaunchRequested = false;
    showStatus("Browser session could not be refreshed.", { sticky: true });
    return false;
  }
}

function requestFreshRuntimeAuthority(error) {
  if (!isAuthoritySessionError(error)) {
    return false;
  }
  stopPageStatusPolling();
  stopPageHeartbeat();
  requestHomeRelaunch(friendlyOpenError(error));
  return true;
}

function setLoading(loading) {
  document.body.dataset.loading = loading ? "true" : "false";
  addressInput.disabled = loading;
  if (engineSelect) {
    engineSelect.disabled = loading;
  }
  if (exitSelect) {
    exitSelect.disabled = loading;
  }
  if (profileResetButton) {
    profileResetButton.disabled = loading;
  }
  renderEmpty.hidden = true;
  refreshButton.disabled = loading || !currentPage;
  updateNavState();
}

function stopPageStatusPolling() {
  window.clearTimeout(pageStatusTimer);
  pageStatusTimer = 0;
  stopPageStatusRefresh();
}

function stopPageStatusRefresh() {
  for (const timer of pageStatusRefreshTimers) {
    window.clearTimeout(timer);
  }
  pageStatusRefreshTimers = [];
}

function stopPageHeartbeat() {
  window.clearTimeout(pageHeartbeatTimer);
  pageHeartbeatTimer = 0;
}

function recoverableRuntimePage(summary = browserSummary) {
  const recovery = summary?.sessions?.recoverable_page;
  if (
    recovery?.schema !== "elastos.browser.recoverable-page/v1" ||
    !["active", "cleanup_pending"].includes(recovery.state) ||
    typeof recovery.page_id !== "string" ||
    recovery.cleanup?.schema !== "elastos.browser.cleanup-handle/v1" ||
    typeof recovery.cleanup.id !== "string"
  ) {
    return null;
  }
  return {
    ...(recovery.engine_page && typeof recovery.engine_page === "object"
      ? recovery.engine_page
      : {}),
    page_id: recovery.page_id,
    runtime_cleanup: recovery.cleanup,
    recovery_state: recovery.state,
  };
}

function displayAttachRequestId(generation) {
  return typeof generation === "string" && generation.startsWith("display:")
    ? generation.slice("display:".length)
    : "";
}

function viewerDisplayAttachTimeoutMs() {
  const value = globalThis.VIEWER_DISPLAY_ATTACH_TIMEOUT_MS;
  return typeof value === "number" && value > 0 ? value : 8000;
}

function reuseDisplayAttachment(pending) {
  const validRequest = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
  const validGeneration = value => typeof value === "string" && /^display:[a-f0-9]{32}$/.test(value);
  if (
    !pending ||
    pending.schema !== "elastos.browser.display-attachment/v1" ||
    !validRequest(pending.request_id) ||
    !validGeneration(pending.previous_display_generation)
  ) {
    return null;
  }
  if (pending.state === "pending" || pending.state === "ready") {
    return pending;
  }
  if (pending.state === "failed" && pending.error_code === "display_attach_uncertain") {
    return pending;
  }
  return null;
}

function terminalDisplayAttachment(pending) {
  return pending &&
    pending.schema === "elastos.browser.display-attachment/v1" &&
    pending.state === "failed" &&
    pending.error_code === "display_attach_failed";
}

function allocateDisplayAttachRetryId(pageId, generation) {
  const validRequest = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
  const boot = typeof window !== "undefined" ? window.__elastosBrowserRestoreBoot : null;
  const shared = (boot && boot.retryIdentity) || displayAttachRetryIdentity;
  if (
    shared &&
    shared.page_id === pageId &&
    shared.generation === generation &&
    validRequest(shared.request_id)
  ) {
    displayAttachRetryIdentity = shared;
    if (boot) boot.retryIdentity = shared;
    return shared.request_id;
  }
  const requestId = crypto.randomUUID().replace(/-/g, "");
  if (shared && typeof shared === "object") {
    shared.page_id = pageId;
    shared.generation = generation;
    shared.request_id = requestId;
    displayAttachRetryIdentity = shared;
    if (boot) boot.retryIdentity = shared;
    return requestId;
  }
  const next = { page_id: pageId, generation, request_id: requestId };
  displayAttachRetryIdentity = next;
  if (boot) boot.retryIdentity = next;
  return next.request_id;
}

function markDisplayAttachRetryNeeded(pageId, generation) {
  const boot = typeof window !== "undefined" ? window.__elastosBrowserRestoreBoot : null;
  const shared = (boot && boot.retryIdentity) || displayAttachRetryIdentity || {
    page_id: "",
    generation: "",
    request_id: "",
  };
  shared.page_id = pageId;
  shared.generation = generation;
  shared.request_id = "";
  displayAttachRetryIdentity = shared;
  if (boot) boot.retryIdentity = shared;
}

function displayAttachRequestForRecovery(pageId, generation, pending) {
  const reused = reuseDisplayAttachment(pending);
  if (reused) {
    return {
      type: "display_attach",
      request_id: reused.request_id,
      display_generation: reused.previous_display_generation,
    };
  }
  const boot = typeof window !== "undefined" ? window.__elastosBrowserRestoreBoot : null;
  const shared = (boot && boot.retryIdentity) || displayAttachRetryIdentity;
  if (
    terminalDisplayAttachment(pending) ||
    (shared && shared.page_id === pageId && shared.generation === generation)
  ) {
    return {
      type: "display_attach",
      request_id: allocateDisplayAttachRetryId(pageId, generation),
      display_generation: generation,
    };
  }
  return {
    type: "display_attach",
    request_id: displayAttachRequestId(generation),
    display_generation: generation,
  };
}

function startRecoverableDisplayAttach() {
  if (
    recoverableDisplayAttachStarted ||
    homeWindowCloseInFlight ||
    homeWindowTerminalCloseConfirmed
  ) {
    return;
  }
  const pageId = currentPage?.page_id;
  const generation = currentPage?.display_session?.display_generation;
  const requestId = displayAttachRequestId(generation);
  if (
    typeof pageId !== "string" ||
    !pageId ||
    !launchToken ||
    !/^[a-f0-9]{32}$/.test(requestId) ||
    currentPage?.display_session?.mode !== PRODUCT_DISPLAY_MODE ||
    browserSummary?.engine_adapter?.display_attach_supported !== true
  ) {
    return;
  }
  recoverableDisplayAttachStarted = true;
  try {
    console.info(JSON.stringify({
      schema: "elastos.browser.media-diagnostic/v1",
      event: "dying_page_attach",
      request_id: requestId,
    }));
  } catch {
    // Keep unload attach on the product path if diagnostics cannot print.
  }
  fetch(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/webrtc`, {
    method: "POST",
    headers: {
      "x-elastos-home-token": launchToken,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      type: "display_attach",
      request_id: requestId,
      display_generation: generation,
    }),
    keepalive: true,
  }).catch(() => {});
}

function persistRecoverableDisplayQuery(page) {
  if (typeof location === "undefined" || typeof history === "undefined") {
    return;
  }
  const url = new URL(location.href);
  const generation = page?.display_session?.display_generation;
  const validGeneration = typeof generation === "string" && /^display:[a-f0-9]{32}$/.test(generation);
  if (typeof page?.page_id === "string" && page.page_id && validGeneration) {
    url.searchParams.set("page_id", page.page_id);
    url.searchParams.set("display_generation", generation);
  } else if (!page?.page_id) {
    url.searchParams.delete("page_id");
    url.searchParams.delete("display_generation");
  }
  const next = `${url.pathname}${url.search}${url.hash}`;
  const current = `${location.pathname}${location.search}${location.hash}`;
  if (next !== current) {
    history.replaceState(history.state, "", next);
  }
}

function publishRuntimePageForHost(page = currentPage) {
  window.__elastosBrowserCurrentPageId = page?.page_id || "";
  persistRecoverableDisplayQuery(page);
}

function currentRuntimePageOwner() {
  return runtimePageOwner(currentPage, currentPageGeneration);
}

function runtimeViewerOwnerActive(owner) {
  return !unloadCleanupStarted && !homeWindowCloseInFlight && !homeWindowTerminalCloseConfirmed &&
    sameRuntimePageOwner(currentRuntimePageOwner(), owner) &&
    !runtimePageCleanup.status(owner) &&
    !sameRuntimePageOwner(pendingHomeWindowCloseDelivery?.owner, owner);
}

function finalizeRuntimePageClose(owner) {
  if (sameRuntimePageOwner(currentRuntimePageOwner(), owner)) {
    currentPage = null;
    currentPageGeneration = 0;
    restoredViewerOwner = null;
    currentBrowserEngineId = "";
    currentRemoteExitId = "";
    publishRuntimePageForHost(null);
    stopPageStatusPolling();
    stopPageHeartbeat();
    closeRemoteDisplay();
    updateMetricsNode(null);
    updateNavState();
    runtimeOwnershipTerminallyAbsent = true;
    return true;
  }
  return false;
}

const runtimePageCleanup = createRuntimePageCleanupController({
  closePage: (owner, { signal }) =>
    fetchJson(
      `/api/apps/browser/pages/${encodeURIComponent(owner.page_id)}/close`,
      {
        method: "POST",
        signal,
        body: {
          schema: "elastos.browser.close-request/v2",
          cleanup_id: owner.runtime_cleanup.id,
          ...(browserInstanceId
            ? { browser_instance: browserInstanceId }
            : {}),
        },
      },
    ),
  onPending: (owner, _outcome, failure) => {
    if (failure && sameRuntimePageOwner(currentRuntimePageOwner(), owner)) {
      showStatus(
        `${runtimeOwnedFailureSummary(failure.kind)} Runtime cleanup is pending; the existing page remains owned and no replacement will open until Runtime confirms a terminal close.`,
        { sticky: true },
      );
    }
  },
  onTerminal: (owner, _outcome, failure) => {
    const applied = finalizeRuntimePageClose(owner);
    deliverPendingHomeBrowserWindowClose(owner, _outcome);
    if (
      applied &&
      (_outcome?.profile_durability === "failed" ||
        _outcome?.profile_durability === "unknown") &&
      !unloadCleanupStarted
    ) {
      showStatus(
        "Runtime closed the Browser session, but the profile save failed. You can open the address again.",
        { sticky: true },
      );
      return;
    }
    if (applied && failure && !unloadCleanupStarted) {
      showStatus(
        "Runtime confirmed the failed Browser session closed. You can open the address again or choose another Browser Engine.",
        { sticky: true },
      );
      return;
    }
    if (
      applied &&
      Number(runtimePageCleanup.status(owner)?.attempts || 0) > 1 &&
      !unloadCleanupStarted
    ) {
      showStatus(
        "Runtime confirmed the Browser session closed after cleanup reconciliation. Refresh Browser to open a new session.",
        { sticky: true },
      );
    }
  },
});

async function closeRuntimePage(
  page = currentPage,
  {
    generation =
      page === currentPage
        ? currentPageGeneration
        : Number.isSafeInteger(page?.generation)
          ? page.generation
          : 0,
    schedule = true,
    explicitRetry = false,
  } = {},
) {
  const owner = runtimePageOwner(page, generation);
  if (page?.page_id && !owner) {
    return {
      state: "pending",
      page_id: String(page.page_id),
      generation: Number(generation || 0),
      reason: "invalid_runtime_cleanup_handle",
    };
  }
  return runtimePageCleanup.reconcile(owner, {
    schedule,
    newWindow: explicitRetry,
  });
}

function runtimeOwnedFailureSummary(kind) {
  if (kind === "display_status") {
    return "Browser display status failed.";
  }
  if (kind === "malformed_response") {
    return "Browser received an invalid Runtime display response.";
  }
  if (kind === "timeout") {
    return "Browser display timed out.";
  }
  if (kind === "no_first_frame") {
    return "The Browser Engine started, but no video frame arrived.";
  }
  return "Browser display signaling failed.";
}

async function failRuntimeOwnedPage(kind, message) {
  const owner = currentRuntimePageOwner();
  if (!owner) {
    return {
      state: "terminal",
      page_id: "",
      generation: 0,
      terminal_kind: "no_page",
    };
  }
  stopPageStatusPolling();
  stopPageHeartbeat();
  closeRemoteDisplay();
  showStatus(
    `${runtimeOwnedFailureSummary(kind)} Runtime cleanup is pending; the existing page remains owned and no replacement will open until Runtime confirms a terminal close.`,
    { sticky: true },
  );
  return runtimePageCleanup.fail(owner, {
    kind,
    message,
    retry: false,
  });
}

function cleanupPendingError(outcome) {
  const error = new Error(
    "Browser cleanup is pending. Runtime has not confirmed a terminal close; the existing page is retained and no replacement will open.",
  );
  error.cleanupOutcome = outcome;
  return error;
}

function requireTerminalRuntimePageCloseOutcome(outcome) {
  if (outcome?.state !== "terminal") {
    throw cleanupPendingError(outcome);
  }
  return outcome;
}

function hasExactMessageKeys(value, expectedKeys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length &&
    actual.every((key, index) => key === expected[index]);
}

function isHomeBrowserWindowCloseRequest(event) {
  const message = event.data;
  return Boolean(
    event.origin === HOME_GUI_OPAQUE_ORIGIN &&
      event.source === window.parent &&
      hasExactMessageKeys(message, [
        "type",
        "requestId",
        "homeToken",
        "browserInstance",
      ]) &&
      message.type === BROWSER_WINDOW_CLOSE_REQUEST_TYPE &&
      typeof message.requestId === "string" &&
      message.requestId.length >= 1 &&
      message.requestId.length <= 128 &&
      message.homeToken === launchToken &&
      message.browserInstance === browserInstanceId &&
      Boolean(launchToken && browserInstanceId)
  );
}

function postHomeBrowserWindowCloseResult(message, owner, outcome, error = null) {
  const terminal = outcome?.state === "terminal";
  const state = error ? "error" : terminal ? "terminal" : "pending";
  window.parent.postMessage(
    {
      type: BROWSER_WINDOW_CLOSE_RESULT_TYPE,
      requestId: message.requestId,
      homeToken: launchToken,
      browserInstance: browserInstanceId,
      state,
      pageId: String(outcome?.page_id || owner?.page_id || ""),
      generation: Number(outcome?.generation ?? owner?.generation ?? 0),
      cleanupId: String(owner?.runtime_cleanup?.id || ""),
      terminalKind: terminal ? String(outcome.terminal_kind || "") : "",
      reason: terminal
        ? ""
        : String(error ? "close_error" : outcome?.reason || "cleanup_pending"),
    },
    "*",
  );
}

function deliverPendingHomeBrowserWindowClose(owner, outcome) {
  const delivery = pendingHomeWindowCloseDelivery;
  const currentOwner = currentRuntimePageOwner();
  if (
    !delivery ||
    outcome?.state !== "terminal" ||
    !["closed", "already_absent"].includes(outcome.terminal_kind) ||
    outcome.page_id !== owner?.page_id ||
    Number(outcome.generation) !== Number(owner?.generation) ||
    !sameRuntimePageOwner(delivery.owner, owner) ||
    (currentOwner && !sameRuntimePageOwner(currentOwner, owner))
  ) {
    return false;
  }
  pendingHomeWindowCloseDelivery = null;
  homeWindowTerminalCloseConfirmed = true;
  postHomeBrowserWindowCloseResult(delivery.message, owner, outcome);
  return true;
}

async function handleHomeBrowserWindowCloseRequest(event) {
  if (!isHomeBrowserWindowCloseRequest(event)) {
    return false;
  }
  const message = event.data;
  let page = currentPage || recoverableRuntimePage() || unsettledRuntimeOpen?.page;
  let generation = page === currentPage ? currentPageGeneration : 0;
  let owner = runtimePageOwner(page, generation);
  if (
    runtimeOpenInFlight > 0 ||
    browserSummaryPromise ||
    homeWindowCloseInFlight
  ) {
    showStatus(
      "Browser ownership is changing. Close Browser again when the current action finishes.",
      { sticky: true },
    );
    postHomeBrowserWindowCloseResult(message, owner, {
      state: "pending",
      page_id: owner?.page_id || "",
      generation: owner?.generation || 0,
      reason:
        runtimeOpenInFlight > 0 || browserSummaryPromise
          ? "runtime_open_in_flight"
          : "window_close_in_flight",
    });
    return true;
  }
  homeWindowCloseInFlight = true;
  try {
    if (!owner) {
      const ownership = await resolveRuntimeOwnershipForWindowClose();
      if (ownership.state !== "owned") {
        if (ownership.state === "terminal") {
          pendingHomeWindowCloseDelivery = null;
          unsettledRuntimeOpen = null;
          runtimeOwnershipTerminallyAbsent = true;
          homeWindowTerminalCloseConfirmed = true;
        } else {
          showStatus(
            "Runtime ownership is not settled. Close Browser again to retry.",
            { sticky: true },
          );
        }
        postHomeBrowserWindowCloseResult(message, null, ownership);
        return true;
      }
      page = ownership.page;
      generation = 0;
      owner = runtimePageOwner(page, generation);
      if (!owner) {
        postHomeBrowserWindowCloseResult(message, null, {
          state: "pending",
          page_id: String(page?.page_id || ""),
          generation: 0,
          reason: "invalid_runtime_cleanup_handle",
        });
        return true;
      }
    }
    if (owner) {
      postHomeBrowserWindowCloseResult(message, owner, {
        state: "pending",
        page_id: owner.page_id,
        generation: owner.generation,
        reason: "cleanup_in_flight",
      });
    }
    const outcome = owner
      ? await closeRuntimePage(owner, { explicitRetry: true })
      : await closeRuntimePage(page, { generation, explicitRetry: true });
    if (outcome?.state === "terminal") {
      if (
        pendingHomeWindowCloseDelivery &&
        sameRuntimePageOwner(pendingHomeWindowCloseDelivery.owner, owner)
      ) {
        pendingHomeWindowCloseDelivery = null;
      }
      unsettledRuntimeOpen = null;
      runtimeOwnershipTerminallyAbsent = true;
      homeWindowTerminalCloseConfirmed = true;
    } else {
      pendingHomeWindowCloseDelivery = Object.freeze({
        message: Object.freeze({ ...message }),
        owner,
      });
      showStatus(
        "Runtime cleanup is pending. Close Browser again to retry.",
        { sticky: true },
      );
    }
    postHomeBrowserWindowCloseResult(message, owner, outcome);
  } catch (_error) {
    if (
      pendingHomeWindowCloseDelivery &&
      sameRuntimePageOwner(pendingHomeWindowCloseDelivery.owner, owner)
    ) {
      pendingHomeWindowCloseDelivery = null;
    }
    showStatus(
      "Runtime cleanup could not be confirmed. Close Browser again to retry.",
      { sticky: true },
    );
    postHomeBrowserWindowCloseResult(message, owner, null, _error);
  } finally {
    homeWindowCloseInFlight = false;
  }
  return true;
}

function runtimeOpenResultPage(response) {
  const page = response?.engine_page;
  if (
    response?.schema !== "elastos.browser.open-result/v1" ||
    page?.schema !== "elastos.browser.engine.page/v1" ||
    response?.runtime_cleanup?.schema !== "elastos.browser.cleanup-handle/v1"
  ) {
    return null;
  }
  const openedPage = {
    ...page,
    runtime_cleanup: response.runtime_cleanup,
  };
  return runtimePageOwner(openedPage, 0) ? openedPage : null;
}

function pendingWindowCloseOwnership(reason) {
  return {
    state: "pending",
    page_id: "",
    generation: 0,
    reason,
  };
}

function terminalRuntimeOpenOutcome(value) {
  const outcome = value?.outcome;
  return Boolean(
    outcome?.schema === "elastos.browser.open-outcome/v1" &&
      [
        "terminal_pre_effect_failure",
        "terminal_post_effect_cleanup",
      ].includes(outcome.state)
  );
}

function terminalWindowCloseAbsence() {
  return {
    state: "terminal",
    page_id: "",
    generation: 0,
    terminal_kind: "no_page",
  };
}

function proveRuntimeOwnershipAbsentBeforeDispatch() {
  if (
    currentPage?.page_id ||
    recoverableRuntimePage()?.page_id ||
    unsettledRuntimeOpen
  ) {
    return false;
  }
  runtimeOwnershipTerminallyAbsent = true;
  return true;
}

function normalizeRuntimeOpenUrl(value) {
  try {
    return normalizeUrl(value);
  } catch (error) {
    proveRuntimeOwnershipAbsentBeforeDispatch();
    throw error;
  }
}

async function settleUnresolvedRuntimeOpenForWindowClose(settlement) {
  let response;
  try {
    if (settlement.statusUrl) {
      response = await fetchJson(settlement.statusUrl, { method: "GET" });
    } else {
      response = await fetchJson("/api/apps/browser/open", {
        method: "POST",
        body: settlement.body,
      });
    }
  } catch (error) {
    if (terminalRuntimeOpenOutcome(error?.payload)) {
      return terminalWindowCloseAbsence();
    }
    return pendingWindowCloseOwnership("runtime_open_status_unavailable");
  }
  if (response?.schema === "elastos.browser.open-accepted/v1") {
    if (typeof response.status_url !== "string" || !response.status_url) {
      return pendingWindowCloseOwnership("runtime_open_status_invalid");
    }
    settlement.statusUrl = response.status_url;
    return pendingWindowCloseOwnership("runtime_open_status_pending");
  }
  if (response?.schema === "elastos.browser.open-status/v1") {
    if (response.status === "pending") {
      return pendingWindowCloseOwnership("runtime_open_status_pending");
    }
    if (response.status === "failed") {
      return terminalRuntimeOpenOutcome(response.error)
        ? terminalWindowCloseAbsence()
        : { state: "probe" };
    }
    if (response.status !== "completed") {
      return pendingWindowCloseOwnership("runtime_open_status_invalid");
    }
    response = response.result;
  }
  const page = runtimeOpenResultPage(response);
  if (!page) {
    return pendingWindowCloseOwnership("runtime_open_result_invalid");
  }
  settlement.page = page;
  return { state: "owned", page };
}

function classifyFreshWindowCloseSummary(summary) {
  const sessions = summary?.sessions;
  if (
    summary?.schema !== "elastos.browser.runtime/v1" ||
    sessions?.schema !== "elastos.browser.session-capacity/v1" ||
    sessions.status !== "configured"
  ) {
    return pendingWindowCloseOwnership("runtime_ownership_probe_invalid");
  }
  const page = recoverableRuntimePage(summary);
  if (page) {
    return { state: "owned", page };
  }
  const closeOwnership = sessions.window_close_ownership;
  if (
    sessions.recoverable_page === null &&
    closeOwnership?.schema === "elastos.browser.window-close-ownership/v1" &&
    closeOwnership.browser_instance === browserInstanceId &&
    closeOwnership.state === "absent"
  ) {
    return terminalWindowCloseAbsence();
  }
  return pendingWindowCloseOwnership("runtime_ownership_unproven");
}

async function resolveRuntimeOwnershipForWindowClose() {
  const settlement = unsettledRuntimeOpen;
  if (settlement?.page) {
    return { state: "owned", page: settlement.page };
  }
  if (settlement) {
    const outcome = await settleUnresolvedRuntimeOpenForWindowClose(settlement);
    if (outcome.state !== "probe") {
      return outcome;
    }
  }
  if (runtimeOwnershipTerminallyAbsent) {
    return terminalWindowCloseAbsence();
  }
  if (!browserInstanceId) {
    return pendingWindowCloseOwnership("runtime_ownership_unbound");
  }
  let summary;
  try {
    summary = await fetchJson(
      `/api/apps/browser/summary?browser_instance=${encodeURIComponent(browserInstanceId)}`,
      { method: "GET" },
    );
  } catch (_error) {
    return pendingWindowCloseOwnership("runtime_ownership_probe_failed");
  }
  browserSummary = summary;
  return classifyFreshWindowCloseSummary(summary);
}

function settleInitialRuntimeOpenPostFailure(settlement, error) {
  if (
    unsettledRuntimeOpen !== settlement ||
    (!isAuthoritySessionError(error) &&
      !terminalRuntimeOpenOutcome(error?.payload))
  ) {
    return false;
  }
  unsettledRuntimeOpen = null;
  runtimeOwnershipTerminallyAbsent = true;
  return true;
}

function currentBrowserUrl() {
  return (
    currentPage?.actual_url ||
    currentPage?.url ||
    getCurrentUrl() ||
    lastRequestedUrl ||
    DEFAULT_URL
  );
}

function settleRemoteDisplayFailure(
  message,
  { failureKind = "signaling" } = {},
) {
  if (unloadCleanupStarted || relaunchRequested) {
    return Promise.resolve();
  }
  if (sameRuntimePageOwner(currentRuntimePageOwner(), restoredViewerOwner)) {
    closeRemoteDisplay();
    showStatus("The Browser display could not reconnect. Reload Browser to retry.", { sticky: true });
    return Promise.resolve();
  }
  return failRuntimeOwnedPage(failureKind, message);
}

function recoverMissingRuntimePage(error, message) {
  if (!isMissingRuntimePageError(error)) {
    return false;
  }
  settleRemoteDisplayFailure(message, { failureKind: "display_status" });
  return true;
}

function releaseRuntimePageForUnload() {
  if (unloadCleanupStarted) {
    return;
  }
  unloadCleanupStarted = true;
  homeClipboard.teardown();
  stopPageStatusPolling();
  stopPageHeartbeat();
  clearBrowserAuthorityRenewalRequest();
  window.clearTimeout(browserAuthorityRenewalRetryTimer);
  browserAuthorityRenewalRetryTimer = 0;
}

function updateMetricsNode(status) {
  if (!metricsNode) {
    return;
  }
  const remoteMetrics = remoteDisplay?.metricsState() || {};
  window.__elastosBrowserRemoteDisplayMetrics = {
    current_page_id: currentPage?.page_id || "",
    display_mode: currentDisplayMode || "",
    display_input: currentDisplayInput || "",
    status_audio: status?.audio ?? null,
    status_video: status?.video ?? null,
    status_webrtc_connection_state: status?.webrtc_connection_state || "",
    status_ice_connection_state: status?.ice_connection_state || "",
    latestWebrtcStats: remoteMetrics.latestWebrtcStats || null,
    latestVideoWebrtcStats: remoteMetrics.latestVideoWebrtcStats || null,
    latestAudioWebrtcStats: remoteMetrics.latestAudioWebrtcStats || null,
    remoteAudioExpected: remoteMetrics.remoteAudioExpected === true,
    remoteAudioUnlocked: remoteMetrics.remoteAudioUnlocked === true,
    remoteAudioMuted: remoteMetrics.remoteAudioMuted === true,
    remoteAudioPaused: remoteMetrics.remoteAudioPaused === true,
    remoteAudioTrackCount: Number(remoteMetrics.remoteAudioTrackCount || 0),
    remoteAudioConnectionState: remoteMetrics.remoteAudioConnectionState || "",
    remoteAudioIceConnectionState: remoteMetrics.remoteAudioIceConnectionState || "",
    remoteAudioSignalingState: remoteMetrics.remoteAudioSignalingState || "",
    remoteAudioIceGatheringState: remoteMetrics.remoteAudioIceGatheringState || "",
    audioBrowserCandidateCount: Number(remoteMetrics.audioBrowserCandidateCount || 0),
    audioEngineCandidateCount: Number(remoteMetrics.audioEngineCandidateCount || 0),
    audioRawCandidateEventCount: Number(remoteMetrics.audioRawCandidateEventCount || 0),
    audioNullCandidateEventCount: Number(remoteMetrics.audioNullCandidateEventCount || 0),
    lastAudioBrowserCandidateSummary: remoteMetrics.lastAudioBrowserCandidateSummary || "",
    lastAudioEngineCandidateSummary: remoteMetrics.lastAudioEngineCandidateSummary || "",
    audioOfferSummary: remoteMetrics.audioOfferSummary || null,
    audioAnswerSummary: remoteMetrics.audioAnswerSummary || null,
    remoteVideoMuted: remoteMetrics.remoteVideoMuted === true,
    remoteVideoPaused: remoteMetrics.remoteVideoPaused === true,
  };
  if (!debugMetrics || !status) {
    metricsNode.hidden = true;
    return;
  }
  metricsNode.textContent = browserMetricsText(status, {
    ...remoteMetrics,
    remoteVideo,
  });
  metricsNode.hidden = false;
}

async function fetchPageStatus({
  history = "replace",
  forceAddress = false,
  fast = false,
} = {}) {
  if (!currentPage?.page_id) {
    return null;
  }
  const owner = currentRuntimePageOwner();
  const query = fast ? "?fast=1" : "";
  const status = await fetchJson(
    `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/status${query}`,
    { method: "GET" },
  );
  if (!runtimeViewerOwnerActive(owner)) return null;
  if (
    status?.schema !== "elastos.browser.page-status/v1" ||
    status.page_id !== currentPage.page_id
  ) {
    const error = new Error("Browser could not read the page status.");
    error.runtimeOwnedFailureKind = "malformed_response";
    throw error;
  }
  if (status.direct_network !== false) {
    const error = new Error("Browser reported an unsafe network setup.");
    error.runtimeOwnedFailureKind = "malformed_response";
    throw error;
  }
  lastPageStatus = status;
  syncViewFromResponse(status);
  handleFileChooserFromStatus(status);
  if (status.actual_url) {
    currentPage = {
      ...currentPage,
      actual_url: status.actual_url,
      title: status.title || currentPage?.title,
    };
    syncBrowserLocation(status.actual_url, status.title || "", history, {
      forceAddress,
    });
  }
  updateMetricsNode(status);
  return status;
}

function handleFileChooserFromStatus(status) {
  const chooser = status?.file_chooser;
  if (chooser?.pending !== true || !chooser.request_id || !currentPage?.page_id) {
    return;
  }
  if (chooser.request_id === lastLibraryFilePickerRequestId && libraryPickerRequest?.pageId === currentPage.page_id) {
    return;
  }
  lastLibraryFilePickerRequestId = chooser.request_id;
  libraryPickerRequest = { requestId: chooser.request_id, pageId: currentPage?.page_id, consumed: false };
  showStatus("Choose a Library item for Browser.");
  openLibraryFilePicker();
}

function openLibraryFilePicker() {
  if (!window.parent || window.parent === window) {
    showStatus("Open Library from Home to choose a Browser file.", {
      sticky: true,
    });
    return false;
  }
  window.top.postMessage(
    {
      type: "home:open-target",
      homeToken: launchToken,
      target: "library",
      requestId: libraryPickerRequest?.requestId,
      documentNonce: libraryPickerDocumentNonce,
      query: {
        mode: "attach",
        returnTarget: "browser",
      },
    },
    homeParentOrigin,
  );
  return true;
}

function fileNameFromLibraryPayload(payload) {
  const name = String(payload?.fileName || payload?.title || "Library item")
    .replace(/[\0\r\n]/g, "")
    .split(/[\\/]/)
    .pop()
    .trim();
  return name || "Library item";
}

async function base64FromBlob(blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

async function handleLibraryFilePickerSelection(payload) {
  const request = libraryPickerRequest;
  if (!request || request.consumed || payload?.requestId !== request.requestId ||
      payload?.documentNonce !== libraryPickerDocumentNonce ||
      !payload?.pickerId || !payload?.deliveryId || currentPage?.page_id !== request.pageId) return false;
  request.consumed = true;
  if (!payload?.blob || typeof payload.blob.arrayBuffer !== "function") {
    showStatus("Library did not return file bytes for Browser.", { sticky: true });
    return false;
  }
  if (payload.blob.size > LIBRARY_FILE_PICKER_MAX_BYTES) {
    showStatus("Browser file picker accepts Library items up to 16 MiB.", {
      sticky: true,
    });
    return false;
  }
  const fileName = fileNameFromLibraryPayload(payload);
  const contentBase64 = await base64FromBlob(payload.blob);
  if (libraryPickerRequest !== request || currentPage?.page_id !== request.pageId) return false;
  showStatus(`Inserting ${fileName}...`);
  const response = await sendBrowserInput(
    {
      type: "file_upload",
      request_id: request.requestId,
      file_name: fileName,
      mime_type:
        typeof payload.mimeType === "string" && payload.mimeType
          ? payload.mimeType
          : payload.blob.type || "application/octet-stream",
      content_base64: contentBase64,
      object_uri: typeof payload.objectUri === "string" ? payload.objectUri : "",
    },
    { history: "replace" },
  );
  if (libraryPickerRequest !== request || currentPage?.page_id !== request.pageId ||
      response?.accepted !== true || response?.file_upload?.request_id !== request.requestId) return false;
  lastLibraryFilePickerRequestId = "";
  showStatus(`Inserted ${fileName}.`);
  return true;
}

async function handlePageObservationFailure(error, owner) {
  if (unloadCleanupStarted || !sameRuntimePageOwner(currentRuntimePageOwner(), owner)) return false;
  if (requestFreshRuntimeAuthority(error)) return false;
  if (error.runtimeTransportFailure === true) {
    showStatus("Connection interrupted. Browser is reconnecting.");
    return true;
  }
  await failRuntimeOwnedPage(
    error.runtimeOwnedFailureKind || "display_status",
    friendlyOpenError(error),
  );
  return false;
}

function schedulePageStatusRefresh({
  history = "replace",
  delay = PAGE_STATUS_AFTER_INPUT_DELAY_MS,
  forceAddress = false,
} = {}) {
  const delays = [
    delay,
    ...PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS.filter(
      (value) => value > delay,
    ),
  ];
  stopPageStatusRefresh();
  pageStatusRefreshTimers = delays.map((nextDelay) => {
    const timer = window.setTimeout(async () => {
      pageStatusRefreshTimers = pageStatusRefreshTimers.filter(
        (candidate) => candidate !== timer,
      );
      if (
        unloadCleanupStarted ||
        !currentPage ||
        document.hidden ||
        (!forceAddress && isAddressEditing())
      ) {
        return;
      }
      const owner = currentRuntimePageOwner();
      try {
        await fetchPageStatus({ history, forceAddress });
      } catch (error) {
        await handlePageObservationFailure(error, owner);
      }
    }, nextDelay);
    return timer;
  });
}

function startPageStatusPolling() {
  stopPageStatusPolling();
  const poll = async () => {
    if (unloadCleanupStarted || !currentPage) {
      return;
    }
    if (document.hidden) {
      pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_INTERVAL_MS);
      return;
    }
    if (isAddressEditing()) {
      pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_INTERVAL_MS);
      return;
    }
    const owner = currentRuntimePageOwner();
    try {
      await fetchPageStatus({ fast: true });
    } catch (error) {
      await handlePageObservationFailure(error, owner);
    } finally {
      if (
        sameRuntimePageOwner(currentRuntimePageOwner(), owner) &&
        !unloadCleanupStarted &&
        !relaunchRequested &&
        !runtimePageCleanup.status(currentRuntimePageOwner())?.failure
      ) {
        pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_INTERVAL_MS);
      }
    }
  };
  pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_FIRST_POLL_MS);
}

function startPageHeartbeat() {
  stopPageHeartbeat();
  const beat = async () => {
    if (unloadCleanupStarted || !currentPage?.page_id) {
      return;
    }
    const owner = currentRuntimePageOwner();
    let nextDelay = PAGE_HEARTBEAT_INTERVAL_MS;
    try {
      await fetchJson(
        `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/heartbeat`,
        { method: "POST" },
      );
    } catch (error) {
      if (await handlePageObservationFailure(error, owner)) nextDelay = PAGE_STATUS_INTERVAL_MS;
    } finally {
      if (
        sameRuntimePageOwner(currentRuntimePageOwner(), owner) &&
        !unloadCleanupStarted &&
        !relaunchRequested &&
        !runtimePageCleanup.status(currentRuntimePageOwner())?.failure
      ) {
        pageHeartbeatTimer = window.setTimeout(beat, nextDelay);
      }
    }
  };
  pageHeartbeatTimer = window.setTimeout(beat, PAGE_HEARTBEAT_INTERVAL_MS);
}

function updateNavState() {
  backButton.disabled =
    document.body.dataset.loading === "true" || !currentPage || !canGoBack;
  forwardButton.disabled =
    document.body.dataset.loading === "true" || !currentPage || !canGoForward;
  refreshButton.disabled =
    document.body.dataset.loading === "true" || !currentPage;
}

function syncViewFromResponse(response, { dimensions = true } = {}) {
  if (typeof response?.can_go_back === "boolean") {
    canGoBack = response.can_go_back;
  }
  if (typeof response?.can_go_forward === "boolean") {
    canGoForward = response.can_go_forward;
  }
  if (
    dimensions &&
    Number(response?.width) &&
    Number(response?.height)
  ) {
    currentView = {
      ...(currentView || {}),
      width: Number(response.width),
      height: Number(response.height),
    };
  }
  updateNavState();
}

function syncDisplayInputFromSession(displaySession) {
  currentDisplayInput =
    displaySession?.input === "datachannel" ? "datachannel" : "runtime_route";
  currentDisplayInputProtocol =
    displaySession?.input_protocol === "selkies_v1"
      ? "selkies_v1"
      : "elastos_json";
}

function currentInputTransport() {
  return currentPage?.display_session?.input === "datachannel"
    ? "datachannel"
    : currentDisplayInput;
}

function currentInputProtocol() {
  return currentPage?.display_session?.input_protocol === "selkies_v1"
    ? "selkies_v1"
    : currentDisplayInputProtocol;
}

function viewFromDisplaySession(displaySession) {
  if (
    displaySession?.schema !== "elastos.browser.display-session/v1" ||
    !Number(displaySession.width) ||
    !Number(displaySession.height)
  ) {
    return null;
  }
  return {
    schema: "elastos.browser.view/v1",
    mode: displaySession.mode || "webrtc_remote_display",
    width: Number(displaySession.width),
    height: Number(displaySession.height),
  };
}

function encodeDatachannelInput(event) {
  if (currentInputProtocol() === "selkies_v1") {
    return selkiesMessagesForInput(event, currentView);
  }
  return [
    JSON.stringify({
      schema: "elastos.browser.input-event/v1",
      page_id: currentPage.page_id,
      event,
    }),
  ];
}

async function sendBrowserInput(
  event,
  { focus = true, history = "push" } = {},
) {
  const owner = currentRuntimePageOwner();
  if (!owner) {
    return;
  }
  if (!browserInputQueue || !sameRuntimePageOwner(browserInputQueue.owner, owner)) {
    browserInputQueue = { owner, tail: Promise.resolve(), pending: 0, failed: false };
  }
  const queue = browserInputQueue;
  if (queue.pending >= MAX_PENDING_BROWSER_INPUTS) {
    throw new Error("Browser input is busy. Check the page before typing again.");
  }
  queue.pending += 1;
  // Runtime text insertion must settle before a later key or pointer action.
  const pending = queue.tail.then(() => {
    if (!sameRuntimePageOwner(currentRuntimePageOwner(), owner)) {
      return;
    }
    if (queue.failed) {
      throw new Error("Browser input was canceled after a failed operation.");
    }
    return dispatchBrowserInput(event, { focus, history }, owner);
  });
  queue.tail = pending.catch(() => {
    // Delivery can be uncertain. Abandon dependent input without replaying it.
    queue.failed = true;
    if (browserInputQueue === queue) {
      browserInputQueue = null;
    }
  });
  try {
    return await pending;
  } finally {
    queue.pending -= 1;
  }
}

async function dispatchBrowserInput(
  event,
  { focus = true, history = "push" } = {},
  inputOwner = currentRuntimePageOwner(),
) {
  if (!sameRuntimePageOwner(currentRuntimePageOwner(), inputOwner)) {
    return;
  }
  const requiresRuntimeRoute =
    event?.type === "browser_command" ||
    // A following text insertion needs the Engine's completed click/focus,
    // rather than only a successful send on a different transport.
    event?.type === "click" ||
    event?.type === "paste_text" ||
    event?.type === "file_upload" ||
    event?.type === "clipboard_write";
  if (
    currentDisplayMode === "webrtc_remote_display" &&
    currentInputTransport() === "datachannel" &&
    !requiresRuntimeRoute
  ) {
    if (!remoteDisplay?.inputChannelOpen()) {
      throw new Error("Browser remote-display input channel is not open.");
    }
    remoteDisplay.sendInputMessages(encodeDatachannelInput(event));
    schedulePageStatusRefresh({
      history,
      forceAddress: true,
      delay:
        event?.type === "wheel"
          ? PAGE_STATUS_AFTER_SCROLL_DELAY_MS
          : PAGE_STATUS_AFTER_INPUT_DELAY_MS,
    });
    if (focus) {
      focusRemoteInput();
    }
  } else {
    const inputPageId = inputOwner.page_id;
    let response;
    try {
      response = await fetchJson(
        `/api/apps/browser/pages/${encodeURIComponent(inputPageId)}/input`,
        {
          method: "POST",
          body: { event },
        },
      );
    } catch (error) {
      if (!sameRuntimePageOwner(currentRuntimePageOwner(), inputOwner)) {
        return;
      }
      recoverMissingRuntimePage(error, "Browser session was released.");
      throw error;
    }
    if (!sameRuntimePageOwner(currentRuntimePageOwner(), inputOwner)) {
      return;
    }
    if (event?.type === "file_upload" &&
        (currentPage?.page_id !== inputPageId || libraryPickerRequest?.requestId !== event.request_id)) {
      throw new Error("The Browser file request changed. Check the page before trying again.");
    }
    handleFileChooserFromStatus(response);
    syncViewFromResponse(response);
    if (response.actual_url) {
      currentPage = {
        ...currentPage,
        actual_url: response.actual_url,
        title: response.title || currentPage?.title,
      };
      syncBrowserLocation(response.actual_url, response.title, history);
    }
    if (focus) {
      focusRemoteInput();
    }
    if (response?.accepted !== true && !response?.actual_url && !response?.file_chooser) {
      throw new Error("Browser could not send that input.");
    }
    return response;
  }
}

const {
  copyRemoteClipboardToHost,
  handleRemoteInputChannelMessage,
  pasteHostClipboardIntoRemote,
  teardownRemoteClipboard,
} = createBrowserClipboardBridge({
  cancelHostClipboardRequestFn: homeClipboard.cancel,
  createClipboardRequestIdFn: homeClipboard.newRequestId,
  friendlyOpenError,
  getCurrentPage: () => currentPage,
  sendBrowserInput,
  showStatus,
  writeHostClipboardTextFn: homeClipboard.writeText,
});

window.addEventListener("message", (event) => {
  if (isHomeBrowserWindowCloseRequest(event)) {
    void handleHomeBrowserWindowCloseRequest(event);
    return;
  }
  if (handleHomeBrowserAuthorityRenewalResult(event)) {
    return;
  }
  if (event.origin !== homeParentOrigin || event.source !== window.top) {
    return;
  }
  const payload = event.data && typeof event.data === "object" ? event.data : null;
  if (payload?.type !== "browser:file-picker-selection") {
    return;
  }
  handleLibraryFilePickerSelection(payload).catch((error) => {
    showStatus(friendlyOpenError(error), { sticky: true });
    return false;
  }).then((accepted) => {
    window.top.postMessage({
      type: "home:picker-accepted", homeToken: launchToken,
      pickerId: payload.pickerId, requestId: payload.requestId,
      documentNonce: payload.documentNonce, deliveryId: payload.deliveryId, accepted,
    }, homeParentOrigin);
  });
});

remoteDisplay = createBrowserRemoteDisplay({
  debugMetrics,
  fetchJson,
  friendlyOpenError,
  getCurrentDisplayMode: () => currentDisplayMode,
  getLastPageStatus: () => lastPageStatus,
  supportsDisplayGeneration: () => browserSummary?.engine_adapter?.display_attach_supported === true,
  captureViewerOwnerGuard: () => {
    const owner = currentRuntimePageOwner();
    return () => runtimeViewerOwnerActive(owner);
  },
  handleRemoteInputChannelMessage,
  handleRemoteInputChannelTeardown: teardownRemoteClipboard,
  onRecoveryRequired: settleRemoteDisplayFailure,
  remoteVideo,
  renderEmpty,
  renderPanel,
  resetPageStatus: () => {
    lastPageStatus = null;
  },
  setActiveBrowserPage: () => {
    document.body.dataset.browserPage = "active";
  },
  setDisplayInput: (input, protocol) => {
    currentDisplayInput = input;
    currentDisplayInputProtocol = protocol;
  },
  showStatus,
  updateMetrics: updateMetricsNode,
});

// A requested diagnostic sample reads the current viewer peers; the HUD keeps its normal cadence.
async function readRemoteDisplayMetrics() {
  const owner = currentRuntimePageOwner();
  if (!runtimeViewerOwnerActive(owner)) return null;
  const metrics = await remoteDisplay.refreshMetrics();
  if (!metrics || !runtimeViewerOwnerActive(owner)) return null;
  updateMetricsNode(lastPageStatus || {});
  return window.__elastosBrowserRemoteDisplayMetrics || null;
}

window.__elastosBrowserReadRemoteDisplayMetrics = readRemoteDisplayMetrics;

function closeRemoteDisplay() {
  remoteDisplay?.close();
}

async function connectRemoteDisplay(displaySession, enginePage = currentPage) {
  await remoteDisplay.connect(displaySession, enginePage);
}

function unlockRemoteAudioFromGesture() {
  remoteDisplay?.unlockAudioFromGesture();
}

function visibleRemoteCarrierExits(summary) {
  const exits = summary?.net?.exit_provider?.remote_carrier_exits;
  return Array.isArray(exits)
    ? exits.filter((exit) =>
        typeof exit?.id === "string" &&
        exit.id &&
        exit.allowed_for_principal !== false &&
        exit.state !== "expired")
    : [];
}

function visibleBrowserEngines(summary) {
  const engines = summary?.engine_adapter?.adapters;
  return Array.isArray(engines)
    ? engines.filter((engine) =>
        typeof engine?.id === "string" &&
        engine.id &&
        engine.direct_network === false &&
        engine.wallet_injection === false)
    : [];
}

function selectedBrowserEngine(summary = browserSummary) {
  return visibleBrowserEngines(summary).find((engine) => engine.id === selectedBrowserEngineId) || null;
}

function browserEngineKindLabel(engine) {
  const substrate = String(engine?.backing_substrate || "").toLowerCase();
  if (substrate === "remote_operator_vm") {
    return "Mac Browser Engine";
  }
  if (substrate === "local_microvm") {
    const marker = `${engine?.id || ""} ${engine?.engine || ""}`.toLowerCase();
    return marker.includes("jetson") || marker.includes("crosvm")
      ? "Jetson Browser Engine"
      : "MicroVM Browser Engine";
  }
  if (substrate === "host_policy_webview") {
    return "Device Browser Engine";
  }
  if (substrate === "operator_rbi") {
    return "Remote Browser Engine";
  }
  const marker = `${engine?.id || ""} ${engine?.engine || ""}`.toLowerCase();
  if (marker.includes("jetson") || marker.includes("crosvm")) {
    return "Jetson Browser Engine";
  }
  if (marker.includes("mac") || marker.includes("vz") || marker.includes("darwin")) {
    return "Mac Browser Engine";
  }
  if (marker.includes("microvm") || marker.includes("chromium")) {
    return "MicroVM Browser Engine";
  }
  if (marker.includes("hosted") || marker.includes("remote")) {
    return "Remote Browser Engine";
  }
  return "Browser Engine";
}

function browserEngineLabel(adapterId = selectedBrowserEngineId) {
  if (!adapterId) {
    return DEFAULT_ENGINE_LABEL;
  }
  const engine = visibleBrowserEngines(browserSummary).find(
    (candidate) => candidate.id === adapterId,
  );
  return engine ? `${browserEngineKindLabel(engine)}: ${engine.id}` : `Browser Engine: ${adapterId}`;
}

function browserEngineSummary(engine) {
  if (!engine) {
    return DEFAULT_ENGINE_SUMMARY;
  }
  return `${browserEngineLabel(engine.id)}. Runs the page in an isolated Browser Engine; network access uses the selected Exit Node.`;
}

function updateSettingsTitle() {
  if (!settingsButton) {
    return;
  }
  const engineText = engineSummaryNode?.textContent || DEFAULT_ENGINE_SUMMARY;
  const exitText = exitSummaryNode?.textContent || LOCAL_EXIT_SUMMARY;
  settingsButton.title = `Browser settings: ${engineText} ${exitText}`;
}

function remoteCarrierExitLabel(exit) {
  const id = String(exit?.id || "").trim();
  const grant = String(exit?.grant_id || "").trim();
  const displayName = String(exit?.display_name || exit?.label || "").trim();
  const label = displayName || id || grant || "shared Exit Node";
  const marker = `${id} ${grant}`.toLowerCase();
  const kind = marker.includes("seed") ? "Seed Exit Node" : "Shared Exit Node";
  return `${kind}: ${label}`;
}

function remoteCarrierExitSummary(exit) {
  const label = remoteCarrierExitLabel(exit);
  const remaining = Number(exit?.accounting?.active_streams_remaining);
  const quota =
    Number.isFinite(remaining) && remaining >= 0
      ? ` ${remaining} stream${remaining === 1 ? "" : "s"} available.`
      : "";
  return `${label}.${quota}`;
}

function browserExitLabel(remoteExitId = selectedRemoteExitId) {
  if (!remoteExitId) {
    return LOCAL_EXIT_LABEL;
  }
  const exit = visibleRemoteCarrierExits(browserSummary).find(
    (candidate) => candidate.id === remoteExitId,
  );
  return exit ? remoteCarrierExitLabel(exit) : "Shared Exit Node";
}

function syncExitSelect(summary) {
  if (!exitSelect) {
    return;
  }
  const exits = visibleRemoteCarrierExits(summary);
  const selectedExit = renderServiceSelection(exitSelect, {
    services: exits,
    selectedId: selectedRemoteExitId,
    defaultLabel: LOCAL_EXIT_LABEL,
    labelForService: remoteCarrierExitLabel,
    unavailableLabel: "Selected Exit Node (unavailable)",
  });
  const summaryText = selectedExit
    ? remoteCarrierExitSummary(selectedExit)
    : selectedRemoteExitId
    ? "Your selected Exit Node is unavailable. Choose another Exit Node to change where Browser traffic exits."
    : LOCAL_EXIT_SUMMARY;
  if (exitSummaryNode) {
    exitSummaryNode.textContent = summaryText;
  }
  updateSettingsTitle();
}

function syncEngineSelect(summary) {
  if (!engineSelect) {
    return;
  }
  const engines = visibleBrowserEngines(summary);
  const selectedEngine = renderServiceSelection(engineSelect, {
    services: engines,
    selectedId: selectedBrowserEngineId,
    defaultLabel: DEFAULT_ENGINE_LABEL,
    labelForService: (engine) => `${browserEngineKindLabel(engine)}: ${engine.id}`,
    unavailableLabel: "Selected Browser Engine (unavailable)",
  });
  if (engineSummaryNode) {
    engineSummaryNode.textContent = selectedBrowserEngineId && !selectedEngine
      ? "Your selected Browser Engine is unavailable. Choose another Engine to change where the page runs."
      : browserEngineSummary(selectedEngine);
  }
  updateSettingsTitle();
}

async function fetchBrowserSummary({ remoteServices = true } = {}) {
  if (browserSummaryPromise) {
    return browserSummaryPromise;
  }
  const boot = typeof window !== "undefined" ? window.__elastosBrowserRestoreBoot : null;
  if (!remoteServices && boot?.summaryPromise) {
    const pending = boot.summaryPromise;
    boot.summaryPromise = null;
    try {
      const summary = await pending;
      browserSummary = summary;
      syncEngineSelect(summary);
      syncExitSelect(summary);
      return summary;
    } catch {
      // Fall through to a live summary request when the boot prefetch fails.
    }
  }
  const query = new URLSearchParams();
  if (browserInstanceId) {
    query.set("browser_instance", browserInstanceId);
  }
  if (!remoteServices) {
    query.set("remote_services", "0");
  }
  const encoded = query.toString();
  const summaryPath = encoded ? `/api/apps/browser/summary?${encoded}` : "/api/apps/browser/summary";
  browserSummaryPromise = fetchJson(summaryPath, { method: "GET" })
    .then((summary) => {
      browserSummary = summary;
      syncEngineSelect(summary);
      syncExitSelect(summary);
      return summary;
    })
    .catch((error) => {
      browserSummary = null;
      syncEngineSelect(null);
      syncExitSelect(null);
      throw error;
    })
    .finally(() => {
      browserSummaryPromise = null;
    });
  return browserSummaryPromise;
}

function explicitDisplayModeParam() {
  return params.get("display_mode") || params.get("display") || "";
}

function explicitGuaranteeLevelParam() {
  return params.get("guarantee_level") || params.get("guarantee") || "";
}

function guaranteeLevelForOpen(displayMode, summary) {
  const explicit = explicitGuaranteeLevelParam();
  if (explicit) {
    return explicit;
  }
  const engine = selectedBrowserEngine(summary);
  const levels = Array.isArray(engine?.supported_guarantee_levels)
    ? engine.supported_guarantee_levels
    : Array.isArray(summary?.engine_adapter?.supported_guarantee_levels)
    ? summary.engine_adapter.supported_guarantee_levels
    : [];
  if (levels.includes(GUARANTEE_MECHANISM_MICROVM)) {
    return GUARANTEE_MECHANISM_MICROVM;
  }
  if (levels.includes(GUARANTEE_OPERATOR_RBI)) {
    return GUARANTEE_OPERATOR_RBI;
  }
  if (levels.includes(GUARANTEE_POLICY_WEBVIEW)) {
    return GUARANTEE_POLICY_WEBVIEW;
  }
  return GUARANTEE_MECHANISM_MICROVM;
}

async function launchContractForOpen() {
  if (explicitDisplayModeParam()) {
    const displayMode = requestedDisplayMode(params, debugMetrics);
    const summary = explicitGuaranteeLevelParam()
      ? null
      : await fetchBrowserSummary();
    return {
      displayMode,
      guaranteeLevel: guaranteeLevelForOpen(displayMode, summary),
    };
  }
  const summary = await fetchBrowserSummary();
  const engine = selectedBrowserEngine(summary);
  const supportedModes = Array.isArray(engine?.supported_display_modes)
    ? engine.supported_display_modes
    : Array.isArray(summary?.engine_adapter?.supported_display_modes)
    ? summary.engine_adapter.supported_display_modes
    : [];
  const displayMode = supportedModes.includes(PRODUCT_DISPLAY_MODE)
    ? PRODUCT_DISPLAY_MODE
    : requestedDisplayMode(params, debugMetrics);
  return {
    displayMode,
    guaranteeLevel: guaranteeLevelForOpen(displayMode, summary),
  };
}

function wait(ms) {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function browserOpenJobError(status) {
  const error = status?.error || {};
  const message =
    error.message ||
    error.error ||
    "Browser Engine failed to start cleanly. Refresh Browser, or choose another Browser Engine.";
  const next = new Error(message);
  next.status = error.http_status || 500;
  next.payload = error;
  return next;
}

async function waitForRuntimeOpen(response, { engineLabel, exitLabel }) {
  if (response?.schema === "elastos.browser.open-result/v1") {
    return response;
  }
  if (
    response?.schema !== "elastos.browser.open-accepted/v1" ||
    !response.status_url
  ) {
    return response;
  }
  const startedAt = Date.now();
  while (Date.now() - startedAt < BROWSER_OPEN_POLL_TIMEOUT_MS) {
    const elapsedSeconds = Math.max(1, Math.round((Date.now() - startedAt) / 1000));
    showStatus(
      `Opening ${engineLabel} with ${exitLabel}... ${elapsedSeconds}s`,
      { sticky: true },
    );
    await wait(BROWSER_OPEN_POLL_INTERVAL_MS);
    const status = await fetchJson(response.status_url, { method: "GET" });
    if (status?.schema !== "elastos.browser.open-status/v1") {
      throw new Error("Browser did not finish starting.");
    }
    if (status.status === "completed") {
      return status.result;
    }
    if (status.status === "failed") {
      throw browserOpenJobError(status);
    }
  }
  throw new Error(
    "Browser Engine startup is still pending. Refresh Browser, or choose another Browser Engine.",
  );
}

async function requestRuntimeOpen(value, { history = "push" } = {}) {
  if (
    runtimeOpenInFlight > 0 ||
    homeWindowCloseInFlight ||
    pendingHomeWindowCloseDelivery ||
    homeWindowTerminalCloseConfirmed
  ) {
    throw cleanupPendingError({
      state: "pending",
      page_id: currentPage?.page_id || "",
      generation: currentPageGeneration,
      reason: runtimeOpenInFlight > 0
        ? "runtime_open_in_flight"
        : homeWindowTerminalCloseConfirmed
          ? "home_window_close_confirmed"
          : pendingHomeWindowCloseDelivery
            ? "home_window_close_pending"
            : "home_window_close_in_flight",
    });
  }
  const nextUrl = normalizeRuntimeOpenUrl(value);
  const visibleAddress = visibleAddressForUrl(nextUrl);
  const browserEngineId = selectedBrowserEngineId;
  const engineLabel = browserEngineLabel(browserEngineId);
  const remoteExitId = selectedRemoteExitId;
  const exitLabel = browserExitLabel(remoteExitId);
  const isExitSwitch =
    Boolean(currentPage?.page_id) && remoteExitId !== currentRemoteExitId;
  const isEngineSwitch =
    Boolean(currentPage?.page_id) && browserEngineId !== currentBrowserEngineId;
  setLoading(true);
  showStatus(`Opening ${visibleAddress} using ${engineLabel} and ${exitLabel}...`, {
    sticky: true,
  });
  let openedOwner = null;
  let runtimePostStarted = false;
  runtimeOpenInFlight += 1;
  try {
    const { displayMode, guaranteeLevel } = await launchContractForOpen();
    requireViewer(displayMode);
    const previousPage = currentPage;
    const previousGeneration = currentPageGeneration;
    const previousOwner = runtimePageOwner(previousPage, previousGeneration);
    const stalePage = previousPage ? null : recoverableRuntimePage();
    if (previousPage?.page_id || stalePage?.page_id) {
      showStatus(
        isExitSwitch
          ? "Closing current Browser session before switching Exit Node..."
          : isEngineSwitch
            ? "Closing current Browser session before switching Browser Engine..."
          : "Closing previous Browser page...",
        { sticky: true },
      );
    }
    const previousClose = await closeRuntimePage(previousPage, {
      generation: previousGeneration,
      explicitRetry: true,
    });
    requireTerminalRuntimePageCloseOutcome(previousClose);
    const staleClose = await closeRuntimePage(stalePage, {
      explicitRetry: true,
    });
    requireTerminalRuntimePageCloseOutcome(staleClose);
    const ownerAfterClose = currentRuntimePageOwner();
    if (
      ownerAfterClose &&
      !sameRuntimePageOwner(ownerAfterClose, previousOwner)
    ) {
      throw new Error(
        "Browser ownership changed while cleanup was pending. No replacement page was opened.",
      );
    }
    if (currentPage?.page_id) {
      throw cleanupPendingError({
        state: "pending",
        page_id: currentPage.page_id,
        generation: currentPageGeneration,
        reason: "ownership_retained",
      });
    }
    lastRequestedUrl = nextUrl;
    clearAddressDraft();
    setCurrentUrl(nextUrl, { blur: true });
    showStatus(`Opening ${engineLabel} with ${exitLabel}...`, {
      sticky: true,
    });
    const body = {
      url: nextUrl,
      reason: "open browser page",
      display_mode: displayMode,
      guarantee_level: guaranteeLevel,
      async_open: true,
    };
    if (remoteExitId) {
      body.remote_exit_id = remoteExitId;
    }
    if (browserEngineId) {
      body.adapter_id = browserEngineId;
    }
    if (browserInstanceId) {
      body.browser_instance = browserInstanceId;
    }
    const openSettlement = {
      body: { ...body },
      page: null,
      statusUrl: "",
    };
    runtimePostStarted = true;
    runtimeOwnershipTerminallyAbsent = false;
    unsettledRuntimeOpen = openSettlement;
    let accepted;
    try {
      accepted = await fetchJson("/api/apps/browser/open", {
        method: "POST",
        body,
      });
    } catch (error) {
      settleInitialRuntimeOpenPostFailure(openSettlement, error);
      throw error;
    }
    if (
      accepted?.schema === "elastos.browser.open-accepted/v1" &&
      typeof accepted.status_url === "string"
    ) {
      openSettlement.statusUrl = accepted.status_url;
    }
    const response = await waitForRuntimeOpen(accepted, { engineLabel, exitLabel });
    const openedPage = runtimeOpenResultPage(response);
    if (!openedPage) {
      throw new Error("Browser did not open the page.");
    }
    openSettlement.page = openedPage;
    if (remoteExitId && response?.stream_session?.backend !== remoteExitId) {
      throw new Error("Browser could not use the selected Exit Node.");
    }
    currentPage = openedPage;
    currentPageGeneration = nextPageGeneration++;
    openedOwner = currentRuntimePageOwner();
    if (unsettledRuntimeOpen === openSettlement) {
      unsettledRuntimeOpen = null;
    }
    currentBrowserEngineId = browserEngineId;
    currentRemoteExitId = remoteExitId;
    publishRuntimePageForHost(currentPage);
    currentDisplayMode = openedPage.display_session?.mode || "";
    syncDisplayInputFromSession(openedPage.display_session);
    currentView =
      viewFromDisplaySession(openedPage.display_session) || openedPage.view;
    canGoBack = false;
    canGoForward = false;
    updateMetricsNode(null);
    showStatus("Browser session ready. Connecting display...", {
      sticky: true,
    });
    const actualUrl = openedPage.actual_url || openedPage.url || nextUrl;
    syncViewFromResponse(currentView || {});
    syncBrowserLocation(actualUrl, openedPage.title, history, {
      forceAddress: true,
    });
    if (currentDisplayMode !== "webrtc_remote_display") {
      throw new Error(
        `Browser display mode ${currentDisplayMode || "none"} is not supported by this host.`,
      );
    }
    await connectRemoteDisplay(openedPage.display_session, openedPage);
    publishRuntimePageForHost(currentPage);
    startPageStatusPolling();
    startPageHeartbeat();
    if (!remoteDisplay.isTrackReady()) {
      showStatus("Remote display negotiated. Waiting for video...", {
        sticky: true,
      });
    }
  } catch (error) {
    if (!runtimePostStarted) {
      proveRuntimeOwnershipAbsentBeforeDispatch();
    }
    if (
      openedOwner &&
      sameRuntimePageOwner(currentRuntimePageOwner(), openedOwner)
    ) {
      const outcome = await failRuntimeOwnedPage(
        error.runtimeOwnedFailureKind || "signaling",
        friendlyOpenError(error),
      );
      if (outcome?.state !== "terminal") {
        throw cleanupPendingError(outcome);
      }
      throw new Error(
        "Runtime confirmed the failed Browser session closed. You can open the address again or choose another Browser Engine.",
      );
    }
    if (isAuthoritySessionError(error) && requestHomeRelaunch(friendlyOpenError(error))) {
      return;
    }
    showStatus(friendlyOpenError(error), { sticky: true });
    throw error;
  } finally {
    runtimeOpenInFlight -= 1;
    setLoading(false);
  }
}

async function navigateAddress(value) {
  const nextUrl = normalizeUrl(value);
  const currentUrl = currentBrowserUrl();
  const crossStreamTarget = !sameBrowserStreamTarget(currentUrl, nextUrl);
  clearAddressDraft();
  if (!currentPage?.page_id) {
    return requestRuntimeOpen(nextUrl);
  }
  if (selectedBrowserEngineId !== currentBrowserEngineId) {
    return requestRuntimeOpen(nextUrl);
  }
  if (selectedRemoteExitId !== currentRemoteExitId) {
    return requestRuntimeOpen(nextUrl);
  }
  if (isBrowserErrorUrl(currentUrl)) {
    showStatus(`Recovering ${visibleAddressForUrl(nextUrl)} in a fresh Browser session...`, {
      sticky: true,
    });
    return requestRuntimeOpen(nextUrl);
  }
  addressInput.value = nextUrl;
  addressInput.blur();
  setLoading(true);
  showStatus(`Opening ${visibleAddressForUrl(nextUrl)}...`, { sticky: true });
  const openingStatus = statusNode.firstChild;
  try {
    const response = await sendBrowserInput(
      { type: "browser_command", command: "navigate", url: nextUrl },
      { history: "push" },
    );
    if (response?.accepted === true && statusNode.firstChild === openingStatus) {
      window.clearTimeout(statusTimer);
      statusNode.dataset.visible = "false";
    }
    startPageStatusPolling();
  } catch (error) {
    if (crossStreamTarget) {
      showStatus(`Reopening ${visibleAddressForUrl(nextUrl)} in a fresh Browser session...`, {
        sticky: true,
      });
      return requestRuntimeOpen(nextUrl);
    }
    showStatus(friendlyOpenError(error), { sticky: true });
    throw error;
  } finally {
    setLoading(false);
  }
}

async function navigateBrowser(command) {
  if (
    (command === "back" && !canGoBack) ||
    (command === "forward" && !canGoForward)
  ) {
    return;
  }
  try {
    await sendBrowserInput(
      { type: "browser_command", command },
      { history: "replace" },
    );
    updateNavState();
  } catch {
    updateNavState();
  }
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  clearAddressDraft();
  navigateAddress(addressInput.value).catch(() => {});
});

addressInput.addEventListener("input", markAddressDraftEdited);

engineSelect?.addEventListener("change", () => {
  selectedBrowserEngineId = engineSelect.value || "";
  syncEngineSelect(browserSummary);
  if (currentPage?.page_id && selectedBrowserEngineId !== currentBrowserEngineId) {
    const nextUrl = currentBrowserUrl();
    requestRuntimeOpen(nextUrl, { history: "replace" }).catch((error) => {
      selectedBrowserEngineId = currentBrowserEngineId;
      syncEngineSelect(browserSummary);
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }
});

exitSelect?.addEventListener("change", () => {
  selectedRemoteExitId = exitSelect.value || "";
  syncExitSelect(browserSummary);
  if (currentPage?.page_id && selectedRemoteExitId !== currentRemoteExitId) {
    const nextUrl = currentBrowserUrl();
    requestRuntimeOpen(nextUrl, { history: "replace" }).catch((error) => {
      selectedRemoteExitId = currentRemoteExitId;
      syncExitSelect(browserSummary);
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }
});

settingsButton?.addEventListener("click", (event) => {
  event.preventDefault();
  event.stopPropagation();
  const willOpen = Boolean(settingsPanel?.hidden);
  setSettingsOpen(willOpen);
  if (willOpen) {
    void fetchBrowserSummary().catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }
});

settingsCloseButton?.addEventListener("click", (event) => {
  event.preventDefault();
  setSettingsOpen(false);
  settingsButton?.focus({ preventScroll: true });
});

document.addEventListener("click", (event) => {
  if (
    settingsPanel?.hidden ||
    settingsPanel?.contains(event.target) ||
    settingsButton?.contains(event.target)
  ) {
    return;
  }
  setSettingsOpen(false);
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !settingsPanel?.hidden) {
    setSettingsOpen(false);
    settingsButton?.focus({ preventScroll: true });
  }
});

addressInput.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") {
    return;
  }
  event.preventDefault();
  clearAddressDraft();
  resetAddressToCurrent();
});

backButton.addEventListener("click", () => {
  navigateBrowser("back").catch((error) =>
    showStatus(friendlyOpenError(error), { sticky: true }),
  );
});

forwardButton.addEventListener("click", () => {
  navigateBrowser("forward").catch((error) =>
    showStatus(friendlyOpenError(error), { sticky: true }),
  );
});

refreshButton.addEventListener("click", () => {
  sendBrowserInput(
    { type: "browser_command", command: "reload" },
    { history: "replace" },
  ).catch((error) => {
    showStatus(friendlyOpenError(error), { sticky: true });
  });
});

async function resetBrowserProfile() {
  if (!launchToken) {
    showStatus("Browser profile reset requires a Browser launch token.", { sticky: true });
    return;
  }
  if (!window.confirm("Reset Browser cookies, local storage, history, and cache for this account?")) {
    return;
  }
  const activePage = currentPage;
  const activeGeneration = currentPageGeneration;
  const activeOwner = runtimePageOwner(activePage, activeGeneration);
  const rememberedPage = recoverableRuntimePage();
  const stalePage =
    rememberedPage?.page_id && rememberedPage.page_id !== activePage?.page_id
      ? rememberedPage
      : null;
  setLoading(true);
  showStatus("Closing Browser page before profile reset...", { sticky: true });
  try {
    const activeClose = await closeRuntimePage(activePage, {
      explicitRetry: true,
    });
    requireTerminalRuntimePageCloseOutcome(activeClose);
    const staleClose = await closeRuntimePage(stalePage, {
      explicitRetry: true,
    });
    requireTerminalRuntimePageCloseOutcome(staleClose);
    const ownerAfterClose = currentRuntimePageOwner();
    if (ownerAfterClose && !sameRuntimePageOwner(ownerAfterClose, activeOwner)) {
      throw new Error(
        "Browser ownership changed while cleanup was pending. The profile was not reset.",
      );
    }
    if (currentPage?.page_id) {
      throw cleanupPendingError({
        state: "pending",
        page_id: currentPage.page_id,
        generation: currentPageGeneration,
        reason: "ownership_retained",
      });
    }
    const response = await fetchJson("/api/apps/browser/profile/reset", {
      method: "POST",
    });
    showStatus(
      response?.removed_profile_disk
        ? "Browser profile reset. Open the address again."
        : "Browser profile was already clear.",
      { sticky: true },
    );
  } catch (error) {
    showStatus(friendlyOpenError(error), { sticky: true });
  } finally {
    setLoading(false);
  }
}

profileResetButton?.addEventListener("click", () => {
  resetBrowserProfile().catch((error) => {
    showStatus(friendlyOpenError(error), { sticky: true });
  });
});

bindBrowserInputSurface({
  copyRemoteClipboardToHost,
  friendlyOpenError,
  getCurrentPage: () => currentPage,
  getCurrentView: () => currentView,
  keyboardCapture,
  pasteHostClipboardIntoRemote,
  readHostClipboardText: homeClipboard.readText,
  remoteVideo,
  renderPanel,
  sendBrowserInput,
  showStatus,
  unlockRemoteAudioFromGesture,
});

async function attachRecoveredDisplay(summary, owner) {
  if (!runtimeViewerOwnerActive(owner)) return null;
  const display = currentPage?.display_session;
  const validGeneration = value => typeof value === "string" && /^display:[a-f0-9]{32}$/.test(value);
  const validRequest = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
  if (summary?.engine_adapter?.display_attach_supported !== true || !validGeneration(display?.display_generation)) {
    throw new Error("Browser needs an update to restore this display. Close Browser to finish the session.");
  }
  const pending = summary.sessions.recoverable_page.display_attachment;
  if (pending && (pending.schema !== "elastos.browser.display-attachment/v1" ||
      !["pending", "ready", "failed"].includes(pending.state) ||
      !validRequest(pending.request_id) || !validGeneration(pending.previous_display_generation))) {
    throw new Error("Runtime could not check the pending Browser display attachment.");
  }
  const request = displayAttachRequestForRecovery(
    owner.page_id,
    display.display_generation,
    pending,
  );
  let result;
  const boot = typeof window !== "undefined" ? window.__elastosBrowserRestoreBoot : null;
  const bootAttach = boot?.attachPromise;
  if (bootAttach) {
    boot.attachPromise = null;
    try {
      const attached = await bootAttach;
      if (attached?.page_id === owner.page_id && attached.request && attached.result) {
        request.request_id = attached.request.request_id;
        request.display_generation = attached.request.display_generation;
        result = attached.result;
      }
    } catch {
      result = undefined;
    }
  }
  if (!result) {
    try {
      result = await fetchJson(`/api/apps/browser/pages/${encodeURIComponent(owner.page_id)}/webrtc`, {
        method: "POST",
        body: request,
        signal: AbortSignal.timeout(viewerDisplayAttachTimeoutMs()),
      });
    } catch (error) {
      if (!runtimeViewerOwnerActive(owner)) return null;
      if (error?.payload?.code === "display_attach_failed") {
        markDisplayAttachRetryNeeded(owner.page_id, request.display_generation);
      }
      throw error;
    }
  }
  if (!runtimeViewerOwnerActive(owner)) return null;
  const offerValid = offer => offer?.schema === "elastos.browser.webrtc-offer/v1" && offer.type === "offer" &&
    typeof offer.sdp === "string" && offer.sdp.length > 0 && offer.sdp.length <= 256 * 1024 &&
    (offer.candidates === undefined || (Array.isArray(offer.candidates) && offer.candidates.length <= 256));
  if (result?.schema !== "elastos.browser.display-attach-result/v1" || result.page_id !== owner.page_id ||
      result.request_id !== request.request_id || result.previous_display_generation !== request.display_generation ||
      !validGeneration(result.display_generation) || result.display_generation === request.display_generation ||
      !offerValid(result.initial_offer) || !offerValid(result.audio_offer)) {
    throw new Error("Runtime could not restore the Browser display.");
  }
  return { ...display, display_generation: result.display_generation,
    initial_offer: result.initial_offer, audio_offer: result.audio_offer };
}

async function restoreRuntimePageViewer(summary) {
  if (unloadCleanupStarted || homeWindowCloseInFlight || homeWindowTerminalCloseConfirmed || currentPage) return true;
  const sessions = summary?.sessions;
  if (sessions?.schema !== "elastos.browser.session-capacity/v1" ||
      !Object.hasOwn(sessions, "recoverable_page")) {
    throw new Error("Browser could not check its current session. Reload Browser to retry.");
  }
  if (sessions.recoverable_page === null) {
    if (sessions.status !== "configured" || sessions.fresh_start_allowed !== true) {
      throw new Error("Runtime is still resolving Browser session ownership. Reload Browser to retry.");
    }
    return false;
  }
  const page = recoverableRuntimePage(summary);
  if (!page || (page.recovery_state === "active" &&
      page.page_id !== sessions.recoverable_page.engine_page?.page_id)) {
    throw new Error("Browser could not restore its current session.");
  }
  const selection = sessions.recoverable_page.service_selection;
  if (page.recovery_state === "active" &&
      (page.schema !== "elastos.browser.engine.page/v1" ||
       selection?.schema !== "elastos.browser.service-selection/v1" ||
       typeof selection.engine_id !== "string" || selection.engine_id.length > 512 ||
       typeof selection.exit_id !== "string" || selection.exit_id.length > 512)) {
    throw new Error("Runtime could not restore the Browser service selection.");
  }
  const owner = runtimePageOwner(page, nextPageGeneration);
  if (!owner) throw new Error("Runtime could not restore Browser cleanup authority.");
  currentPage = page;
  currentPageGeneration = nextPageGeneration++;
  runtimeOwnershipTerminallyAbsent = false;
  if (page.recovery_state === "cleanup_pending") {
    publishRuntimePageForHost(currentPage);
    showStatus("Browser session cleanup is pending. Close Browser to retry.", { sticky: true });
    setLoading(false);
    return true;
  }
  restoredViewerOwner = owner;
  selectedBrowserEngineId = currentBrowserEngineId = selection.engine_id;
  selectedRemoteExitId = currentRemoteExitId = selection.exit_id;
  syncEngineSelect(summary);
  syncExitSelect(summary);
  showStatus("Restoring the Browser display...", { sticky: true });
  startPageHeartbeat();
  let statusPromise = null;
  try {
    // Page status is diagnostic; attach replaces offers within retained Runtime authority.
    if (currentPage.display_session?.mode !== "webrtc_remote_display") {
      throw new Error("Runtime could not restore the Browser display.");
    }
    statusPromise = fetchPageStatus({ history: "replace", forceAddress: true });
    const display = await attachRecoveredDisplay(summary, owner);
    if (!display || !runtimeViewerOwnerActive(owner)) {
      await statusPromise.catch(() => null);
      return true;
    }
    if (display.mode !== "webrtc_remote_display") {
      throw new Error("Runtime could not restore the Browser display.");
    }
    currentDisplayMode = display.mode;
    syncDisplayInputFromSession(display);
    currentView = viewFromDisplaySession(display) || currentPage.view;
    startPageStatusPolling();
    const connecting = connectRemoteDisplay(display, { ...currentPage, display_session: display });
    let status;
    try {
      status = await statusPromise;
    } catch (error) {
      await connecting.catch(() => null);
      throw error;
    }
    if (!status || !runtimeViewerOwnerActive(owner)) {
      await connecting.catch(() => null);
      return true;
    }
    currentPage = { ...currentPage, display_session: display };
    publishRuntimePageForHost(currentPage);
    await connecting;
    return true;
  } catch (error) {
    if (statusPromise) await statusPromise.catch(() => null);
    if (!runtimeViewerOwnerActive(owner)) return true;
    throw error;
  } finally {
    if (runtimeViewerOwnerActive(owner)) setLoading(false);
  }
}

window.addEventListener("beforeunload", () => {
  startRecoverableDisplayAttach();
  releaseRuntimePageForUnload();
});

window.addEventListener("pagehide", (event) => {
  if (!event.persisted) {
    startRecoverableDisplayAttach();
  }
  releaseRuntimePageForUnload();
});

mountOperatorApproval({
  container: settingsPanel,
  controller: createOperatorApprovalController({
    fetchJson,
    runtimeOrigin: new URL(window.location.href).origin,
    getOwner: () => unloadCleanupStarted || homeWindowCloseInFlight
      ? null : runtimePageOwner(currentPage, currentPageGeneration),
    sameOwner: sameRuntimePageOwner,
  }),
});

const requestedStartupUrl = params.get("url");
const initialUrl = requestedStartupUrl || DEFAULT_URL;
addressInput.value = initialUrl;
setLoading(true);
fetchBrowserSummary({ remoteServices: false })
  .then(async (summary) => {
    if (await restoreRuntimePageViewer(summary)) return;
    if (!requestedStartupUrl) {
      setLoading(false);
      return;
    }
    return requestRuntimeOpen(requestedStartupUrl, { history: "replace" });
  })
  .catch((error) => {
    if (unloadCleanupStarted || homeWindowCloseInFlight || homeWindowTerminalCloseConfirmed) return;
    if (isAuthoritySessionError(error) && requestHomeRelaunch(friendlyOpenError(error))) {
      return;
    }
    setLoading(false);
    showStatus(friendlyOpenError(error), { sticky: true });
  });
