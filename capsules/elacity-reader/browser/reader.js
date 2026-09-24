// Opens one protected file, reads it part by part, and hands the whole of it
// to the renderer for its kind.
//
// The session is the same three steps the video viewer uses: open, read, close.
// What differs is the geometry — a file arrives as a fixed number of equal
// parts and is reassembled into one run of bytes before anything is drawn —
// and the fail-closed check at the top: a session that is not a file session
// is refused outright rather than read with the wrong idea of its shape.

import { describeKind, kindFor, rendererFor } from "./render.js";

const VIEWER_OPEN_SCHEMA = "elastos.library.runtime-custody-viewer/v1";
const VIEWER_PART_SCHEMA = "elastos.library.runtime-custody-viewer-part/v1";
const VIEWER_HANDLE_HEX_RE = /^[0-9a-f]{64}$/;
const MINT_ID_HEX_RE = /^[0-9a-f]{64}$/;
const VIEWER_CONTENT_KIND_OBJECT = "object";

/** Largest file this reader will hold in one piece. */
export const MAX_OBJECT_BYTES = 64 * 1024 * 1024;
/** Most parts a file can arrive in. */
export const MAX_OBJECT_CHUNK_COUNT = 64;
/** Plaintext bytes every part but the last one carries. */
export const OBJECT_CHUNK_BYTES = 1024 * 1024;
const MAX_CHUNK_BASE64_BYTES = Math.ceil(OBJECT_CHUNK_BYTES / 3) * 4;

/** Longest a declared content type may be before it is refused. */
const MAX_CONTENT_TYPE_LENGTH = 255;
const CONTENT_TYPE_RE = /^[\x21-\x7e]+$/;

const OPEN_RESPONSE_KEYS = [
  "chunk_count",
  "chunk_plaintext_bytes",
  "content_kind",
  "content_type",
  "expires_at",
  "mint_id",
  "plaintext_bytes",
  "schema",
  "viewer_session_handle",
];
const PART_RESPONSE_KEYS = [
  "chunk_index",
  "content_kind",
  "data",
  "encoding",
  "mint_id",
  "schema",
  "viewer_session_handle",
];

/**
 * Field names that must never reach this page. Nothing here needs a key to do
 * its job: bytes arrive already readable. If one of these ever turned up in a
 * reply it would mean something upstream had changed shape, and the reader
 * stops rather than carrying it any further.
 */
const FORBIDDEN_FIELDS = new Set(["cek", "content_key", "iv", "share"]);
const MAX_SCAN_NODES = 10000;

const FILE_UNAVAILABLE = "This file is unavailable.";
const PART_UNAVAILABLE = "Part of this file is unavailable.";
const SESSION_UNAVAILABLE = "This file could not be closed.";
const WRONG_KIND = "This file plays in Elacity Player.";
const TOO_LARGE = "This file is too large for this reader to open.";
const NOTHING_CHOSEN = "Open a file from your Library to read it here.";
const RETRY_LABEL = "Try again";

/** Wire identity of the state Runtime answers an unfinished open with. */
export const OPEN_PROGRESS_SCHEMA = "elastos.protected-content.open-progress/v1";

/**
 * The stages an open can report, and how this reader waits through each.
 *
 * Opening a protected file is a ceremony, not a request: the wallet holds a
 * rights-signature request that the person approves, and only then does the
 * release run. Every first open of every protected item reaches this state, so
 * a reader that treated it as a failure -- which this one did -- turned the
 * ordinary case into a dead end.
 *
 * `intervalMs` is how long to wait before asking again, and `maxAttempts`
 * bounds the asking. A rights approval is bounded by the window Runtime sends
 * in `expires_at` as well, so the reader stops when the wallet request does
 * rather than at a number of its own choosing.
 */
const RESUME_POLICY = new Map([
  ["rights_approval", { intervalMs: 2000, maxAttempts: 180 }],
  ["unavailable", { intervalMs: 3000, maxAttempts: 3 }],
]);

/**
 * Refuses any reply that carries key material.
 *
 * Ported from the reader this one replaces. It is a standing check, not a
 * reaction to a known failure: it costs one walk of a small reply and it means
 * a change upstream can never quietly turn this page into a place key material
 * ends up.
 */
export function assertNoKeyMaterial(value) {
  const stack = [value];
  let seen = 0;
  while (stack.length) {
    const node = stack.pop();
    if (!node || typeof node !== "object") continue;
    seen += 1;
    if (seen > MAX_SCAN_NODES) throw new Error(FILE_UNAVAILABLE);
    if (Array.isArray(node)) {
      for (const item of node) stack.push(item);
      continue;
    }
    for (const [key, item] of Object.entries(node)) {
      if (FORBIDDEN_FIELDS.has(key.toLowerCase())) {
        throw new Error(FILE_UNAVAILABLE);
      }
      stack.push(item);
    }
  }
  return value;
}

/**
 * Reads the typed state Runtime attaches to an unfinished open.
 *
 * Returns `null` for anything this reader does not recognise, so an answer of
 * an unexpected shape is treated as a plain failure rather than guessed at. In
 * particular, a stage with no resume policy is not resumable here whatever the
 * `resumable` flag says: the reader has no rule for how to wait through it.
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
 * names internal machinery a reader should never put on screen.
 */
export function waitingMessage(progress) {
  if (progress?.stage !== "rights_approval") {
    return "This file is not ready yet. Trying again...";
  }
  if (!progress.awaitsPerson) {
    return "Your wallet is approving this file. It opens on its own.";
  }
  return progress.connectorId
    ? `Approve opening this file in ${progress.connectorId}. It opens on its own once you do.`
    : "Approve opening this file in your wallet. It opens on its own once you do.";
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
    throw new Error(FILE_UNAVAILABLE);
  }
  return value;
}

/**
 * A refusal, carrying the typed state that came with it.
 *
 * The state rides the error rather than a return value because every caller
 * already handles a refusal; what changes is that some refusals are a stage of
 * a ceremony the reader waits through rather than the end of one.
 */
function providerError(message, progress) {
  const error = new Error(message);
  if (progress) error.openProgress = progress;
  return error;
}

async function readProviderEnvelope(response, fallback) {
  const text = typeof response?.text === "function" ? await response.text() : "";
  let payload = null;
  if (text) {
    try {
      payload = parseJsonObject(text);
    } catch {
      throw new Error(fallback);
    }
  }
  if (payload) assertNoKeyMaterial(payload);
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

/**
 * The two things a session can be given back with, when a reply carries them
 * in a shape this reader can trust.
 *
 * Read before the reply is judged on anything else. Opening a session costs
 * something whether or not this reader goes on to read it, so a reply this
 * reader refuses is still one it can hand back, rather than one left standing
 * until it expires on its own.
 */
export function closableSession(data, expectedMintId) {
  const object = data && typeof data === "object" && !Array.isArray(data) ? data : null;
  if (!object) return null;
  const mintId = String(object.mint_id || "");
  const viewerSessionHandle = String(object.viewer_session_handle || "");
  if (
    mintId !== expectedMintId ||
    !MINT_ID_HEX_RE.test(mintId) ||
    !VIEWER_HANDLE_HEX_RE.test(viewerSessionHandle)
  ) {
    return null;
  }
  return { mintId, viewerSessionHandle };
}

/**
 * Reads the reply that opens a file session.
 *
 * Every field the reply declares is checked, including the two that describe
 * how the file is cut up: the part size must be the one this reader is built
 * around, and the number of parts must be exactly the number that size implies
 * for that many bytes. A geometry that does not add up is refused rather than
 * read around.
 */
export function parseViewerOpenData(data, expectedMintId) {
  if (!hasExactKeys(data, OPEN_RESPONSE_KEYS)) {
    throw new Error(FILE_UNAVAILABLE);
  }
  const mintId = String(data.mint_id || "");
  const viewerSessionHandle = String(data.viewer_session_handle || "");
  const contentType = String(data.content_type || "");
  // The counts are taken exactly as they arrive, never converted first. A
  // count written as words is a reply of a shape this reader does not know,
  // and reading it as the number it looks like would be accepting a shape on
  // the strength of a guess. `Number.isInteger` below refuses anything that is
  // not already a whole number.
  const plaintextBytes = data.plaintext_bytes;
  const chunkCount = data.chunk_count;
  const chunkBytes = data.chunk_plaintext_bytes;
  const expiresAt = data.expires_at;
  if (
    data.schema !== VIEWER_OPEN_SCHEMA ||
    mintId !== expectedMintId ||
    !MINT_ID_HEX_RE.test(mintId) ||
    !VIEWER_HANDLE_HEX_RE.test(viewerSessionHandle) ||
    !Number.isInteger(expiresAt) ||
    expiresAt <= 0
  ) {
    throw new Error(FILE_UNAVAILABLE);
  }
  // The kind check comes before every shortcut below. A media session's
  // geometry is a segment ladder, not a run of equal parts, and reading one
  // through this path would mean reading fields that are not there.
  if (data.content_kind !== VIEWER_CONTENT_KIND_OBJECT) {
    throw new Error(WRONG_KIND);
  }
  if (
    !contentType ||
    contentType.length > MAX_CONTENT_TYPE_LENGTH ||
    !CONTENT_TYPE_RE.test(contentType)
  ) {
    throw new Error(FILE_UNAVAILABLE);
  }
  if (
    !Number.isInteger(chunkBytes) ||
    chunkBytes !== OBJECT_CHUNK_BYTES ||
    !Number.isInteger(plaintextBytes) ||
    plaintextBytes <= 0 ||
    !Number.isInteger(chunkCount) ||
    chunkCount <= 0
  ) {
    throw new Error(FILE_UNAVAILABLE);
  }
  // The size cap is checked before the part count. A file over the cap also
  // has more parts than the cap allows, and the count is not the reason a
  // reader could act on: the size is, so it is the one that gets said.
  if (plaintextBytes > MAX_OBJECT_BYTES) {
    throw new Error(TOO_LARGE);
  }
  if (chunkCount > MAX_OBJECT_CHUNK_COUNT || chunkCount !== Math.ceil(plaintextBytes / chunkBytes)) {
    throw new Error(FILE_UNAVAILABLE);
  }
  return {
    mintId,
    viewerSessionHandle,
    contentType,
    plaintextBytes,
    chunkCount,
    chunkBytes,
  };
}

function decodeBase64(base64Text, expectedBytes) {
  const value = String(base64Text || "");
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new Error(PART_UNAVAILABLE);
  }
  if (value.length > MAX_CHUNK_BASE64_BYTES) {
    throw new Error(PART_UNAVAILABLE);
  }
  let bytes = null;
  try {
    if (typeof atob === "function") {
      const binary = atob(value);
      if (btoa(binary) !== value) throw new Error(PART_UNAVAILABLE);
      bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    } else {
      const buffer = Buffer.from(value, "base64");
      if (buffer.toString("base64") !== value) throw new Error(PART_UNAVAILABLE);
      bytes = Uint8Array.from(buffer);
    }
  } catch {
    throw new Error(PART_UNAVAILABLE);
  }
  if (bytes.length !== expectedBytes) {
    throw new Error(PART_UNAVAILABLE);
  }
  return bytes;
}

/**
 * Reads one part of a file. The part's own index and the number of bytes it
 * should carry are both known before the reply arrives, and both are checked
 * against it: a part that arrives out of order, or short, is refused.
 */
export function parseViewerPartData(data, expected) {
  if (!hasExactKeys(data, PART_RESPONSE_KEYS)) {
    throw new Error(PART_UNAVAILABLE);
  }
  const mintId = String(data.mint_id || "");
  const viewerSessionHandle = String(data.viewer_session_handle || "");
  if (
    data.schema !== VIEWER_PART_SCHEMA ||
    data.content_kind !== VIEWER_CONTENT_KIND_OBJECT ||
    mintId !== expected.mintId ||
    viewerSessionHandle !== expected.viewerSessionHandle ||
    data.encoding !== "base64" ||
    data.chunk_index !== expected.chunkIndex ||
    !MINT_ID_HEX_RE.test(mintId) ||
    !VIEWER_HANDLE_HEX_RE.test(viewerSessionHandle)
  ) {
    throw new Error(PART_UNAVAILABLE);
  }
  return decodeBase64(String(data.data || ""), expected.chunkBytes);
}

/** How many plaintext bytes part `index` of this file carries. */
export function chunkByteLength(session, index) {
  const start = index * session.chunkBytes;
  return Math.min(session.chunkBytes, session.plaintextBytes - start);
}

function responseFallback(op) {
  if (op === "open_viewer") return FILE_UNAVAILABLE;
  if (op === "read_viewer") return PART_UNAVAILABLE;
  return SESSION_UNAVAILABLE;
}

export function createReaderController({
  documentObject = document,
  windowObject = window,
  locationObject = window.location,
  fetchImpl = fetch,
  urlObject = URL,
  render = rendererFor,
  // The clock and the timer are taken as inputs so the waiting can be driven
  // in a test without waiting through it.
  nowSeconds = () => Math.floor(Date.now() / 1000),
  setTimeoutImpl = (handler, delay) => setTimeout(handler, delay),
  clearTimeoutImpl = (handle) => clearTimeout(handle),
} = {}) {
  const stage = documentObject.getElementById("reader-stage");
  const status = documentObject.getElementById("reader-status");
  const kindLabel = documentObject.getElementById("reader-kind");
  const controls = documentObject.getElementById("reader-controls");
  const previousButton = documentObject.getElementById("reader-previous");
  const nextButton = documentObject.getElementById("reader-next");
  const position = documentObject.getElementById("reader-position");
  const { mintId, homeToken } = readLaunchContext(locationObject);
  let session = null;
  // What was opened, whether or not this reader went on to accept it.
  let opened = null;
  let view = null;
  let pager = null;
  let index = 0;
  let closed = false;
  let closePromise = null;
  let failed = false;
  // The ceremony's own state. `attempts` counts asks of one waiting stage, so
  // moving from one stage to another starts its bound afresh rather than
  // inheriting a count from the stage before it.
  let resumeTimer = null;
  let resumeStage = "";
  let attempts = 0;
  let disposed = false;
  let retryButton = null;

  function setStatus(message, state = "info") {
    status.textContent = message;
    status.dataset.state = state;
    status.hidden = message === "";
  }

  function releaseView() {
    try {
      view?.release?.();
    } catch {}
    view = null;
    pager = null;
    controls.hidden = true;
  }

  /** Stops the waiting. Called wherever the reader stops caring about it. */
  function cancelResume() {
    if (resumeTimer !== null) {
      clearTimeoutImpl(resumeTimer);
      resumeTimer = null;
    }
  }

  function showEmpty(message, action = null) {
    releaseView();
    retryButton = null;
    while (stage.firstChild) stage.removeChild(stage.firstChild);
    const panel = documentObject.createElement("div");
    panel.className = "view view-empty";
    const line = documentObject.createElement("p");
    line.textContent = message;
    panel.appendChild(line);
    if (action) {
      const button = documentObject.createElement("button");
      button.type = "button";
      button.className = "reader-action";
      button.textContent = action.label;
      button.addEventListener("click", action.run);
      panel.appendChild(button);
      retryButton = button;
    }
    stage.appendChild(panel);
    // Focus follows the state that changed, so a person reading by keyboard
    // reaches the one control this panel offers without hunting for it.
    try {
      retryButton?.focus?.();
    } catch {}
  }

  function paintPosition() {
    position.textContent = `${pager.unit} ${index + 1} of ${pager.count}`;
    previousButton.disabled = index === 0;
    nextButton.disabled = index + 1 >= pager.count;
  }

  async function goTo(target) {
    if (!pager || target < 0 || target >= pager.count) return;
    previousButton.disabled = true;
    nextButton.disabled = true;
    try {
      await pager.show(target);
      index = target;
      paintPosition();
      setStatus("");
    } catch (error) {
      await fail(messageOf(error, PART_UNAVAILABLE));
    }
  }

  function messageOf(error, fallback) {
    return error instanceof Error && error.message ? error.message : fallback;
  }

  async function postProvider(op, body, options = {}) {
    if (!homeToken) {
      throw new Error(responseFallback(op));
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
    if (closePromise || closed || !opened) {
      return closePromise || Promise.resolve();
    }
    closed = true;
    closePromise = postProvider(
      "close_viewer",
      {
        mint_id: opened.mintId,
        viewer_session_handle: opened.viewerSessionHandle,
      },
      options,
    ).catch(() => {
      if (!options.quiet) {
        throw new Error(SESSION_UNAVAILABLE);
      }
    });
    return closePromise;
  }

  /**
   * The end of this attempt.
   *
   * `retry` is offered only where asking again could answer differently. A
   * refusal Runtime called final gets no button, because a button that cannot
   * work is worse than none: it invites the person to keep trying something
   * that has already been decided.
   */
  async function fail(message, { retry = false } = {}) {
    if (failed) return;
    failed = true;
    cancelResume();
    showEmpty(message, retry ? { label: RETRY_LABEL, run: () => void restart() } : null);
    setStatus(message, "error");
    await closeViewer({ quiet: true });
  }

  /** An explicit retry after a terminal failure, asked for by the person. */
  async function restart() {
    if (disposed) return;
    failed = false;
    closed = false;
    closePromise = null;
    opened = null;
    session = null;
    attempts = 0;
    resumeStage = "";
    retryButton = null;
    await attemptOpen();
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
    // reader stops asking at the moment there is nothing left to answer.
    if (progress.expiresAt && nowSeconds() >= progress.expiresAt) return false;
    setStatus(waitingMessage(progress));
    showEmpty(waitingMessage(progress));
    cancelResume();
    resumeTimer = setTimeoutImpl(() => {
      resumeTimer = null;
      void attemptOpen();
    }, policy.intervalMs);
    return true;
  }

  async function readAllBytes() {
    const bytes = new Uint8Array(session.plaintextBytes);
    let at = 0;
    for (let chunkIndex = 0; chunkIndex < session.chunkCount; chunkIndex += 1) {
      setStatus(`Reading part ${chunkIndex + 1} of ${session.chunkCount}...`);
      const data = await postProvider("read_viewer", {
        mint_id: session.mintId,
        viewer_session_handle: session.viewerSessionHandle,
        chunk_index: chunkIndex,
      });
      const part = parseViewerPartData(data, {
        mintId: session.mintId,
        viewerSessionHandle: session.viewerSessionHandle,
        chunkIndex,
        chunkBytes: chunkByteLength(session, chunkIndex),
      });
      bytes.set(part, at);
      at += part.length;
    }
    if (at !== session.plaintextBytes) {
      throw new Error(PART_UNAVAILABLE);
    }
    return bytes;
  }

  async function open() {
    // A launch with nothing chosen is not a failure, so it is said once and
    // plainly: it names where files come from instead of reporting that one is
    // unavailable, which would describe a file that was never named.
    if (!mintId) {
      showEmpty(NOTHING_CHOSEN);
      setStatus(NOTHING_CHOSEN);
      return;
    }
    if (!MINT_ID_HEX_RE.test(mintId)) {
      showEmpty(FILE_UNAVAILABLE);
      setStatus(FILE_UNAVAILABLE, "error");
      return;
    }
    await attemptOpen();
  }

  async function attemptOpen() {
    if (disposed) return;
    setStatus("Opening the file...");
    try {
      const reply = await postProvider("open_viewer", { mint_id: mintId });
      // Noted before the reply is judged: a refusal below still closes the
      // session the reply opened.
      opened = closableSession(reply, mintId);
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
      session = parseViewerOpenData(reply, mintId);
      const kind = kindFor(session.contentType);
      kindLabel.textContent = describeKind(kind);
      const bytes = await readAllBytes();
      setStatus("Preparing the view...");
      view = await render(session.contentType)({
        stage,
        bytes,
        contentType: session.contentType,
        urlObject,
        setStatus: (message) => setStatus(message),
      });
      if (view.pager) {
        pager = view.pager;
        index = 0;
        controls.hidden = false;
        // A right-to-left book turns its own way: Previous and Next swap
        // sides so Next always points the way the book goes on.
        controls.dir = pager.rightToLeft ? "rtl" : "ltr";
        await pager.show(0);
        paintPosition();
      }
      setStatus("");
    } catch (error) {
      const progress = error?.openProgress ?? null;
      // A stage the reader knows how to wait through is waited through. Only
      // when the waiting itself is over does this become a failure, and only
      // then is the person offered the choice to ask again.
      if (progress?.resumable && scheduleResume(progress)) return;
      await fail(messageOf(error, FILE_UNAVAILABLE), {
        retry: Boolean(progress?.resumable),
      });
    }
  }

  /**
   * Gives back everything this page is holding.
   *
   * Reached from the page going away and from nothing else, so it is the one
   * place the timer, the renderer's resources and the session are released
   * together.
   */
  function dispose() {
    if (disposed) return;
    disposed = true;
    cancelResume();
    releaseView();
    void closeViewer({ keepalive: true, quiet: true });
  }

  previousButton.addEventListener("click", () => {
    void goTo(index - 1);
  });
  nextButton.addEventListener("click", () => {
    void goTo(index + 1);
  });
  windowObject.addEventListener("keydown", (event) => {
    if (!pager || event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    const back = pager.rightToLeft ? "ArrowRight" : "ArrowLeft";
    const on = pager.rightToLeft ? "ArrowLeft" : "ArrowRight";
    if (event.key === back) void goTo(index - 1);
    else if (event.key === on) void goTo(index + 1);
  });
  windowObject.addEventListener("pagehide", dispose, { once: true });

  return {
    open,
    closeViewer,
    dispose,
    getSession() {
      return session;
    },
    getState() {
      return {
        closed,
        failed,
        index,
        mintId,
        pageCount: pager?.count ?? 0,
        waiting: resumeTimer !== null,
        waitingStage: resumeStage,
        attempts,
        disposed,
        retryOffered: retryButton !== null,
      };
    },
  };
}

export function bootstrapReader() {
  const controller = createReaderController();
  void controller.open();
  return controller;
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  bootstrapReader();
}
