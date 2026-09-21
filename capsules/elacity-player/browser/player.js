const VIEWER_OPEN_SCHEMA = "elastos.library.runtime-custody-viewer/v1";
const VIEWER_PART_SCHEMA = "elastos.library.runtime-custody-viewer-part/v1";
const VIEWER_HANDLE_HEX_RE = /^[0-9a-f]{64}$/;
const MINT_ID_HEX_RE = /^[0-9a-f]{64}$/;
export const MAX_VIEWER_SEGMENT_COUNT = 512;
export const MAX_VIEWER_MEDIA_PART_BYTES = 2 * 1024 * 1024;
const MAX_VIEWER_MEDIA_PART_BASE64_BYTES = Math.ceil(MAX_VIEWER_MEDIA_PART_BYTES / 3) * 4;
const VIEWER_CONTENT_KIND_MEDIA = "media";
const AUDIO_ONLY_CLASS = "audio-only";
// Codec families that paint a frame. A rendition whose declared codec list has
// one of these carries a picture the decoder draws for us; anything else (AAC,
// Opus, FLAC, ...) is sound only.
const IMAGE_TRACK_CODEC_PREFIXES = [
  "avc1.",
  "avc3.",
  "hev1.",
  "hvc1.",
  "hvc2.",
  "av01.",
  "vp08",
  "vp09",
  "vp8",
  "vp9",
  "mp4v.",
];
export const PRESENTATION_IMAGE_TRACK = "image-track";
export const PRESENTATION_POSTER = "poster";
export const PRESENTATION_CONTROLS_ONLY = "controls-only";
const MEDIA_UNAVAILABLE = "Protected media is unavailable.";
const NOTHING_CHOSEN = "Open something from your Library to play it here.";
const RETRY_LABEL = "Try again";

/** Wire identity of the state Runtime answers an unfinished open with. */
export const OPEN_PROGRESS_SCHEMA = "elastos.protected-content.open-progress/v1";

/**
 * The stages an open can report, and how this player waits through each.
 *
 * Opening protected media is a ceremony, not a request: the wallet holds a
 * rights-signature request that the person approves, and only then does the
 * release run. Every first open of every protected item reaches this state, so
 * a player that treated it as a failure -- which this one did -- turned the
 * ordinary case into a dead end.
 */
const RESUME_POLICY = new Map([
  ["rights_approval", { intervalMs: 2000, maxAttempts: 180 }],
  ["unavailable", { intervalMs: 3000, maxAttempts: 3 }],
]);

const OPEN_RESPONSE_KEYS = [
  "codecs",
  "content_kind",
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

/**
 * Reads the typed state Runtime attaches to an unfinished open.
 *
 * Returns `null` for anything this player does not recognise, so an answer of
 * an unexpected shape is treated as a plain failure rather than guessed at. A
 * stage with no resume policy is not resumable here whatever the `resumable`
 * flag says: the player has no rule for how to wait through it.
 */
export function readOpenProgress(payload) {
  const source = payload && typeof payload === "object" && !Array.isArray(payload) ? payload : null;
  const value = source?.open_progress;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  if (value.schema !== OPEN_PROGRESS_SCHEMA) return null;
  const stage = String(value.stage || "");
  if (!stage) return null;
  const connectorId = typeof value.connector_id === "string" ? value.connector_id.trim() : "";
  return {
    stage,
    resumable: value.resumable === true && RESUME_POLICY.has(stage),
    awaitsPerson: value.awaits_person === true,
    connectorId,
    expiresAt: Number.isInteger(value.expires_at) && value.expires_at > 0 ? value.expires_at : 0,
  };
}

/**
 * What to tell the person while an open waits.
 *
 * Composed here from the typed state rather than taken from the answer's own
 * sentence: that sentence is Runtime's stable wording for its operators, and it
 * names internal machinery a player should never put on screen.
 */
export function waitingMessage(progress) {
  if (progress?.stage !== "rights_approval") {
    return "This is not ready yet. Trying again...";
  }
  if (!progress.awaitsPerson) {
    return "Your wallet is approving this. It plays on its own.";
  }
  return progress.connectorId
    ? `Approve playing this in ${progress.connectorId}. It plays on its own once you do.`
    : "Approve playing this in your wallet. It plays on its own once you do.";
}

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

/**
 * A refusal, carrying the typed state that came with it.
 *
 * The state rides the error because every caller already handles a refusal;
 * what changes is that some refusals are a stage of a ceremony the player waits
 * through rather than the end of one.
 */
function providerError(message, progress) {
  const error = new Error(message);
  if (progress) error.openProgress = progress;
  return error;
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
  const progress = readOpenProgress(payload);
  if (!response?.ok) {
    const message =
      typeof payload?.message === "string" && payload.message.trim() ? payload.message : fallback;
    throw providerError(message, progress);
  }
  if (payload?.status === "error") {
    throw providerError(
      typeof payload?.message === "string" && payload.message.trim() ? payload.message : fallback,
      progress,
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
    // The player renders media sessions only: an object session's geometry is
    // not a segment ladder, so refuse it here rather than misread its fields.
    data.content_kind !== VIEWER_CONTENT_KIND_MEDIA ||
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
    throw new Error("Media data is unavailable.");
  }
  if (value.length > MAX_VIEWER_MEDIA_PART_BASE64_BYTES) {
    throw new Error("Media data is unavailable.");
  }
  try {
    if (typeof atob === "function") {
      const binary = atob(value);
      if (btoa(binary) !== value) {
        throw new Error("Media data is unavailable.");
      }
      const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
      if (bytes.length > MAX_VIEWER_MEDIA_PART_BYTES) {
        throw new Error("Media data is unavailable.");
      }
      return bytes;
    }
    const buffer = Buffer.from(value, "base64");
    if (buffer.toString("base64") !== value) {
      throw new Error("Media data is unavailable.");
    }
    if (buffer.length > MAX_VIEWER_MEDIA_PART_BYTES) {
      throw new Error("Media data is unavailable.");
    }
    return Uint8Array.from(buffer);
  } catch {
    throw new Error("Media data is unavailable.");
  }
}

export function parseViewerPartData(data, expectedMintId, expectedHandle) {
  if (!hasExactKeys(data, PART_RESPONSE_KEYS)) {
    throw new Error("Media data is unavailable.");
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
    throw new Error("Media data is unavailable.");
  }
  const bytes = decodeBase64(String(data.data || ""));
  if (!bytes.length) {
    throw new Error("Media data is unavailable.");
  }
  return bytes;
}

/**
 * Whether the rendition carries an image track of its own. Read from the
 * declared codec list rather than the container type, so a rendition that
 * carries a picture is recognised whichever container it arrives in.
 */
export function renditionHasImageTrack(codecs) {
  return String(codecs || "")
    .split(",")
    .map((codec) => codec.trim().toLowerCase())
    .some((codec) => IMAGE_TRACK_CODEC_PREFIXES.some((prefix) => codec.startsWith(prefix)));
}

/**
 * How to present a session, highest precedence first:
 *
 *   1. the rendition's own image track - the decoder paints the frame;
 *   2. a poster set on the element - shown in the frame instead;
 *   3. neither - the frame collapses to the height of its own controls.
 *
 * Nothing is substituted at step 3: with no image track and no poster the
 * player shows no picture at all rather than standing in for one.
 */
export function presentationFor(session, poster) {
  if (renditionHasImageTrack(session?.codecs)) {
    return PRESENTATION_IMAGE_TRACK;
  }
  return String(poster || "").trim() ? PRESENTATION_POSTER : PRESENTATION_CONTROLS_ONLY;
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
    return Promise.reject(new Error("Media data is unavailable."));
  }
  const deferred = createDeferred();
  const onUpdateEnd = () => {
    cleanup();
    deferred.resolve();
  };
  const onError = () => {
    cleanup();
    deferred.reject(new Error("Media data is unavailable."));
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
    deferred.reject(error instanceof Error ? error : new Error("Media data is unavailable."));
  }
  return deferred.promise;
}

function responseFallback(op) {
  if (op === "open_viewer") return MEDIA_UNAVAILABLE;
  if (op === "read_viewer") return "Media data is unavailable.";
  return "Viewer session is unavailable.";
}

export function createPlayerController({
  documentObject = document,
  windowObject = window,
  locationObject = window.location,
  fetchImpl = fetch,
  mediaSourceClass = globalThis.MediaSource,
  urlObject = URL,
  // The clock and the timer are taken as inputs so the waiting can be driven
  // in a test without waiting through it.
  nowSeconds = () => Math.floor(Date.now() / 1000),
  setTimeoutImpl = (handler, delay) => setTimeout(handler, delay),
  clearTimeoutImpl = (handle) => clearTimeout(handle),
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
  // The ceremony's own state. `attempts` counts asks of one waiting stage, so
  // moving from one stage to another starts its bound afresh.
  let resumeTimer = null;
  let resumeStage = "";
  let attempts = 0;
  let disposed = false;
  let retryButton = null;
  let mediaSource = null;

  function setStatus(message, state = "info") {
    status.textContent = message;
    status.dataset.state = state;
    overlayText.textContent = message;
  }

  function clearRetry() {
    if (retryButton && typeof overlay.removeChild === "function") {
      try {
        overlay.removeChild(retryButton);
      } catch {}
    }
    retryButton = null;
  }

  function showOverlay(message, state = "info", action = null) {
    setStatus(message, state);
    overlay.hidden = false;
    clearRetry();
    if (!action) return;
    const button = documentObject.createElement("button");
    button.type = "button";
    button.className = "player-action";
    button.textContent = action.label;
    button.addEventListener("click", action.run);
    overlay.appendChild(button);
    retryButton = button;
    // Focus follows the state that changed, so a person playing by keyboard
    // reaches the one control this overlay offers.
    try {
      button.focus?.();
    } catch {}
  }

  /** Stops the waiting. Called wherever the player stops caring about it. */
  function cancelResume() {
    if (resumeTimer !== null) {
      clearTimeoutImpl(resumeTimer);
      resumeTimer = null;
    }
  }

  function hideOverlay(message = "Ready") {
    status.textContent = message;
    status.dataset.state = "ready";
    overlay.hidden = true;
  }

  function clearMedia() {
    try {
      video.pause?.();
    } catch {}
    // An open MediaSource holds its buffers until it is ended. Giving it back
    // before the object URL is revoked means neither outlives this page.
    if (mediaSource) {
      try {
        if (mediaSource.readyState === "open") mediaSource.endOfStream?.();
      } catch {}
      mediaSource = null;
    }
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
      throw new Error(MEDIA_UNAVAILABLE);
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

  /**
   * The end of this attempt.
   *
   * `retry` is offered only where asking again could answer differently. A
   * refusal Runtime called final gets no button, because a button that cannot
   * work invites the person to keep trying something already decided.
   */
  async function fail(message, { retry = false } = {}) {
    // A page that has gone away has nothing to show and nothing to close. The
    // video element fires an error as its MediaSource is torn down, so without
    // this the overlay would be repainted on a page nobody is looking at.
    if (failed || disposed) {
      return;
    }
    failed = true;
    cancelResume();
    clearMedia();
    showOverlay(message, "error", retry ? { label: RETRY_LABEL, run: () => void restart() } : null);
    await closeViewer({ quiet: true });
  }

  /** An explicit retry after a terminal failure, asked for by the person. */
  async function restart() {
    if (disposed) return;
    failed = false;
    closed = false;
    closePromise = null;
    session = null;
    attempts = 0;
    resumeStage = "";
    clearRetry();
    await attemptPlayback();
  }

  /**
   * Waits, then asks again with the identical open.
   *
   * The same request is re-issued rather than a new one built: Runtime resumes
   * the attempt it already has, against the recipient the decrypt provider
   * still holds, so asking again is how the person's approval is collected --
   * not a second release.
   */
  function scheduleResume(progress) {
    if (disposed) return false;
    if (progress.stage !== resumeStage) {
      resumeStage = progress.stage;
      attempts = 0;
    }
    const policy = RESUME_POLICY.get(progress.stage);
    if (!policy) return false;
    attempts += 1;
    if (attempts > policy.maxAttempts) return false;
    // Runtime says when the wallet request lapses. Stopping with it means the
    // player stops asking at the moment there is nothing left to answer.
    if (progress.expiresAt && nowSeconds() >= progress.expiresAt) return false;
    showOverlay(waitingMessage(progress));
    cancelResume();
    resumeTimer = setTimeoutImpl(() => {
      resumeTimer = null;
      void attemptPlayback();
    }, policy.intervalMs);
    return true;
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
    // A launch with nothing chosen is not a failure, so it is said once and
    // plainly: it names where media comes from instead of reporting that
    // something is unavailable, which would describe an item never named.
    if (!mintId) {
      showOverlay(NOTHING_CHOSEN);
      return;
    }
    if (!MINT_ID_HEX_RE.test(mintId)) {
      showOverlay(MEDIA_UNAVAILABLE, "error");
      return;
    }
    if (!mediaSourceClass) {
      showOverlay("This browser cannot play protected media.", "error");
      return;
    }
    await attemptPlayback();
  }

  async function attemptPlayback() {
    if (disposed) return;
    showOverlay("Loading media...");
    try {
      session = parseViewerOpenData(await postProvider("open_viewer", { mint_id: mintId }), mintId);
      // A page that went away while this open was in flight still opened a
      // session, so it is handed back rather than left to expire on its own.
      if (disposed) {
        await closeViewer({ keepalive: true, quiet: true });
        return;
      }
      // The ceremony finished, so nothing is waiting on it any more.
      cancelResume();
      resumeStage = "";
      attempts = 0;
      const mimeType = buildViewerMimeType(session.mimeType, session.codecs);
      if (!mediaSourceSupported(mediaSourceClass, mimeType)) {
        await fail("This browser cannot play protected media.");
        return;
      }
      // Only a rendition with neither an image track nor a poster collapses to
      // the height of its own controls; either kind of picture keeps the frame.
      const presentation = presentationFor(session, video.getAttribute?.("poster") ?? video.poster);
      video.classList?.toggle?.(AUDIO_ONLY_CLASS, presentation === PRESENTATION_CONTROLS_ONLY);
      mediaSource = new mediaSourceClass();
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
        // A page that closed part way through stops reading rather than
        // appending into a source nobody is watching.
        if (disposed || closed) return;
        await appendBytes(sourceBuffer, await readPart(segmentIndex));
      }
      if (mediaSource) mediaSource.endOfStream?.();
    } catch (error) {
      const progress = error?.openProgress ?? null;
      // A stage the player knows how to wait through is waited through. Only
      // when the waiting itself is over does this become a failure, and only
      // then is the person offered the choice to ask again.
      if (progress?.resumable && scheduleResume(progress)) return;
      const message = error instanceof Error && error.message ? error.message : MEDIA_UNAVAILABLE;
      await fail(message, { retry: Boolean(progress?.resumable) });
    }
  }

  /**
   * Gives back everything this page is holding.
   *
   * Reached from the page going away and from nothing else, so it is the one
   * place the timer, the MediaSource, the object URL and the session are
   * released together.
   */
  function dispose() {
    if (disposed) return;
    disposed = true;
    cancelResume();
    clearMedia();
    void closeViewer({ keepalive: true, quiet: true });
  }

  video.addEventListener("ended", () => {
    void closeViewer({ quiet: true });
  });
  video.addEventListener("error", () => {
    void fail("Playback failed.");
  });
  windowObject.addEventListener("pagehide", dispose, { once: true });

  return {
    startPlayback,
    closeViewer,
    dispose,
    getSession() {
      return session;
    },
    getState() {
      return {
        closed,
        failed,
        mintId,
        waiting: resumeTimer !== null,
        waitingStage: resumeStage,
        attempts,
        disposed,
        retryOffered: retryButton !== null,
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
