const VIEWER_OPEN_SCHEMA = "elastos.library.runtime-custody-viewer/v1";
const VIEWER_PART_SCHEMA = "elastos.library.runtime-custody-viewer-part/v1";
const VIEWER_HANDLE_HEX_RE = /^[0-9a-f]{64}$/;
const MINT_ID_HEX_RE = /^[0-9a-f]{64}$/;
export const MAX_VIEWER_SEGMENT_COUNT = 512;
export const MAX_VIEWER_MEDIA_PART_BYTES = 2 * 1024 * 1024;
const MAX_VIEWER_MEDIA_PART_BASE64_BYTES = Math.ceil(MAX_VIEWER_MEDIA_PART_BYTES / 3) * 4;
const OPEN_RESPONSE_KEYS = [
  "codecs",
  "expires_at",
  "has_init_segment",
  "mime_type",
  "mint_id",
  "schema",
  "segment_count",
  "viewer_session_handle",
];
const PART_RESPONSE_KEYS = [
  "data",
  "encoding",
  "mint_id",
  "schema",
  "viewer_session_handle",
];

function hasExactKeys(value, keys) {
  const object = value && typeof value === "object" && !Array.isArray(value) ? value : null;
  if (!object) return false;
  const actual = Object.keys(object).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function readSearchParam(locationLike, key) {
  const search = typeof locationLike?.search === "string" ? locationLike.search : "";
  return new URLSearchParams(search).get(key) || "";
}

function readHashParam(locationLike, key) {
  const hash = typeof locationLike?.hash === "string" ? locationLike.hash.replace(/^#/, "") : "";
  return new URLSearchParams(hash).get(key) || "";
}

export function readLaunchContext(locationLike) {
  return {
    mintId: readSearchParam(locationLike, "mint_id").trim(),
    homeToken: readHashParam(locationLike, "home_token").trim(),
  };
}

function parseJsonObject(text) {
  const value = JSON.parse(text);
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Viewer response is unavailable.");
  }
  return value;
}

async function readResponseText(response) {
  return typeof response?.text === "function" ? response.text() : "";
}

async function readProviderEnvelope(response, fallback) {
  const text = await readResponseText(response);
  let payload = null;
  if (text) {
    try {
      payload = parseJsonObject(text);
    } catch {
      throw new Error(fallback);
    }
  }
  if (!response?.ok) {
    const message =
      typeof payload?.message === "string" && payload.message.trim() ? payload.message : fallback;
    throw new Error(message);
  }
  if (payload?.status === "error") {
    throw new Error(
      typeof payload?.message === "string" && payload.message.trim() ? payload.message : fallback,
    );
  }
  if (payload?.status !== "ok" || !payload.data || typeof payload.data !== "object") {
    throw new Error(fallback);
  }
  return payload.data;
}

export function buildViewerMimeType(mimeType, codecs) {
  const mime = String(mimeType || "").trim();
  const codecValue = String(codecs || "").trim();
  if (!mime || !codecValue) {
    throw new Error("Viewer response is unavailable.");
  }
  return `${mime}; codecs="${codecValue}"`;
}

export function parseViewerOpenData(data, expectedMintId) {
  if (!hasExactKeys(data, OPEN_RESPONSE_KEYS)) {
    throw new Error("Viewer response is unavailable.");
  }
  const mintId = String(data.mint_id || "");
  const viewerSessionHandle = String(data.viewer_session_handle || "");
  const mimeType = String(data.mime_type || "");
  const codecs = String(data.codecs || "");
  const segmentCount = Number(data.segment_count);
  const expiresAt = Number(data.expires_at);
  if (
    data.schema !== VIEWER_OPEN_SCHEMA ||
    mintId !== expectedMintId ||
    !MINT_ID_HEX_RE.test(mintId) ||
    !VIEWER_HANDLE_HEX_RE.test(viewerSessionHandle) ||
    data.has_init_segment !== true ||
    !mimeType ||
    !codecs ||
    !Number.isInteger(segmentCount) ||
    segmentCount <= 0 ||
    segmentCount > MAX_VIEWER_SEGMENT_COUNT ||
    !Number.isInteger(expiresAt) ||
    expiresAt <= 0
  ) {
    throw new Error("Viewer response is unavailable.");
  }
  return {
    mintId,
    viewerSessionHandle,
    mimeType,
    codecs,
    segmentCount,
  };
}

function decodeBase64(base64Text) {
  const value = String(base64Text || "");
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new Error("Video data is unavailable.");
  }
  if (value.length > MAX_VIEWER_MEDIA_PART_BASE64_BYTES) {
    throw new Error("Video data is unavailable.");
  }
  try {
    if (typeof atob === "function") {
      const binary = atob(value);
      if (btoa(binary) !== value) {
        throw new Error("Video data is unavailable.");
      }
      const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
      if (bytes.length > MAX_VIEWER_MEDIA_PART_BYTES) {
        throw new Error("Video data is unavailable.");
      }
      return bytes;
    }
    const buffer = Buffer.from(value, "base64");
    if (buffer.toString("base64") !== value) {
      throw new Error("Video data is unavailable.");
    }
    if (buffer.length > MAX_VIEWER_MEDIA_PART_BYTES) {
      throw new Error("Video data is unavailable.");
    }
    return Uint8Array.from(buffer);
  } catch {
    throw new Error("Video data is unavailable.");
  }
}

export function parseViewerPartData(data, expectedMintId, expectedHandle) {
  if (!hasExactKeys(data, PART_RESPONSE_KEYS)) {
    throw new Error("Video data is unavailable.");
  }
  const mintId = String(data.mint_id || "");
  const viewerSessionHandle = String(data.viewer_session_handle || "");
  if (
    data.schema !== VIEWER_PART_SCHEMA ||
    mintId !== expectedMintId ||
    viewerSessionHandle !== expectedHandle ||
    data.encoding !== "base64" ||
    !MINT_ID_HEX_RE.test(mintId) ||
    !VIEWER_HANDLE_HEX_RE.test(viewerSessionHandle)
  ) {
    throw new Error("Video data is unavailable.");
  }
  const bytes = decodeBase64(String(data.data || ""));
  if (!bytes.length) {
    throw new Error("Video data is unavailable.");
  }
  return bytes;
}

function mediaSourceSupported(MediaSourceLike, mimeType) {
  return (
    MediaSourceLike &&
    typeof MediaSourceLike.isTypeSupported === "function" &&
    MediaSourceLike.isTypeSupported(mimeType)
  );
}

function createDeferred() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

function appendBytes(sourceBuffer, bytes) {
  if (!(bytes instanceof Uint8Array) || !bytes.length) {
    return Promise.reject(new Error("Video data is unavailable."));
  }
  const deferred = createDeferred();
  const onUpdateEnd = () => {
    cleanup();
    deferred.resolve();
  };
  const onError = () => {
    cleanup();
    deferred.reject(new Error("Video data is unavailable."));
  };
  const cleanup = () => {
    sourceBuffer.removeEventListener("updateend", onUpdateEnd);
    sourceBuffer.removeEventListener("error", onError);
  };
  sourceBuffer.addEventListener("updateend", onUpdateEnd, { once: true });
  sourceBuffer.addEventListener("error", onError, { once: true });
  try {
    sourceBuffer.appendBuffer(bytes);
  } catch (error) {
    cleanup();
    deferred.reject(error instanceof Error ? error : new Error("Video data is unavailable."));
  }
  return deferred.promise;
}

function responseFallback(op) {
  if (op === "open_viewer") return "Protected video is unavailable.";
  if (op === "read_viewer") return "Video data is unavailable.";
  return "Viewer session is unavailable.";
}

export function createPlayerController({
  documentObject = document,
  windowObject = window,
  locationObject = window.location,
  fetchImpl = fetch,
  mediaSourceClass = globalThis.MediaSource,
  urlObject = URL,
} = {}) {
  const video = documentObject.getElementById("player-video");
  const status = documentObject.getElementById("player-status");
  const overlay = documentObject.getElementById("player-overlay");
  const overlayText = documentObject.getElementById("player-overlay-text");
  const { mintId, homeToken } = readLaunchContext(locationObject);
  let session = null;
  let objectUrl = "";
  let closed = false;
  let closePromise = null;
  let failed = false;

  function setStatus(message, state = "info") {
    status.textContent = message;
    status.dataset.state = state;
    overlayText.textContent = message;
  }

  function showOverlay(message, state = "info") {
    setStatus(message, state);
    overlay.hidden = false;
  }

  function hideOverlay(message = "Ready") {
    status.textContent = message;
    status.dataset.state = "ready";
    overlay.hidden = true;
  }

  function clearVideo() {
    try {
      video.pause?.();
    } catch {}
    if (objectUrl) {
      urlObject.revokeObjectURL?.(objectUrl);
      objectUrl = "";
    }
    if (typeof video.removeAttribute === "function") {
      video.removeAttribute("src");
    } else {
      video.src = "";
    }
    video.load?.();
  }

  async function postProvider(op, body, options = {}) {
    if (!homeToken) {
      throw new Error("Protected video is unavailable.");
    }
    const response = await fetchImpl(`/api/provider/object/${op}`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify(body),
      ...(options.keepalive ? { keepalive: true } : {}),
    });
    return readProviderEnvelope(response, responseFallback(op));
  }

  async function closeViewer(options = {}) {
    if (closePromise || closed || !session) {
      return closePromise || Promise.resolve();
    }
    closed = true;
    closePromise = postProvider(
      "close_viewer",
      {
        mint_id: session.mintId,
        viewer_session_handle: session.viewerSessionHandle,
      },
      options,
    ).catch(() => {
      if (!options.quiet) {
        throw new Error("Viewer session is unavailable.");
      }
    });
    return closePromise;
  }

  async function fail(message) {
    if (failed) {
      return;
    }
    failed = true;
    clearVideo();
    showOverlay(message, "error");
    await closeViewer({ quiet: true });
  }

  async function readPart(segmentIndex = null) {
    const data = await postProvider("read_viewer", {
      mint_id: session.mintId,
      viewer_session_handle: session.viewerSessionHandle,
      ...(segmentIndex === null ? {} : { segment_index: segmentIndex }),
    });
    return parseViewerPartData(data, session.mintId, session.viewerSessionHandle);
  }

  async function startPlayback() {
    if (!MINT_ID_HEX_RE.test(mintId)) {
      showOverlay("Protected video is unavailable.", "error");
      return;
    }
    if (!mediaSourceClass) {
      showOverlay("This browser cannot play protected video.", "error");
      return;
    }
    showOverlay("Loading video...");
    try {
      session = parseViewerOpenData(await postProvider("open_viewer", { mint_id: mintId }), mintId);
      const mimeType = buildViewerMimeType(session.mimeType, session.codecs);
      if (!mediaSourceSupported(mediaSourceClass, mimeType)) {
        await fail("This browser cannot play protected video.");
        return;
      }
      const mediaSource = new mediaSourceClass();
      objectUrl = urlObject.createObjectURL(mediaSource);
      video.src = objectUrl;
      const sourceOpen = createDeferred();
      mediaSource.addEventListener("sourceopen", () => sourceOpen.resolve(), { once: true });
      await sourceOpen.promise;
      const sourceBuffer = mediaSource.addSourceBuffer(mimeType);
      await appendBytes(sourceBuffer, await readPart());
      await appendBytes(sourceBuffer, await readPart(0));
      hideOverlay("Playing");
      try {
        await video.play?.();
      } catch {}
      for (let segmentIndex = 1; segmentIndex < session.segmentCount; segmentIndex += 1) {
        await appendBytes(sourceBuffer, await readPart(segmentIndex));
      }
      mediaSource.endOfStream?.();
    } catch (error) {
      const message =
        error instanceof Error && error.message ? error.message : "Protected video is unavailable.";
      await fail(message);
    }
  }

  video.addEventListener("ended", () => {
    void closeViewer({ quiet: true });
  });
  video.addEventListener("error", () => {
    void fail("Playback failed.");
  });
  windowObject.addEventListener(
    "pagehide",
    () => {
      void closeViewer({ keepalive: true, quiet: true });
    },
    { once: true },
  );

  return {
    startPlayback,
    closeViewer,
    getSession() {
      return session;
    },
    getState() {
      return {
        closed,
        mintId,
      };
    },
  };
}

export function bootstrapPlayer() {
  const controller = createPlayerController();
  void controller.startPlayback();
  return controller;
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  bootstrapPlayer();
}
