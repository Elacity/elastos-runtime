export const DEFAULT_URL = "https://ela.city/";

export function normalizeUrl(value, defaultUrl = DEFAULT_URL) {
  const trimmed = String(value || "").trim();
  const candidate = trimmed || defaultUrl;
  if (/^tls:\/\//i.test(candidate) || /^tcp:\/\//i.test(candidate)) {
    const transport = new URL(candidate);
    const scheme = transport.protocol === "tls:" ? "https" : "http";
    const defaultPort = transport.protocol === "tls:" ? "443" : "80";
    const port = transport.port && transport.port !== defaultPort ? `:${transport.port}` : "";
    const suffix = `${transport.pathname || "/"}${transport.search}${transport.hash}`;
    return `${scheme}://${transport.hostname}${port}${suffix}`;
  }
  const withScheme = /^[a-z][a-z0-9+.-]*:/i.test(candidate)
    ? candidate
    : `https://${candidate}`;
  const parsed = new URL(withScheme);
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error("Only http and https addresses can be opened.");
  }
  return parsed.toString();
}

export function streamTargetForUrl(value) {
  const parsed = new URL(value);
  const port = parsed.port || (parsed.protocol === "https:" ? "443" : "80");
  const scheme = parsed.protocol === "https:" ? "tls" : "tcp";
  return `${scheme}://${parsed.hostname}:${port}`;
}

export function sameBrowserStreamTarget(currentValue, nextValue) {
  try {
    return (
      streamTargetForUrl(normalizeUrl(currentValue)) ===
      streamTargetForUrl(normalizeUrl(nextValue))
    );
  } catch {
    return false;
  }
}

export function isBrowserErrorUrl(value) {
  const text = String(value || "").trim().toLowerCase();
  return text === "chrome-error://chromewebdata/" || text.startsWith("chrome-error://");
}

export function visibleAddressForUrl(value) {
  const parsed = new URL(value);
  if (parsed.protocol === "https:" && (parsed.hostname === "ela.city" || parsed.hostname.endsWith(".ela.city"))) {
    const suffix = `${parsed.pathname || "/"}${parsed.search}${parsed.hash}`;
    return suffix === "/" ? `${parsed.hostname}://` : `${parsed.hostname}://${suffix.replace(/^\/+/, "")}`;
  }
  return parsed.toString();
}

// The web Runtime adapter reports viewer features, independently of the Engine
// host's OS, virtualization or placement. A capability report is not media proof.
export function browserViewerCapabilities(host = globalThis) {
  const peer = host.RTCPeerConnection?.prototype;
  const report = {
    schema: "elastos.browser.viewer-capabilities/v1",
    display_mode: "webrtc_remote_display",
    peer_connection: typeof host.RTCPeerConnection === "function",
    transceivers: typeof peer?.addTransceiver === "function",
    data_channel: typeof peer?.createDataChannel === "function",
    video_codecs: null,
    audio_codecs: null,
    eligible: false,
    reason: null,
  };
  if (!report.peer_connection || !report.transceivers || !report.data_channel) {
    report.reason = "webrtc_unavailable";
    return report;
  }
  try {
    if (typeof host.RTCRtpReceiver?.getCapabilities === "function") {
      for (const kind of ["video", "audio"]) {
        const codecs = host.RTCRtpReceiver.getCapabilities(kind)?.codecs;
        if (!Array.isArray(codecs)) {
          report.reason = "codec_capabilities_unavailable";
          return report;
        }
        report[`${kind}_codecs`] = [...new Set(codecs
          .map((codec) => String(codec.mimeType || "").toLowerCase())
          .filter((mime) => mime.startsWith(`${kind}/`) && mime.length <= 64))].sort();
        if (!report[`${kind}_codecs`].length) {
          report.reason = `${kind}_decoder_unavailable`;
          return report;
        }
      }
    }
  } catch {
    report.reason = "codec_capabilities_unavailable";
    return report;
  }
  // Older viewers without a codec query proceed to normal SDP negotiation.
  // Null means unreported, not that any particular codec was proven to work.
  report.eligible = true;
  return report;
}

export function requireBrowserViewer(displayMode, host = globalThis) {
  const capabilities = browserViewerCapabilities(host);
  if (displayMode === "webrtc_remote_display" && capabilities.eligible) {
    return capabilities;
  }
  const error = new Error("Browser viewer is incompatible with the requested display.");
  error.status = 400;
  error.payload = {
    code: displayMode === "webrtc_remote_display"
      ? "viewer_unavailable" : "unsupported_viewer_display_mode",
    stage: "viewer_compatibility",
    capabilities,
    outcome: {
      schema: "elastos.browser.open-outcome/v1",
      state: "terminal_pre_effect_failure",
      effects: { page_acquired: false, vm_acquired: false, stream_acquired: false },
    },
  };
  throw error;
}

export function createRuntimeApi({ launchToken }) {
  function homeHeaders(hasBody = false) {
    const headers = {};
    if (launchToken) {
      headers["x-elastos-home-token"] = launchToken;
    }
    if (hasBody) {
      headers["content-type"] = "application/json";
    }
    return headers;
  }

  async function fetchJson(path, options = {}) {
    const body = options.body == null ? undefined : JSON.stringify(options.body);
    let response, text;
    try {
      response = await fetch(path, {
        ...options,
        body,
        headers: {
          ...homeHeaders(Boolean(body)),
          ...(options.headers || {}),
        },
      });
      text = await response.text();
    } catch (cause) {
      const error = new Error("Browser connection interrupted.", { cause });
      // Received HTTP failures retain their authority/policy meaning even if
      // the connection drops before their response body is complete.
      if (response && !response.ok) error.status = response.status;
      else error.runtimeTransportFailure = true;
      throw error;
    }
    let payload = null;
    if (text) {
      try {
        payload = JSON.parse(text);
      } catch {
        payload = text;
      }
    }
    if (!response.ok) {
      const message =
        typeof payload === "string"
          ? payload
          : payload?.error || payload?.message || `request failed: ${response.status}`;
      const error = new Error(message);
      error.status = response.status;
      error.payload = payload;
      throw error;
    }
    return payload;
  }

  return { fetchJson, homeHeaders, requireViewer: requireBrowserViewer };
}
