// ddrm-viewer — protected NON-MEDIA viewer (satisfies elastos.viewer/document@1).
//
// Containment contract (mirrors elacity-player + the runtime decrypt boundary):
// this capsule renders an owned, protected asset (image / PDF / text / code / 3D)
// by fetching the ALREADY-DECRYPTED object bytes through an opaque per-open SESSION
// handle and the runtime's scoped object route. It never sees — and never asks for
// — the CEK, the IV, the key-release receipt, or any KMS/wallet/chain material. The
// decrypt happens in the decrypt-provider sandbox; only the cleartext object crosses
// to the viewer. We additionally assert the manifest carries no key fields and fail
// closed if it does.
//
// Scoped object contract served by elastos-server:
//   GET /api/viewers/ddrm-viewer/object/{session}
//        -> { schema:"elastos.viewer.object/v1", mime, byte_length, expires_at }
//           (metadata only — no key)
//   GET /api/viewers/ddrm-viewer/object/{session}/bytes
//        -> decrypted object bytes (octet-stream, no-store); expired/unauthorized => 4xx
//        -> 403 for pixel-lock assets (their raw bytes never leave the decrypt boundary)
//   GET /api/viewers/ddrm-viewer/object/{session}/page?n=N
//        -> one watermarked page IMAGE for a pixel-lock asset (e.g. PDF rendered in-boundary);
//           X-Asset-Pages carries the page count. The raw document never reaches the browser.
//
// Auth: the Home launch token rides in the `x-elastos-home-token` header, exactly
// like the other viewer capsules.

const renderRoot = document.getElementById("render-root");
const overlay = document.getElementById("overlay");
const overlayText = document.getElementById("overlay-text");
const statusEl = document.getElementById("status");
const mimeEl = document.getElementById("mime");
const kindBadge = document.getElementById("kind-badge");

const VIEWER_ID = "ddrm-viewer";
const OBJECT_SCHEMA = "elastos.viewer.object/v1";
// Hard cap so a hostile/oversized manifest can't exhaust the viewer (the runtime
// also bounds this; defense in depth).
const MAX_OBJECT_BYTES = 64 * 1024 * 1024;
// Fields that must NEVER appear in anything this viewer receives. Their presence
// means a broken boundary, so we refuse to render.
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
let objectUrl = null;

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
  // No partial render, no key material, no retained object URL.
  if (objectUrl) {
    try {
      URL.revokeObjectURL(objectUrl);
    } catch (_error) {
      /* best effort */
    }
    objectUrl = null;
  }
  renderRoot.replaceChildren();
  setOverlay(message, true);
  setStatus(message, true);
  console.error("[ddrm-viewer] fail closed:", message);
}

function objectUrlFor(suffix) {
  const base = "/api/viewers/" + encodeURIComponent(VIEWER_ID)
    + "/object/" + encodeURIComponent(sessionId);
  return suffix ? base + suffix : base;
}

function assertNoKeyMaterial(manifest) {
  const lowered = JSON.stringify(manifest).toLowerCase();
  for (const field of FORBIDDEN_KEY_FIELDS) {
    if (Object.prototype.hasOwnProperty.call(manifest, field)) {
      throw new Error("object manifest carried a forbidden key field: " + field);
    }
  }
  if (lowered.includes("\"raw_cek\"") || lowered.includes("\"wrapped_cek\"")) {
    throw new Error("object manifest carried nested key material");
  }
}

async function fetchManifest() {
  const resp = await fetch(objectUrlFor(""), { headers: { ...launchHeaders() } });
  if (!resp.ok) {
    throw new Error("object session unavailable: " + resp.status);
  }
  const manifest = await resp.json();
  if (manifest.schema !== OBJECT_SCHEMA) {
    throw new Error("unexpected object manifest schema: " + manifest.schema);
  }
  assertNoKeyMaterial(manifest);
  if (typeof manifest.mime !== "string" || manifest.mime.length === 0) {
    throw new Error("object manifest missing mime");
  }
  if (!Number.isInteger(manifest.byte_length) || manifest.byte_length < 0) {
    throw new Error("object manifest has invalid byte_length");
  }
  if (manifest.byte_length > MAX_OBJECT_BYTES) {
    throw new Error("object exceeds the viewer size limit");
  }
  return manifest;
}

async function fetchObjectBytes() {
  const resp = await fetch(objectUrlFor("/bytes"), { headers: { ...launchHeaders() } });
  if (!resp.ok) {
    throw new Error("object fetch failed: " + resp.status);
  }
  return await resp.arrayBuffer();
}

// The render kind we route on, from the asset mime.
function kindFor(mime) {
  const m = mime.toLowerCase();
  if (m.startsWith("image/")) return "image";
  if (m === "application/pdf") return "pdf";
  if (m.startsWith("text/") || m === "application/json") return "text";
  if (m.startsWith("model/") || m === "application/octet-stream") return "model";
  return "download";
}

function renderImage(bytes, mime) {
  const blob = new Blob([bytes], { type: mime });
  objectUrl = URL.createObjectURL(blob);
  const img = document.createElement("img");
  img.className = "object-image";
  img.alt = "Owned image";
  img.decoding = "async";
  img.src = objectUrl;
  renderRoot.replaceChildren(img);
}

// PIXEL-LOCK: the asset (e.g. PDF) is rendered to flattened, watermarked page images
// IN the decrypt boundary; the raw file never reaches the browser. We page through those
// images via the scoped page route — so there is no blob/data PDF for a browser to block,
// and the buyer watermark is baked into every page. This is the canonical PDF path.
async function renderPixelLockPager(manifest) {
  let total = Number.isInteger(manifest.total_pages) && manifest.total_pages > 0
    ? manifest.total_pages
    : 1;
  let current = 0;

  const img = document.createElement("img");
  img.className = "object-image object-page";
  img.alt = "Owned document page";
  img.decoding = "async";

  const bar = document.createElement("div");
  bar.className = "pager-bar";
  const prev = document.createElement("button");
  prev.type = "button";
  prev.className = "pager-btn";
  prev.textContent = "‹ Prev";
  const label = document.createElement("span");
  label.className = "pager-label";
  const next = document.createElement("button");
  next.type = "button";
  next.className = "pager-btn";
  next.textContent = "Next ›";
  bar.append(prev, label, next);

  const stage = document.createElement("div");
  stage.className = "pager-stage";
  stage.append(img);

  function syncControls() {
    // Single-page assets (e.g. a watermarked image) need no pager chrome.
    bar.hidden = total <= 1;
    label.textContent = `Page ${current + 1} of ${total}`;
    prev.disabled = current <= 0;
    next.disabled = current >= total - 1;
  }

  // Client-side page cache (n -> object URL). The decrypt boundary already caches the
  // RENDERED page (warm parse + page-image LRU), so prefetching the NEXT page primes both
  // the boundary cache AND the browser blob, making forward navigation feel instant.
  // Bounded so a long document can't accumulate unbounded blob URLs.
  const MAX_CACHED_PAGES = 8;
  const cache = new Map(); // n -> { url }
  const inflight = new Map(); // n -> Promise<string url>

  function evictIfNeeded() {
    while (cache.size > MAX_CACHED_PAGES) {
      // Evict the page furthest from the current view (keep a window around `current`).
      let victim = null;
      let bestDist = -1;
      for (const n of cache.keys()) {
        const d = Math.abs(n - current);
        if (d > bestDist) { bestDist = d; victim = n; }
      }
      if (victim === null) break;
      const entry = cache.get(victim);
      cache.delete(victim);
      if (entry && entry.url) {
        try { URL.revokeObjectURL(entry.url); } catch (_e) { /* best effort */ }
      }
    }
  }

  // Fetch page `n` to a blob URL, caching it. Returns the cached URL if present. Shares
  // in-flight requests so a click + a prefetch for the same page don't double-fetch.
  function fetchPage(n) {
    const hit = cache.get(n);
    if (hit) return Promise.resolve(hit.url);
    const pending = inflight.get(n);
    if (pending) return pending;
    const p = (async () => {
      const resp = await fetch(objectUrlFor("/page?n=" + encodeURIComponent(n)), {
        headers: { ...launchHeaders() },
      });
      if (!resp.ok) {
        throw new Error("page fetch failed: " + resp.status);
      }
      const pages = parseInt(resp.headers.get("x-asset-pages") || "", 10);
      if (Number.isInteger(pages) && pages > 0) {
        total = pages;
      }
      const blob = await resp.blob();
      const url = URL.createObjectURL(blob);
      cache.set(n, { url });
      evictIfNeeded();
      return url;
    })();
    inflight.set(n, p);
    p.catch(() => {}).finally(() => inflight.delete(n));
    return p;
  }

  // Background prefetch — never surfaces an error (best effort) and never blocks the UI.
  function prefetch(n) {
    if (n < 0 || n >= total) return;
    fetchPage(n).catch(() => {});
  }

  async function loadPage(n) {
    const url = await fetchPage(n);
    img.src = url;
    current = n;
    syncControls();
    // Warm the neighbours so the next click is instant (forward first — the common path).
    prefetch(n + 1);
    prefetch(n - 1);
  }

  prev.addEventListener("click", () => {
    if (current > 0) loadPage(current - 1).catch((e) => failClosed("Could not load page: " + e.message));
  });
  next.addEventListener("click", () => {
    if (current < total - 1) loadPage(current + 1).catch((e) => failClosed("Could not load page: " + e.message));
  });

  renderRoot.replaceChildren(bar, stage);
  await loadPage(0);
}

function renderText(bytes) {
  // textContent (never innerHTML) — the decrypted bytes are shown as inert text,
  // so an HTML/script payload inside an owned asset cannot execute in this frame.
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  } catch (_error) {
    text = "";
  }
  const pre = document.createElement("pre");
  pre.className = "object-text";
  pre.textContent = text;
  renderRoot.replaceChildren(pre);
}

// 3D is a planned tier: rendering glTF/GLB needs a bundled WebGL renderer, which
// must ship with the capsule (no CDN, per the net policy) — tracked as the next
// viewer increment. Until then we render an honest placeholder rather than pulling
// an external script or silently failing.
function renderModelPlaceholder(mime, byteLength) {
  const box = document.createElement("div");
  box.className = "object-placeholder";
  const h = document.createElement("p");
  h.className = "placeholder-title";
  h.textContent = "3D asset decrypted";
  const p = document.createElement("p");
  p.className = "placeholder-sub";
  p.textContent = `${mime} · ${byteLength} bytes — the protected 3D bytes opened `
    + "through the decrypt boundary. The bundled 3D render tier is the next viewer increment.";
  box.append(h, p);
  renderRoot.replaceChildren(box);
}

async function start() {
  if (!sessionId) {
    failClosed("No decrypt session — open this asset from your Library.");
    return;
  }

  let manifest;
  try {
    manifest = await fetchManifest();
  } catch (error) {
    failClosed("Could not open the protected asset: " + error.message);
    return;
  }

  mimeEl.textContent = manifest.mime;
  const kind = kindFor(manifest.mime);
  kindBadge.textContent = kind;
  kindBadge.hidden = false;

  // PIXEL-LOCK assets (e.g. PDF) are served as watermarked page images — never raw bytes.
  // Take the page route and return; the /bytes egress is refused server-side for these.
  if (manifest.pixel_locked) {
    try {
      await renderPixelLockPager(manifest);
      setOverlay(null);
      setStatus("");
    } catch (error) {
      failClosed("Could not render the protected document: " + error.message);
    }
    return;
  }

  let bytes;
  try {
    bytes = await fetchObjectBytes();
    if (bytes.byteLength !== manifest.byte_length) {
      throw new Error("decrypted length did not match the manifest");
    }
  } catch (error) {
    failClosed("Failed to load the protected asset: " + error.message);
    return;
  }

  try {
    switch (kind) {
      case "image":
        renderImage(bytes, manifest.mime);
        break;
      case "pdf":
        renderPdf(bytes, manifest.mime);
        break;
      case "text":
        renderText(bytes);
        break;
      case "model":
        renderModelPlaceholder(manifest.mime, manifest.byte_length);
        break;
      default:
        renderModelPlaceholder(manifest.mime, manifest.byte_length);
        break;
    }
    setOverlay(null);
    setStatus("");
  } catch (error) {
    failClosed("Could not render the asset: " + error.message);
  }
}

start();
