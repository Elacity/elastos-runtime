// elacity-player — protected media viewer (satisfies elastos.viewer/media@1).
//
// Containment contract (mirrors PC2's elacity-player + the runtime decrypt
// boundary): this capsule plays an owned, protected fMP4/CENC asset by feeding
// ALREADY-DECRYPTED segments into Media Source Extensions. It receives bytes
// only through an opaque per-open SESSION handle and the runtime's scoped media
// route. It never sees — and never asks for — the CEK, the IV, the key-release
// receipt, or any KMS/wallet/chain material. If the runtime ever returned key
// material on these routes it would be a contract violation; we additionally
// assert the play-manifest carries no key fields and fail closed if it does.
//
// Scoped stream contract served by elastos-server (B2):
//   GET /api/viewers/elacity-player/media/{session}
//        -> { schema:"elastos.viewer.media/v1", mime, segment_count, has_init,
//             is_protected, expires_at }                 (metadata only, no key)
//   GET /api/viewers/elacity-player/media/{session}/init
//        -> decrypted init segment bytes (octet-stream, no-store)
//   GET /api/viewers/elacity-player/media/{session}/segment/{index}
//        -> decrypted media segment bytes, in-range only; out-of-range / expired
//           / unauthorized => 4xx (fail closed)
//
// Auth: the Home launch token rides in the `x-elastos-home-token` header, exactly
// like the other viewer capsules.

const video = document.getElementById("video");
const overlay = document.getElementById("overlay");
const overlayText = document.getElementById("overlay-text");
const statusEl = document.getElementById("status");

const VIEWER_ID = "elacity-player";
const MEDIA_SCHEMA = "elastos.viewer.media/v1";
// Fields that must NEVER appear in anything this viewer receives. Their presence
// means a broken boundary, so we refuse to play.
const FORBIDDEN_KEY_FIELDS = [
  "cek",
  "raw_cek",
  "iv",
  "key",
  "wrapped_cek",
  "sealed_cek",
  "release_receipt",
  "kms",
  "wallet",
  "chain_rpc",
];

function query(name) {
  try {
    return new URL(window.location.href).searchParams.get(name);
  } catch (_error) {
    return null;
  }
}

const homeToken = query("home_token");
const sessionId = query("session");

function launchHeaders() {
  return homeToken ? { "x-elastos-home-token": homeToken } : {};
}

function setOverlay(text, isError) {
  if (text === null) {
    overlay.hidden = true;
    return;
  }
  overlay.hidden = false;
  overlayText.textContent = text;
  overlay.classList.toggle("error", Boolean(isError));
}

function setStatus(text, isError) {
  if (!statusEl) return;
  statusEl.textContent = text || "";
  statusEl.hidden = !text;
  statusEl.classList.toggle("error", Boolean(isError));
}

function failClosed(message) {
  // No partial playback, no key material, no retries that could leak state.
  try {
    video.removeAttribute("src");
    video.load();
  } catch (_error) {
    /* best effort teardown */
  }
  setOverlay(message, true);
  setStatus(message, true);
  console.error("[elacity-player] fail closed:", message);
}

function mediaUrl(suffix) {
  const base = "/api/viewers/" + encodeURIComponent(VIEWER_ID)
    + "/media/" + encodeURIComponent(sessionId);
  return suffix ? base + suffix : base;
}

function assertNoKeyMaterial(manifest) {
  // Defense in depth: the runtime must not put key material on this route. If a
  // future regression did, refuse rather than risk surfacing it.
  const lowered = JSON.stringify(manifest).toLowerCase();
  for (const field of FORBIDDEN_KEY_FIELDS) {
    if (Object.prototype.hasOwnProperty.call(manifest, field)) {
      throw new Error("play manifest carried a forbidden key field: " + field);
    }
  }
  // Also catch nested raw CEK-looking material defensively.
  if (lowered.includes("\"raw_cek\"") || lowered.includes("\"wrapped_cek\"")) {
    throw new Error("play manifest carried nested key material");
  }
}

async function fetchPlayManifest() {
  const resp = await fetch(mediaUrl(""), { headers: { ...launchHeaders() } });
  if (!resp.ok) {
    throw new Error("media session unavailable: " + resp.status);
  }
  const manifest = await resp.json();
  if (manifest.schema !== MEDIA_SCHEMA) {
    throw new Error("unexpected media manifest schema: " + manifest.schema);
  }
  assertNoKeyMaterial(manifest);
  if (typeof manifest.mime !== "string" || manifest.mime.length === 0) {
    throw new Error("media manifest missing mime/codecs");
  }
  if (!Number.isInteger(manifest.segment_count) || manifest.segment_count < 0) {
    throw new Error("media manifest has invalid segment_count");
  }
  return manifest;
}

async function fetchSegmentBytes(suffix) {
  const resp = await fetch(mediaUrl(suffix), { headers: { ...launchHeaders() } });
  if (!resp.ok) {
    // Out-of-range / expired / substituted-and-rejected all land here.
    throw new Error("segment fetch failed (" + suffix + "): " + resp.status);
  }
  return new Uint8Array(await resp.arrayBuffer());
}

// Serialize appendBuffer calls: MSE requires waiting for `updateend` between
// appends. This also guarantees strict in-ORDER delivery into the SourceBuffer.
function appendChunk(sourceBuffer, bytes) {
  return new Promise((resolve, reject) => {
    const onUpdateEnd = () => {
      sourceBuffer.removeEventListener("updateend", onUpdateEnd);
      sourceBuffer.removeEventListener("error", onError);
      resolve();
    };
    const onError = () => {
      sourceBuffer.removeEventListener("updateend", onUpdateEnd);
      sourceBuffer.removeEventListener("error", onError);
      reject(new Error("SourceBuffer rejected a segment (append error)"));
    };
    sourceBuffer.addEventListener("updateend", onUpdateEnd);
    sourceBuffer.addEventListener("error", onError);
    try {
      sourceBuffer.appendBuffer(bytes);
    } catch (error) {
      sourceBuffer.removeEventListener("updateend", onUpdateEnd);
      sourceBuffer.removeEventListener("error", onError);
      reject(error);
    }
  });
}

async function streamInto(mediaSource, manifest) {
  let sourceBuffer;
  try {
    sourceBuffer = mediaSource.addSourceBuffer(manifest.mime);
  } catch (error) {
    failClosed("This browser cannot play this stream: " + error.message);
    return;
  }
  sourceBuffer.mode = "sequence";

  try {
    if (manifest.has_init) {
      setOverlay("Loading…");
      await appendChunk(sourceBuffer, await fetchSegmentBytes("/init"));
    }
    // Strict in-order append; a gap/substitution fails the AAD-bound fetch on the
    // runtime side (4xx) or the SourceBuffer append, and we fail closed.
    for (let i = 0; i < manifest.segment_count; i += 1) {
      const bytes = await fetchSegmentBytes("/segment/" + i);
      await appendChunk(sourceBuffer, bytes);
      if (i === 0) {
        setOverlay(null);
      }
    }
    if (mediaSource.readyState === "open") {
      mediaSource.endOfStream();
    }
    setStatus("");
  } catch (error) {
    failClosed("Playback failed: " + error.message);
  }
}

async function start() {
  if (!sessionId) {
    failClosed("No decrypt session — open this video from your Library.");
    return;
  }
  if (typeof window.MediaSource !== "function") {
    failClosed("This browser does not support Media Source Extensions.");
    return;
  }

  let manifest;
  try {
    manifest = await fetchPlayManifest();
  } catch (error) {
    failClosed("Could not open the protected stream: " + error.message);
    return;
  }

  if (!window.MediaSource.isTypeSupported(manifest.mime)) {
    failClosed("This device cannot decode " + manifest.mime + ".");
    return;
  }

  const mediaSource = new MediaSource();
  video.src = URL.createObjectURL(mediaSource);
  mediaSource.addEventListener("sourceopen", () => {
    URL.revokeObjectURL(video.src);
    streamInto(mediaSource, manifest);
  }, { once: true });
}

start();
