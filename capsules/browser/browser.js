import {
  localBrowserInstanceId,
  publishRuntimePageForHost as publishRuntimePageForHostForKey,
  rememberedRuntimePage as rememberedRuntimePageForKey,
} from "./browser-history.js?v=browser-20260520e";
import { createBrowserLocationController } from "./browser-location.js?v=browser-20260523b";
import {
  DEFAULT_URL,
  createRuntimeApi,
  normalizeUrl,
  streamTargetForUrl,
} from "./browser-runtime-api.js?v=browser-20260520e";
import { createBrowserClipboardBridge } from "./browser-clipboard.js?v=browser-20260524d";
import {
  selkiesMessagesForInput,
  utf8FromBase64,
} from "./browser-input.js?v=browser-20260520e";
import { bindBrowserInputSurface } from "./browser-input-surface.js?v=browser-20260524d";
import {
  browserMetricsText,
  friendlyOpenError,
  isAuthoritySessionError,
  isMissingRuntimePageError,
  requestedDisplayMode,
} from "./browser-status.js?v=browser-20260520e";
import { createBrowserRemoteDisplay } from "./browser-remote-display.js?v=browser-20260523c";

const STATUS_TTL_MS = 4200;
const FRAME_WAIT_MS = 1200;
const RESIZE_FLUSH_MS = 180;
const PAGE_STATUS_INTERVAL_MS = 60_000;
const PAGE_STATUS_FIRST_POLL_MS = 1200;
const PAGE_HEARTBEAT_INTERVAL_MS = 60_000;
const params = new URLSearchParams(window.location.search);
const launchToken = params.get("home_token") || "";
const debugMetrics =
  params.get("debug") === "1" || params.get("metrics") === "1";
const browserInstanceId =
  params.get("browser_instance") || localBrowserInstanceId();
const RUNTIME_PAGE_STORAGE_KEY = `elastos.browser.current_page_id:${browserInstanceId}`;
const { fetchJson, homeHeaders } = createRuntimeApi({ launchToken });

const form = document.querySelector("#browser-form");
const addressInput = document.querySelector("#browser-url");
const backButton = document.querySelector("#browser-back");
const forwardButton = document.querySelector("#browser-forward");
const refreshButton = document.querySelector("#browser-refresh");
const statusNode = document.querySelector("#browser-status");
const renderPanel = document.querySelector("#browser-render-panel");
const renderImage = document.querySelector("#browser-render");
const remoteVideo = document.querySelector("#browser-remote-display");
const keyboardCapture = document.querySelector("#browser-keyboard-capture");
const renderEmpty = document.querySelector("#browser-render-empty");
const metricsNode = document.querySelector("#browser-metrics");

let currentPage = null;
let currentView = null;
let currentDisplayMode = "";
let currentDisplayInput = "runtime_route";
let currentDisplayInputProtocol = "elastos_json";
let renderObjectUrl = "";
let statusTimer = 0;
let frameLoopSerial = 0;
let frameSeq = 0;
let resizeTimer = 0;
let lastViewport = null;
let canGoBack = false;
let canGoForward = false;
let pageStatusTimer = 0;
let pageHeartbeatTimer = 0;
let lastPageStatus = null;
let unloadCleanupStarted = false;
let remoteDisplay = null;
let relaunchRequested = false;
let remoteReconnectTimer = 0;
let remoteReconnectInFlight = false;
let remoteReconnectAttempt = 0;
let lastRequestedUrl = DEFAULT_URL;

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
  const canCopy = Boolean(sticky && message && navigator.clipboard?.writeText);
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
        await navigator.clipboard.writeText(message);
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
  showStatus(reason || "Browser authority expired. Relaunching through Home...", {
    sticky: true,
  });
  window.parent.postMessage(
    {
      type: "home:relaunch-self",
      homeToken: launchToken,
      reason: reason || "browser_authority_expired",
    },
    window.location.origin,
  );
  return true;
}

function setLoading(loading) {
  document.body.dataset.loading = loading ? "true" : "false";
  addressInput.disabled = loading;
  renderEmpty.hidden = true;
  refreshButton.disabled = loading || !currentPage;
  updateNavState();
}

function stopPageStatusPolling() {
  window.clearTimeout(pageStatusTimer);
  pageStatusTimer = 0;
}

function stopPageHeartbeat() {
  window.clearTimeout(pageHeartbeatTimer);
  pageHeartbeatTimer = 0;
}

async function closeRuntimePage(page = currentPage) {
  if (!page?.page_id) {
    return;
  }
  await fetchJson(
    `/api/apps/browser/pages/${encodeURIComponent(page.page_id)}/close`,
    {
      method: "POST",
    },
  ).catch(() => {});
}

function rememberedRuntimePage() {
  return rememberedRuntimePageForKey(RUNTIME_PAGE_STORAGE_KEY);
}

function publishRuntimePageForHost(page = currentPage) {
  publishRuntimePageForHostForKey(RUNTIME_PAGE_STORAGE_KEY, page);
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

function scheduleRemoteReconnect(message) {
  if (unloadCleanupStarted || relaunchRequested) {
    return;
  }
  if (remoteReconnectInFlight || remoteReconnectTimer) {
    return;
  }
  const nextUrl = remoteReconnectUrl();
  const delay = Math.min(30_000, 1_000 * (2 ** Math.min(remoteReconnectAttempt, 5)));
  showStatus(
    `${message} Retrying ${nextUrl} through Runtime${delay > 1000 ? ` in ${Math.round(delay / 1000)}s` : ""}.`,
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
      remoteReconnectAttempt = 0;
      showStatus("Remote display reconnected through Runtime.");
    } catch (error) {
      if (!isAuthoritySessionError(error)) {
        remoteReconnectAttempt += 1;
        remoteReconnectInFlight = false;
        scheduleRemoteReconnect(friendlyOpenError(error));
      }
    } finally {
      remoteReconnectInFlight = false;
    }
  }, delay);
}

function releaseRuntimePageForUnload() {
  if (unloadCleanupStarted) {
    return;
  }
  unloadCleanupStarted = true;
  const pageId = currentPage?.page_id;
  if (pageId) {
    fetch(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/close`, {
      method: "POST",
      headers: homeHeaders(false),
      keepalive: true,
    }).catch(() => {});
  }
  publishRuntimePageForHost(null);
  frameLoopSerial += 1;
  stopPageStatusPolling();
  stopPageHeartbeat();
  closeRemoteDisplay();
  resizeObserver.disconnect();
}

function updateMetricsNode(status) {
  if (!metricsNode) {
    return;
  }
  if (!debugMetrics || !status) {
    metricsNode.hidden = true;
    return;
  }
  metricsNode.textContent = browserMetricsText(status, {
    ...(remoteDisplay?.metricsState() || {}),
    remoteVideo,
  });
  metricsNode.hidden = false;
}

async function fetchPageStatus() {
  if (!currentPage?.page_id) {
    return null;
  }
  const status = await fetchJson(
    `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/status`,
    { method: "GET" },
  );
  if (status?.schema !== "elastos.browser.page-status/v1") {
    throw new Error("Browser Engine Adapter returned an invalid page status.");
  }
  if (status.direct_network !== false) {
    throw new Error(
      "Browser Engine Adapter reported direct network authority.",
    );
  }
  lastPageStatus = status;
  syncViewFromResponse(status);
  if (status.actual_url) {
    currentPage = {
      ...currentPage,
      actual_url: status.actual_url,
      title: status.title || currentPage?.title,
    };
    syncBrowserLocation(status.actual_url, "", "replace");
  }
  updateMetricsNode(status);
  return status;
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
      await fetchPageStatus();
    } catch (error) {
      if (isMissingRuntimePageError(error)) {
        scheduleRemoteReconnect("Browser Runtime page was released.");
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
        scheduleRemoteReconnect("Browser Runtime page heartbeat was lost.");
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
  return { width, height };
}

function setRenderBlob(
  blob,
  { focus = false, preserveRemoteStream = false } = {},
) {
  if (renderObjectUrl) {
    URL.revokeObjectURL(renderObjectUrl);
  }
  renderObjectUrl = URL.createObjectURL(blob);
  remoteVideo.hidden = true;
  if (!preserveRemoteStream) {
    remoteVideo.srcObject = null;
  }
  renderImage.src = renderObjectUrl;
  renderImage.hidden = false;
  renderEmpty.hidden = true;
  document.body.dataset.browserPage = "active";
  if (focus) {
    focusRemoteInput();
  }
}

function renderBase64Image(
  base64,
  contentType = "image/png",
  { focus = false } = {},
) {
  const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  setRenderBlob(new Blob([bytes], { type: contentType }), { focus });
}

async function fetchBrowserFrame({ focus = false } = {}) {
  if (!currentPage?.page_id) {
    return false;
  }
  const response = await fetchJson(
    `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/frame?since=${encodeURIComponent(
      frameSeq,
    )}&wait_ms=${FRAME_WAIT_MS}`,
    {
      method: "GET",
    },
  );
  if (Number.isFinite(Number(response?.seq))) {
    frameSeq = Number(response.seq);
  }
  syncViewFromResponse(response || {});
  syncBrowserLocation(response?.actual_url, response?.title, "replace");
  if (response?.changed === false) {
    return false;
  }
  if (response?.base64) {
    renderBase64Image(response.base64, response.content_type || "image/png", {
      focus,
    });
    return true;
  }
  return false;
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
  if (currentDisplayInputProtocol === "selkies_v1") {
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
    event?.type === "clipboard_write";
  if (
    currentDisplayMode === "webrtc_remote_display" &&
    currentDisplayInput === "datachannel" &&
    !requiresRuntimeRoute
  ) {
    if (!remoteDisplay?.inputChannelOpen()) {
      throw new Error("Browser remote-display input channel is not open.");
    }
    remoteDisplay.sendInputMessages(encodeDatachannelInput(event));
    if (focus) {
      focusRemoteInput();
    }
  } else {
    const response = await fetchJson(
      `/api/apps/browser/pages/${encodeURIComponent(currentPage.page_id)}/input`,
      {
        method: "POST",
        body: { event },
      },
    );
    if (Number.isFinite(Number(response?.seq))) {
      frameSeq = Number(response.seq);
    }
    if (currentDisplayMode === "webrtc_remote_display") {
      syncViewFromResponse(response);
      if (response.actual_url) {
        currentPage = {
          ...currentPage,
          actual_url: response.actual_url,
          title: response.title || currentPage?.title,
        };
      }
      syncBrowserLocation(response.actual_url, response.title, history);
      if (focus) {
        focusRemoteInput();
      }
      return;
    }
    if (response?.screenshot) {
      renderBase64Image(
        response.screenshot,
        response.content_type || "image/png",
        {
          focus,
        },
      );
      syncViewFromResponse(response);
      if (response.actual_url) {
        currentPage = {
          ...currentPage,
          actual_url: response.actual_url,
          title: response.title || currentPage?.title,
        };
      }
      syncBrowserLocation(response.actual_url, response.title, history);
    } else {
      throw new Error(
        "Browser Engine Adapter returned input without an explicit rendered frame.",
      );
    }
  }
}

const {
  copyRemoteClipboardToHost,
  handleRemoteInputChannelMessage,
  pasteHostClipboardIntoRemote,
} = createBrowserClipboardBridge({
  friendlyOpenError,
  getCurrentPage: () => currentPage,
  sendBrowserInput,
  showStatus,
  utf8FromBase64,
});

remoteDisplay = createBrowserRemoteDisplay({
  debugMetrics,
  fetchJson,
  friendlyOpenError,
  getCurrentDisplayMode: () => currentDisplayMode,
  getLastPageStatus: () => lastPageStatus,
  handleRemoteInputChannelMessage,
  incrementFrameLoopSerial: () => {
    frameLoopSerial += 1;
  },
  onRecoveryRequired: scheduleRemoteReconnect,
  remoteVideo,
  renderEmpty,
  renderImage,
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

function startFrameStream() {
  const serial = ++frameLoopSerial;
  const loop = async () => {
    while (serial === frameLoopSerial) {
      if (
        !currentPage ||
        document.hidden ||
        document.body.dataset.loading === "true"
      ) {
        await new Promise((resolve) => window.setTimeout(resolve, 450));
        continue;
      }
      try {
        await fetchBrowserFrame();
      } catch (error) {
        showStatus(friendlyOpenError(error), { sticky: true });
        await new Promise((resolve) => window.setTimeout(resolve, 1500));
      }
    }
  };
  loop();
}

function closeRemoteDisplay() {
  remoteDisplay?.close();
}

async function connectRemoteDisplay(displaySession) {
  await remoteDisplay.connect(displaySession);
}

function unlockRemoteAudioFromGesture() {
  remoteDisplay?.unlockAudioFromGesture();
}

async function requestRuntimeOpen(value, { history = "push", reconnect = false } = {}) {
  const nextUrl = normalizeUrl(value);
  const streamTarget = streamTargetForUrl(nextUrl);
  const displayMode = requestedDisplayMode(params, debugMetrics);
  if (!reconnect) {
    stopRemoteReconnect();
    remoteReconnectAttempt = 0;
  } else {
    window.clearTimeout(remoteReconnectTimer);
    remoteReconnectTimer = 0;
  }
  lastRequestedUrl = nextUrl;
  clearAddressDraft();
  setCurrentUrl(nextUrl, { blur: true });
  setLoading(true);
  showStatus(`Opening ${streamTarget} through Runtime...`, { sticky: true });
  try {
    const previousPage = currentPage;
    const stalePage = previousPage ? null : rememberedRuntimePage();
    closeRemoteDisplay();
    stopPageStatusPolling();
    stopPageHeartbeat();
    frameLoopSerial += 1;
    currentPage = null;
    publishRuntimePageForHost(null);
    await closeRuntimePage(previousPage);
    await closeRuntimePage(stalePage);
    const response = await fetchJson("/api/apps/browser/open", {
      method: "POST",
      body: {
        url: nextUrl,
        reason: "open browser page",
        viewport: browserViewport(),
        display_mode: displayMode,
      },
    });
    const page = response?.engine_page;
    if (
      response?.schema !== "elastos.browser.open-result/v1" ||
      page?.schema !== "elastos.browser.engine.page/v1"
    ) {
      throw new Error("Browser Engine Adapter returned an invalid page.");
    }
    currentPage = page;
    publishRuntimePageForHost(page);
    currentDisplayMode = page.display_session?.mode || "";
    currentDisplayInput = page.display_session?.input || "runtime_route";
    currentView = page.view || viewFromDisplaySession(page.display_session);
    canGoBack = false;
    canGoForward = false;
    frameSeq = 0;
    updateMetricsNode(null);
    const actualUrl = page.actual_url || page.url || nextUrl;
    syncViewFromResponse(currentView || {});
    syncBrowserLocation(actualUrl, page.title, history, { forceAddress: true });
    if (currentDisplayMode === "webrtc_remote_display") {
      await connectRemoteDisplay(page.display_session);
      startPageStatusPolling();
      startPageHeartbeat();
      if (!remoteDisplay.isTrackReady()) {
        showStatus("Remote display negotiated. Waiting for video...", {
          sticky: true,
        });
      }
    } else if (currentDisplayMode === "diagnostic_frame") {
      await fetchBrowserFrame({ focus: true });
      startFrameStream();
      startPageStatusPolling();
      startPageHeartbeat();
      showStatus("Diagnostic frame connected through Runtime.");
    } else {
      throw new Error(
        `Browser display mode ${currentDisplayMode || "none"} is not supported by this host.`,
      );
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
  clearAddressDraft();
  if (!currentPage?.page_id || currentDisplayMode !== "webrtc_remote_display") {
    return requestRuntimeOpen(nextUrl);
  }
  addressInput.value = nextUrl;
  addressInput.blur();
  setLoading(true);
  showStatus(`Opening ${nextUrl} through Runtime...`, { sticky: true });
  try {
    await sendBrowserInput(
      { type: "browser_command", command: "navigate", url: nextUrl },
      { history: "push" },
    );
    startPageStatusPolling();
  } catch (error) {
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

bindBrowserInputSurface({
  copyRemoteClipboardToHost,
  friendlyOpenError,
  getCurrentDisplayMode: () => currentDisplayMode,
  getCurrentPage: () => currentPage,
  getCurrentView: () => currentView,
  keyboardCapture,
  pasteHostClipboardIntoRemote,
  remoteVideo,
  renderImage,
  renderPanel,
  sendBrowserInput,
  showStatus,
  unlockRemoteAudioFromGesture,
});

function scheduleViewportResize({ force = false } = {}) {
  if (!currentPage) {
    return;
  }
  const viewport = browserViewport();
  if (currentDisplayMode === "webrtc_remote_display") {
    lastViewport = viewport;
    return;
  }
  if (
    !force &&
    lastViewport &&
    lastViewport.width === viewport.width &&
    lastViewport.height === viewport.height
  ) {
    return;
  }
  window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => {
    sendBrowserInput(
      { type: "resize", viewport },
      { focus: false, history: "replace" },
    ).catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }, RESIZE_FLUSH_MS);
}

const resizeObserver = new ResizeObserver(scheduleViewportResize);
resizeObserver.observe(renderPanel);

window.addEventListener("beforeunload", () => {
  releaseRuntimePageForUnload();
});

window.addEventListener("pagehide", releaseRuntimePageForUnload);
window.__elastosBrowserReleaseRuntimePage = releaseRuntimePageForUnload;
publishRuntimePageForHost(null);

addressInput.value = DEFAULT_URL;
updateNavState();
requestRuntimeOpen(DEFAULT_URL, { history: "replace" }).catch((error) => {
  if (isAuthoritySessionError(error) && requestHomeRelaunch(friendlyOpenError(error))) {
    return;
  }
  showStatus(friendlyOpenError(error), { sticky: true });
});
