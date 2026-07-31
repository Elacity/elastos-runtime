import {
  localBrowserInstanceId,
  publishRuntimePageForHost as publishRuntimePageForHostForKey,
  rememberedRuntimePage as rememberedRuntimePageForKey,
} from "./browser-history.js?v=browser-20260520e";
import { createBrowserLocationController } from "./browser-location.js?v=browser-20260523b";
import {
  DEFAULT_URL,
  createRuntimeApi,
  isBrowserErrorUrl,
  normalizeUrl,
  sameBrowserStreamTarget,
  visibleAddressForUrl,
} from "./browser-runtime-api.js?v=browser-20260627b";
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
} from "./browser-page-cleanup.js?v=browser-20260725a";
import { selkiesMessagesForInput } from "./browser-input.js?v=browser-20260520e";
import { bindBrowserInputSurface } from "./browser-input-surface.js?v=browser-20260725b";
import {
  browserMetricsText,
  friendlyOpenError,
  isAuthoritySessionError,
  isMissingRuntimePageError,
  requestedDisplayMode,
} from "./browser-status.js?v=browser-20260725a";
import { createBrowserRemoteDisplay } from "./browser-remote-display.js?v=browser-20260724a";

const STATUS_TTL_MS = 4200;
const PAGE_STATUS_INTERVAL_MS = 2_500;
const PAGE_STATUS_FIRST_POLL_MS = 1200;
const PAGE_STATUS_AFTER_INPUT_DELAY_MS = 650;
const PAGE_STATUS_AFTER_SCROLL_DELAY_MS = 1200;
const PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS = [650, 1800, 3500, 6500];
const PAGE_HEARTBEAT_INTERVAL_MS = 60_000;
const BROWSER_OPEN_POLL_INTERVAL_MS = 1_200;
const BROWSER_OPEN_POLL_TIMEOUT_MS = 5 * 60_000;
const REMOTE_DISPLAY_RECOVERY_MAX_ATTEMPTS = 1;
const LIBRARY_FILE_PICKER_MAX_BYTES = 16 * 1024 * 1024;
const PRODUCT_DISPLAY_MODE = "webrtc_remote_display";
const PRODUCT_DISPLAY_ASPECT_WIDTH = 16;
const PRODUCT_DISPLAY_ASPECT_HEIGHT = 9;
const GUARANTEE_MECHANISM_MICROVM = "mechanism_microvm";
const GUARANTEE_OPERATOR_RBI = "operator_rbi";
const GUARANTEE_POLICY_WEBVIEW = "policy_webview";
const LOCAL_EXIT_LABEL = "This device";
const LOCAL_EXIT_SUMMARY = "Use this device's Exit Node for Browser traffic.";
const DEFAULT_ENGINE_LABEL = "Automatic";
const DEFAULT_ENGINE_SUMMARY = "Use the best Browser Engine available.";
const params = new URLSearchParams(window.location.search);
const launchToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeParentOrigin = params.get("home_origin") || "";
const debugMetrics =
  params.get("debug") === "1" || params.get("metrics") === "1";
const browserInstanceId =
  params.get("browser_instance") || localBrowserInstanceId();
const RUNTIME_PAGE_STORAGE_KEY = `elastos.browser.current_page_id:${browserInstanceId}`;
const { fetchJson } = createRuntimeApi({ launchToken });
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
let currentView = null;
let currentDisplayMode = "";
let currentDisplayInput = "runtime_route";
let currentDisplayInputProtocol = "elastos_json";
let statusTimer = 0;
let lastViewport = null;
let canGoBack = false;
let canGoForward = false;
let pageStatusTimer = 0;
let pageStatusRefreshTimers = [];
let pageHeartbeatTimer = 0;
let lastPageStatus = null;
let unloadCleanupStarted = false;
let remoteDisplay = null;
let relaunchRequested = false;
let remoteReconnectTimer = 0;
let remoteReconnectInFlight = false;
let remoteReconnectAttempt = 0;
let lastRequestedUrl = DEFAULT_URL;
let lastLibraryFilePickerRequestId = "";
let browserSummary = null;
let browserSummaryPromise = null;
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

function requestHomeRelaunch(reason) {
  if (relaunchRequested || !window.parent || window.parent === window) {
    return false;
  }
  relaunchRequested = true;
  showStatus(reason || "Browser session expired. Reopening from Home...", {
    sticky: true,
  });
  window.top.postMessage(
    {
      type: "home:relaunch-self",
      homeToken: launchToken,
      reason: reason || "browser_authority_expired",
    },
    homeParentOrigin,
  );
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

function rememberedRuntimePage() {
  return rememberedRuntimePageForKey(RUNTIME_PAGE_STORAGE_KEY);
}

function publishRuntimePageForHost(page = currentPage) {
  publishRuntimePageForHostForKey(RUNTIME_PAGE_STORAGE_KEY, page);
}

function currentRuntimePageOwner() {
  return runtimePageOwner(currentPage, currentPageGeneration);
}

function finalizeRuntimePageClose(owner) {
  if (sameRuntimePageOwner(currentRuntimePageOwner(), owner)) {
    currentPage = null;
    currentPageGeneration = 0;
    currentBrowserEngineId = "";
    currentRemoteExitId = "";
    publishRuntimePageForHost(null);
    stopPageStatusPolling();
    stopPageHeartbeat();
    closeRemoteDisplay();
    updateMetricsNode(null);
    updateNavState();
    return true;
  }
  const rememberedPage = rememberedRuntimePage();
  if (
    !currentPage &&
    owner.generation === 0 &&
    rememberedPage?.page_id === owner.page_id
  ) {
    publishRuntimePageForHost(null);
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
      },
    ),
  onTerminal: (owner) => {
    const applied = finalizeRuntimePageClose(owner);
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
      page === currentPage ? currentPageGeneration : 0,
    schedule = true,
  } = {},
) {
  const owner = runtimePageOwner(page, generation);
  return runtimePageCleanup.reconcile(owner, { schedule });
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

function stopRemoteReconnect() {
  window.clearTimeout(remoteReconnectTimer);
  remoteReconnectTimer = 0;
  remoteReconnectInFlight = false;
}

function remoteReconnectUrl() {
  return (
    currentPage?.actual_url ||
    currentPage?.url ||
    getCurrentUrl() ||
    lastRequestedUrl ||
    DEFAULT_URL
  );
}

function scheduleRemoteReconnect(message, { retry = true } = {}) {
  if (unloadCleanupStarted || relaunchRequested) {
    return;
  }
  if (!retry || remoteReconnectAttempt >= REMOTE_DISPLAY_RECOVERY_MAX_ATTEMPTS) {
    const failedPage = currentPage;
    const failedGeneration = currentPageGeneration;
    const mediaRouteUnavailable = /no secure display relay candidate|shared secure display route/i.test(message);
    showStatus(
      `${message} ${
        mediaRouteUnavailable
          ? "No automatic display retry was attempted."
          : "The Browser Engine started, but video did not become ready."
      } Runtime cleanup is pending; the current page remains owned and no replacement will open until Runtime confirms a terminal close.`,
      { sticky: true },
    );
    closeRuntimePage(failedPage, { generation: failedGeneration })
      .then((outcome) => {
        if (outcome?.state === "terminal") {
          showStatus(
            "Runtime confirmed the failed Browser session closed. Refresh Browser to retry.",
            { sticky: true },
          );
        }
      })
      .catch(() => {});
    return;
  }
  if (remoteReconnectInFlight || remoteReconnectTimer) {
    return;
  }
  const nextUrl = remoteReconnectUrl();
  const delay = Math.min(30_000, 1_000 * (2 ** Math.min(remoteReconnectAttempt, 5)));
  showStatus(
    `${message} Reconnecting ${nextUrl}${delay > 1000 ? ` in ${Math.round(delay / 1000)}s` : ""}.`,
    { sticky: true },
  );
  remoteReconnectTimer = window.setTimeout(async () => {
    remoteReconnectTimer = 0;
    if (unloadCleanupStarted || relaunchRequested) {
      return;
    }
    remoteReconnectInFlight = true;
    try {
      await requestRuntimeOpen(nextUrl, { history: "replace", reconnect: true });
      if (relaunchRequested) {
        return;
      }
      if (remoteDisplay.isTrackReady()) {
        remoteReconnectAttempt = 0;
        showStatus("Browser session reconnected.");
      } else {
        remoteReconnectAttempt += 1;
        showStatus(
          "Browser session reopened, but video is still waiting. One recovery attempt has been used.",
          { sticky: true },
        );
      }
    } catch (error) {
      if (!isAuthoritySessionError(error)) {
        remoteReconnectAttempt += 1;
        remoteReconnectInFlight = false;
        if (remoteReconnectAttempt >= REMOTE_DISPLAY_RECOVERY_MAX_ATTEMPTS) {
          showStatus(
            `${friendlyOpenError(error)} Browser display recovery stopped after one attempt.`,
            { sticky: true },
          );
        } else {
          scheduleRemoteReconnect(friendlyOpenError(error));
        }
      }
    } finally {
      remoteReconnectInFlight = false;
    }
  }, delay);
}

function recoverMissingRuntimePage(error, message) {
  if (!isMissingRuntimePageError(error)) {
    return false;
  }
  scheduleRemoteReconnect(message);
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
  resizeObserver.disconnect();
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
  const query = fast ? "?fast=1" : "";
  const status = await fetchJson(
    `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/status${query}`,
    { method: "GET" },
  );
  if (status?.schema !== "elastos.browser.page-status/v1") {
    throw new Error("Browser could not read the page status.");
  }
  if (status.direct_network !== false) {
    throw new Error(
      "Browser reported an unsafe network setup.",
    );
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
  if (chooser?.pending !== true || !chooser.request_id) {
    return;
  }
  if (chooser.request_id === lastLibraryFilePickerRequestId) {
    return;
  }
  lastLibraryFilePickerRequestId = chooser.request_id;
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
  if (!payload?.blob || typeof payload.blob.arrayBuffer !== "function") {
    showStatus("Library did not return file bytes for Browser.", { sticky: true });
    return;
  }
  if (payload.blob.size > LIBRARY_FILE_PICKER_MAX_BYTES) {
    showStatus("Browser file picker accepts Library items up to 16 MiB.", {
      sticky: true,
    });
    return;
  }
  const fileName = fileNameFromLibraryPayload(payload);
  showStatus(`Inserting ${fileName}...`);
  await sendBrowserInput(
    {
      type: "file_upload",
      file_name: fileName,
      mime_type:
        typeof payload.mimeType === "string" && payload.mimeType
          ? payload.mimeType
          : payload.blob.type || "application/octet-stream",
      content_base64: await base64FromBlob(payload.blob),
      object_uri: typeof payload.objectUri === "string" ? payload.objectUri : "",
    },
    { history: "replace" },
  );
  lastLibraryFilePickerRequestId = "";
  showStatus(`Inserted ${fileName}.`);
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
        !currentPage ||
        document.hidden ||
        (!forceAddress && isAddressEditing())
      ) {
        return;
      }
      try {
        await fetchPageStatus({ history, forceAddress });
      } catch (error) {
        if (isMissingRuntimePageError(error)) {
          scheduleRemoteReconnect("Browser session was released.");
          return;
        }
        if (debugMetrics) {
          showStatus(friendlyOpenError(error), { sticky: true });
        }
      }
    }, nextDelay);
    return timer;
  });
}

function startPageStatusPolling() {
  stopPageStatusPolling();
  const poll = async () => {
    if (!currentPage) {
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
    try {
      await fetchPageStatus({ fast: true });
    } catch (error) {
      if (isMissingRuntimePageError(error)) {
        scheduleRemoteReconnect("Browser session was released.");
        return;
      }
      if (debugMetrics) {
        showStatus(friendlyOpenError(error), { sticky: true });
      }
    } finally {
      if (currentPage) {
        pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_INTERVAL_MS);
      }
    }
  };
  pageStatusTimer = window.setTimeout(poll, PAGE_STATUS_FIRST_POLL_MS);
}

function startPageHeartbeat() {
  stopPageHeartbeat();
  const beat = async () => {
    if (!currentPage?.page_id) {
      return;
    }
    try {
      await fetchJson(
        `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/heartbeat`,
        { method: "POST" },
      );
    } catch (error) {
      if (isMissingRuntimePageError(error)) {
        scheduleRemoteReconnect("Browser session heartbeat was lost.");
        return;
      }
      if (debugMetrics) {
        showStatus(friendlyOpenError(error), { sticky: true });
      }
    } finally {
      if (currentPage) {
        pageHeartbeatTimer = window.setTimeout(beat, PAGE_HEARTBEAT_INTERVAL_MS);
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

function browserViewport() {
  const rect = renderPanel.getBoundingClientRect();
  const width = Math.max(320, Math.min(3840, Math.round(rect.width || 1280)));
  const height = Math.max(240, Math.min(2160, Math.round(rect.height || 720)));
  return aspectPreservingProductViewport(width, height);
}

function aspectPreservingProductViewport(width, height) {
  const minScale = Math.max(
    Math.ceil(320 / PRODUCT_DISPLAY_ASPECT_WIDTH),
    Math.ceil(240 / PRODUCT_DISPLAY_ASPECT_HEIGHT),
  );
  const maxScale = Math.min(
    Math.floor(3840 / PRODUCT_DISPLAY_ASPECT_WIDTH),
    Math.floor(2160 / PRODUCT_DISPLAY_ASPECT_HEIGHT),
  );
  const requestedScale = Math.min(
    Math.floor(width / PRODUCT_DISPLAY_ASPECT_WIDTH),
    Math.floor(height / PRODUCT_DISPLAY_ASPECT_HEIGHT),
  );
  const scale = Math.max(minScale, Math.min(maxScale, requestedScale));
  return {
    width: PRODUCT_DISPLAY_ASPECT_WIDTH * scale,
    height: PRODUCT_DISPLAY_ASPECT_HEIGHT * scale,
  };
}

function syncViewFromResponse(response) {
  if (typeof response?.can_go_back === "boolean") {
    canGoBack = response.can_go_back;
  }
  if (typeof response?.can_go_forward === "boolean") {
    canGoForward = response.can_go_forward;
  }
  if (Number(response?.width) && Number(response?.height)) {
    currentView = {
      ...(currentView || {}),
      width: Number(response.width),
      height: Number(response.height),
    };
    lastViewport = {
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
  if (!currentPage?.page_id) {
    return;
  }
  const requiresRuntimeRoute =
    event?.type === "browser_command" ||
    event?.type === "resize" ||
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
    let response;
    try {
      response = await fetchJson(
        `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/input`,
        {
          method: "POST",
          body: { event },
        },
      );
    } catch (error) {
      if (recoverMissingRuntimePage(error, "Browser session was released.")) {
        return;
      }
      throw error;
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
  if (event.origin !== homeParentOrigin || event.source !== window.top) {
    return;
  }
  const payload = event.data && typeof event.data === "object" ? event.data : null;
  if (payload?.type !== "browser:file-picker-selection") {
    return;
  }
  handleLibraryFilePickerSelection(payload).catch((error) => {
    showStatus(friendlyOpenError(error), { sticky: true });
  });
});

remoteDisplay = createBrowserRemoteDisplay({
  debugMetrics,
  fetchJson,
  friendlyOpenError,
  getCurrentDisplayMode: () => currentDisplayMode,
  getLastPageStatus: () => lastPageStatus,
  handleRemoteInputChannelMessage,
  handleRemoteInputChannelTeardown: teardownRemoteClipboard,
  onRecoveryRequired: scheduleRemoteReconnect,
  remoteVideo,
  renderEmpty,
  renderPanel,
  resetPageStatus: () => {
    lastPageStatus = null;
  },
  scheduleViewportResize,
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

function closeRemoteDisplay() {
  remoteDisplay?.close();
}

async function connectRemoteDisplay(displaySession) {
  await remoteDisplay.connect(displaySession);
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

function defaultBrowserEngine(summary = browserSummary) {
  const engines = visibleBrowserEngines(summary);
  return engines.find((engine) => engine.default === true) || engines[0] || null;
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
    const engine = defaultBrowserEngine();
    return engine
      ? `${DEFAULT_ENGINE_LABEL} (${browserEngineKindLabel(engine)})`
      : DEFAULT_ENGINE_LABEL;
  }
  const engine = visibleBrowserEngines(browserSummary).find(
    (candidate) => candidate.id === adapterId,
  );
  return engine ? `${browserEngineKindLabel(engine)}: ${engine.id}` : `Browser Engine: ${adapterId}`;
}

function browserEngineSummary(engine) {
  if (!engine) {
    const defaultEngine = defaultBrowserEngine();
    return defaultEngine
      ? `${browserEngineLabel("")}. Browser will use this engine unless you choose another one.`
      : DEFAULT_ENGINE_SUMMARY;
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
  const peer = String(exit?.peer_did || exit?.carrier?.peer_did || "").trim();
  const displayName = String(exit?.display_name || exit?.label || "").trim();
  const label = displayName || id || grant || peer || "shared Exit Node";
  const marker = `${id} ${grant} ${peer}`.toLowerCase();
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
  return exit ? remoteCarrierExitLabel(exit) : `Shared Exit Node: ${remoteExitId}`;
}

function syncExitSelect(summary) {
  if (!exitSelect) {
    return;
  }
  const exits = visibleRemoteCarrierExits(summary);
  const previous = selectedRemoteExitId;
  exitSelect.replaceChildren(new Option(LOCAL_EXIT_LABEL, ""));
  for (const exit of exits) {
    exitSelect.add(new Option(remoteCarrierExitLabel(exit), exit.id));
  }
  selectedRemoteExitId = exits.some((exit) => exit.id === previous)
    ? previous
    : "";
  exitSelect.value = selectedRemoteExitId;
  const selectedExit = exits.find((exit) => exit.id === selectedRemoteExitId);
  const summaryText = selectedExit
    ? remoteCarrierExitSummary(selectedExit)
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
  const previous = selectedBrowserEngineId;
  const defaultEngine = defaultBrowserEngine(summary);
  engineSelect.replaceChildren(new Option(
    defaultEngine
      ? `${DEFAULT_ENGINE_LABEL} (${browserEngineKindLabel(defaultEngine)})`
      : DEFAULT_ENGINE_LABEL,
    "",
  ));
  for (const engine of engines) {
    engineSelect.add(new Option(`${browserEngineKindLabel(engine)}: ${engine.id}`, engine.id));
  }
  selectedBrowserEngineId = engines.some((engine) => engine.id === previous)
    ? previous
    : "";
  engineSelect.value = selectedBrowserEngineId;
  if (engineSummaryNode) {
    engineSummaryNode.textContent = browserEngineSummary(selectedBrowserEngine(summary));
  }
  updateSettingsTitle();
}

async function fetchBrowserSummary() {
  if (browserSummaryPromise) {
    return browserSummaryPromise;
  }
  browserSummaryPromise = fetchJson("/api/apps/browser/summary", { method: "GET" })
    .then((summary) => {
      browserSummary = summary;
      syncEngineSelect(summary);
      syncExitSelect(summary);
      return summary;
    })
    .catch(() => {
      browserSummary = null;
      syncEngineSelect(null);
      syncExitSelect(null);
      return null;
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

async function requestRuntimeOpen(value, { history = "push", reconnect = false } = {}) {
  const nextUrl = normalizeUrl(value);
  const visibleAddress = visibleAddressForUrl(nextUrl);
  const browserEngineId = selectedBrowserEngineId;
  const engineLabel = browserEngineLabel(browserEngineId);
  const remoteExitId = selectedRemoteExitId;
  const exitLabel = browserExitLabel(remoteExitId);
  const isExitSwitch =
    Boolean(currentPage?.page_id) && remoteExitId !== currentRemoteExitId;
  const isEngineSwitch =
    Boolean(currentPage?.page_id) && browserEngineId !== currentBrowserEngineId;
  if (!reconnect) {
    stopRemoteReconnect();
    remoteReconnectAttempt = 0;
  } else {
    window.clearTimeout(remoteReconnectTimer);
    remoteReconnectTimer = 0;
  }
  setLoading(true);
  showStatus(`Opening ${visibleAddress} using ${engineLabel} and ${exitLabel}...`, {
    sticky: true,
  });
  try {
    const { displayMode, guaranteeLevel } = await launchContractForOpen();
    const previousPage = currentPage;
    const previousGeneration = currentPageGeneration;
    const previousOwner = runtimePageOwner(previousPage, previousGeneration);
    const stalePage = previousPage ? null : rememberedRuntimePage();
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
    });
    requireTerminalRuntimePageCloseOutcome(previousClose);
    const staleClose = await closeRuntimePage(stalePage);
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
      viewport: browserViewport(),
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
    const accepted = await fetchJson("/api/apps/browser/open", {
      method: "POST",
      body,
    });
    const response = await waitForRuntimeOpen(accepted, { engineLabel, exitLabel });
    const page = response?.engine_page;
    if (
      response?.schema !== "elastos.browser.open-result/v1" ||
      page?.schema !== "elastos.browser.engine.page/v1"
    ) {
      throw new Error("Browser did not open the page.");
    }
    if (remoteExitId && response?.stream_session?.backend !== remoteExitId) {
      throw new Error("Browser could not use the selected Exit Node.");
    }
    currentPage = page;
    currentPageGeneration = nextPageGeneration++;
    currentBrowserEngineId = browserEngineId;
    currentRemoteExitId = remoteExitId;
    publishRuntimePageForHost(page);
    currentDisplayMode = page.display_session?.mode || "";
    syncDisplayInputFromSession(page.display_session);
    currentView = page.view || viewFromDisplaySession(page.display_session);
    canGoBack = false;
    canGoForward = false;
    updateMetricsNode(null);
    showStatus("Browser session ready. Connecting display...", {
      sticky: true,
    });
    const actualUrl = page.actual_url || page.url || nextUrl;
    syncViewFromResponse(currentView || {});
    syncBrowserLocation(actualUrl, page.title, history, { forceAddress: true });
    if (currentDisplayMode !== "webrtc_remote_display") {
      throw new Error(
        `Browser display mode ${currentDisplayMode || "none"} is not supported by this host.`,
      );
    }
    await connectRemoteDisplay(page.display_session);
    startPageStatusPolling();
    startPageHeartbeat();
    if (!remoteDisplay.isTrackReady()) {
      showStatus("Remote display negotiated. Waiting for video...", {
        sticky: true,
      });
    }
  } catch (error) {
    if (isAuthoritySessionError(error) && requestHomeRelaunch(friendlyOpenError(error))) {
      return;
    }
    showStatus(friendlyOpenError(error), { sticky: true });
    throw error;
  } finally {
    setLoading(false);
  }
}

async function navigateAddress(value) {
  const nextUrl = normalizeUrl(value);
  const currentUrl = remoteReconnectUrl();
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
  try {
    await sendBrowserInput(
      { type: "browser_command", command: "navigate", url: nextUrl },
      { history: "push" },
    );
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
    const nextUrl = remoteReconnectUrl();
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
    const nextUrl = remoteReconnectUrl();
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
    void fetchBrowserSummary();
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
  const rememberedPage = rememberedRuntimePage();
  const stalePage =
    rememberedPage?.page_id && rememberedPage.page_id !== activePage?.page_id
      ? rememberedPage
      : null;
  setLoading(true);
  showStatus("Closing Browser page before profile reset...", { sticky: true });
  stopRemoteReconnect();
  try {
    const activeClose = await closeRuntimePage(activePage);
    requireTerminalRuntimePageCloseOutcome(activeClose);
    const staleClose = await closeRuntimePage(stalePage);
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

function scheduleViewportResize() {
  if (!currentPage) {
    return;
  }
  const viewport = browserViewport();
  lastViewport = viewport;
}

const resizeObserver = new ResizeObserver(scheduleViewportResize);
resizeObserver.observe(renderPanel);

window.addEventListener("beforeunload", () => {
  releaseRuntimePageForUnload();
});

window.addEventListener("pagehide", releaseRuntimePageForUnload);

const initialUrl = params.get("url") || DEFAULT_URL;
addressInput.value = initialUrl;
updateNavState();
void fetchBrowserSummary();
requestRuntimeOpen(initialUrl, { history: "replace" }).catch((error) => {
  if (isAuthoritySessionError(error) && requestHomeRelaunch(friendlyOpenError(error))) {
    return;
  }
  showStatus(friendlyOpenError(error), { sticky: true });
});
