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
//             is_protected, expires_at, tracks?[] }       (metadata only, no key)
//        tracks[] (when present): per-AdaptationSet { index, kind, mime,
//        segment_count, has_init } for split video+audio DASH.
//   GET /api/viewers/elacity-player/media/{session}/init
//        -> decrypted init segment bytes (octet-stream, no-store)   [single-track]
//   GET /api/viewers/elacity-player/media/{session}/segment/{index}
//        -> decrypted media segment bytes, in-range only            [single-track]
//   GET /api/viewers/elacity-player/media/{session}/track/{t}/init
//        -> decrypted init for track t                              [multi-track]
//   GET /api/viewers/elacity-player/media/{session}/track/{t}/segment/{index}
//        -> decrypted media segment for track t, in-range only      [multi-track]
//   out-of-range / expired / unauthorized => 4xx (fail closed)
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

// Best-effort autoplay once the first media is buffered. Browser policy blocks audible
// autoplay without a user gesture (and this viewer runs in a cross-document iframe, so the
// library click does not count). We therefore TRY to play; if the browser refuses, we surface
// a one-tap "click to play" prompt and start on the first gesture. Never fails the stream.
let kicked = false;
function kickoffPlayback() {
  if (kicked) return;
  kicked = true;
  const onGesture = () => {
    video.play().then(() => setOverlay(null)).catch(() => {});
  };
  video.play().then(() => {
    setOverlay(null);
  }).catch(() => {
    // Autoplay denied — keep controls, prompt for a tap, and start on first interaction.
    setOverlay("Click to play");
    overlay.addEventListener("click", onGesture, { once: true });
    video.addEventListener("click", onGesture, { once: true });
  });
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
  // Multi-track manifest (split DASH: separate video + audio AdaptationSets): validate each
  // track. The player attaches one MSE SourceBuffer per track (PC2's two-viewer model).
  if (Array.isArray(manifest.tracks) && manifest.tracks.length > 0) {
    for (const t of manifest.tracks) {
      if (typeof t.mime !== "string" || t.mime.length === 0) {
        throw new Error("media track missing mime/codecs");
      }
      if (!Number.isInteger(t.segment_count) || t.segment_count < 0) {
        throw new Error("media track has invalid segment_count");
      }
    }
    return manifest;
  }
  // Legacy single-track manifest (muxed / older sessions).
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
        kickoffPlayback();
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

// Multi-track playback (split DASH): one MSE SourceBuffer per track. Mirrors PC2's player —
// video + audio are appended into independent SourceBuffers and stay in sync via each
// fragment's baseMediaDecodeTime (default 'segments' mode). A single audio track uses
// 'sequence' mode (audio-only Bento4 DASH resets baseMediaDecodeTime between fragments).
async function streamTracks(mediaSource, tracks) {
  const isMulti = tracks.length > 1;
  const buffers = [];
  try {
    setOverlay("Loading…");
    for (let i = 0; i < tracks.length; i += 1) {
      const track = tracks[i];
      const idx = Number.isInteger(track.index) ? track.index : i;
      let sourceBuffer;
      try {
        sourceBuffer = mediaSource.addSourceBuffer(track.mime);
      } catch (error) {
        failClosed("This browser cannot play " + track.mime + ": " + error.message);
        return;
      }
      if (!isMulti && track.kind !== "video") {
        sourceBuffer.mode = "sequence";
      }
      buffers.push({ sourceBuffer, track, idx });
    }
    // Append every track's init first so MSE has codec config for all tracks before media.
    for (const b of buffers) {
      if (b.track.has_init) {
        await appendChunk(b.sourceBuffer, await fetchSegmentBytes("/track/" + b.idx + "/init"));
      }
    }
    // Stream each track's segments; appends WITHIN a track stay strictly in order (a
    // gap/substitution fails the AAD-bound fetch 4xx or the append, and we fail closed),
    // while different tracks fill in parallel.
    let cleared = false;
    await Promise.all(
      buffers.map(async (b) => {
        for (let i = 0; i < b.track.segment_count; i += 1) {
          const bytes = await fetchSegmentBytes("/track/" + b.idx + "/segment/" + i);
          await appendChunk(b.sourceBuffer, bytes);
          if (!cleared) {
            cleared = true;
            setOverlay(null);
            kickoffPlayback();
          }
        }
      }),
    );
    if (mediaSource.readyState === "open") {
      mediaSource.endOfStream();
    }
    setStatus("");
  } catch (error) {
    failClosed("Playback failed: " + error.message);
  }
}

// Audio-only when there is no video track (multi-track) or the single-track mime is audio/*.
function isAudioOnly(manifest) {
  if (Array.isArray(manifest.tracks) && manifest.tracks.length > 0) {
    return manifest.tracks.every((t) => t.kind !== "video");
  }
  return typeof manifest.mime === "string"
    && manifest.mime.toLowerCase().startsWith("audio/");
}

// Best-effort: fetch the public cover via the launch-token-authed route and set it as the poster.
// A `<video poster>` load can't carry custom headers, so we fetch + blob-URL it ourselves. Any
// failure is silent — falls back to the generated placeholder set first (never a black frame).
async function applyCoverPoster() {
  try {
    const resp = await fetch(mediaUrl("/cover"), { headers: { ...launchHeaders() } });
    if (!resp.ok) return;
    const blob = await resp.blob();
    if (!blob || blob.size === 0) return;
    video.poster = URL.createObjectURL(blob);
  } catch (_error) {
    /* best effort — keep the generated placeholder poster */
  }
}

// Brand placeholder tokens (canvas literals; see the design-token contract). Dark graphite card +
// soft hairline border + muted label. The lime accent (#b7ff5a) is reserved for verified/active
// states and must NEVER appear on a placeholder.
const PH_BG = "#111111"; // --color-graphite
const PH_BORDER = "rgba(255,255,255,0.10)"; // --color-border-soft
const PH_CHIP_BG = "rgba(255,255,255,0.08)"; // muted surface
const PH_CHIP_TEXT = "rgba(255,255,255,0.48)"; // muted label
const PH_TITLE = "rgba(255,255,255,0.72)";
const PH_SUBTLE = "rgba(255,255,255,0.40)";
const PH_FONT = "-apple-system, 'Segoe UI', Roboto, sans-serif";

function phRoundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

// Fallback cover for an asset with no creator-pinned thumbnail. Reproduces the SAME canonical
// public placeholder the Create portal generates at mint (and the marketplace shows): on the brand
// graphite card, a muted file-type chip + title. Mirroring `creator.js` (thumbGeneric) keeps one
// placeholder identity everywhere, so a legacy asset looks identical to a fresh one — including
// audio, which now uses the uniform card (no waveform). Returned as a `<video poster>`.
function generatePlaceholderPoster(mime, label) {
  try {
    const W = 1280;
    const H = 720;
    const canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    const ctx = canvas.getContext("2d");
    if (!ctx) return "";
    const title = (label || "").trim();

    ctx.fillStyle = PH_BG;
    ctx.fillRect(0, 0, W, H);
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";

    // Muted file-type chip (e.g. "MP4") on the graphite card — matches creator.js thumbGeneric.
    const badge = ((mime || "").split("/").pop() || "file").replace(/^x-/, "").slice(0, 5).toUpperCase() || "FILE";
    ctx.font = "bold 80px " + PH_FONT;
    const chipW = Math.max(ctx.measureText(badge).width + 110, 240);
    const chipH = 170;
    phRoundRect(ctx, W / 2 - chipW / 2, H * 0.42 - chipH / 2, chipW, chipH, 28);
    ctx.fillStyle = PH_CHIP_BG;
    ctx.fill();
    ctx.fillStyle = PH_CHIP_TEXT;
    ctx.fillText(badge, W / 2, H * 0.42 + 4);
    if (title) {
      ctx.fillStyle = PH_TITLE;
      ctx.font = "32px " + PH_FONT;
      const clipped = title.length > 44 ? title.slice(0, 43) + "\u2026" : title;
      ctx.fillText(clipped, W / 2, H * 0.66);
    }
    ctx.fillStyle = PH_SUBTLE;
    ctx.font = "22px " + PH_FONT;
    ctx.fillText(mime || "unknown", W / 2, H * 0.66 + 44);
    drawPhBorder(ctx, W, H);
    return canvas.toDataURL("image/png");
  } catch (_error) {
    return "";
  }
}

function drawPhBorder(ctx, w, h) {
  ctx.strokeStyle = PH_BORDER;
  ctx.lineWidth = 2;
  ctx.strokeRect(1, 1, w - 2, h - 2);
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

  // Audio has no video frames. Always give it a poster: paint a generated placeholder immediately
  // (so there's never a black frame), then upgrade to the real cover art if one was pinned at mint.
  if (isAudioOnly(manifest)) {
    const mime = manifest.mime || (Array.isArray(manifest.tracks) && manifest.tracks[0] && manifest.tracks[0].mime) || "audio/*";
    const placeholder = generatePlaceholderPoster(mime, manifest.title || manifest.name || document.title || "");
    if (placeholder) video.poster = placeholder;
    if (manifest.has_cover) applyCoverPoster();
  }

  // Multi-track session: validate every track's codec, then stream per-track.
  if (Array.isArray(manifest.tracks) && manifest.tracks.length > 0) {
    for (const t of manifest.tracks) {
      if (!window.MediaSource.isTypeSupported(t.mime)) {
        failClosed("This device cannot decode " + t.mime + ".");
        return;
      }
    }
    const mediaSource = new MediaSource();
    video.src = URL.createObjectURL(mediaSource);
    mediaSource.addEventListener("sourceopen", () => {
      URL.revokeObjectURL(video.src);
      streamTracks(mediaSource, manifest.tracks);
    }, { once: true });
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
