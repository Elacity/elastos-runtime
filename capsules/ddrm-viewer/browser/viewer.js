// ddrm-viewer — protected NON-MEDIA viewer (satisfies elastos.viewer/document@1).
//
// Containment contract (mirrors the protected media player + the runtime decrypt boundary):
// this capsule renders an owned, protected asset (image / PDF / text / code / 3D)
// by fetching the ALREADY-DECRYPTED object bytes through an opaque per-open SESSION
// handle and the runtime's scoped object route. It never sees — and never asks for
// — the CEK, the IV, the key-release receipt, or any KMS/wallet/chain material. The
// decrypt happens in the runtime decrypt sandbox; only the cleartext object crosses
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
//        -> one render-locked page for a render-locked asset; X-Asset-Pages carries the count.
//           The page's content type comes from the manifest's `page_content_type`:
//             • image/jpeg  -> PIXEL-LOCK: a flattened, watermarked page image (PDF/img/text/cbz)
//             • text/html   -> HTML-LOCK:  a sanitised, self-contained EPUB chapter document,
//                              shown in a script-less sandbox iframe (no allow-scripts).
//           The raw document never reaches the browser in either case.
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

// The launch token arrives in the URL FRAGMENT, never the query string: a fragment is not
// transmitted to any server, so the token stays out of Referer, the gateway access log and shared
// browser history. Same idiom Home already uses for every app launch
// (gateway_home_runtime.rs::append_home_launch_token_to_route). It is delivery only — every call
// below still presents it as the `x-elastos-home-token` header.
function fragmentValue(name) {
  try {
    return new URLSearchParams(window.location.hash.replace(/^#/, "")).get(name);
  } catch (_error) {
    return null;
  }
}

const homeToken = fragmentValue("home_token");
// The session id stays in the QUERY string on purpose: it is the per-open capability these read
// routes are scoped by, it is single-asset and short-lived, and `/close` revokes it.
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

// The render kind we route on, from the asset mime. (Used for the badge label; the actual
// render path for render-locked assets is chosen by the manifest's `pixel_locked` flag.)
function kindFor(mime) {
  const m = mime.toLowerCase();
  if (m.startsWith("image/")) return "image";
  if (m === "application/pdf") return "pdf";
  if (m === "application/epub+zip" || m === "application/epub") return "ebook";
  if (m === "application/vnd.comicbook+zip" || m === "application/x-cbz") return "comic";
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

// RENDER-LOCK pager: the asset is rendered to a locked per-page representation IN the decrypt
// boundary; the raw file never reaches the browser. Two representations share this one pager:
//   • PIXEL-LOCK (PDF / images / text / code / comics) → a flattened, watermarked page IMAGE.
//   • HTML-LOCK  (EPUB) → a sanitised, self-contained chapter HTML document, shown in a
//     script-less SANDBOX iframe (no allow-scripts, no allow-same-origin → inert opaque origin).
// In both cases there is no raw blob a browser could block/leak, and every page carries the
// buyer watermark. The representation is chosen by the manifest's `page_content_type`.
async function renderLockedPager(manifest) {
  const isHtml = typeof manifest.page_content_type === "string"
    && manifest.page_content_type.toLowerCase().startsWith("text/html");
  // Everything render-locked is now a CONTINUOUS SCROLL reader: PIXEL-LOCK pages (PDF / text /
  // code / comics / images) stack as page images; HTML-LOCK (EPUB) chapters stack as inert,
  // content-sized sandbox iframes. The reader just scrolls; the indicator follows the scroll and
  // Prev/Next jump-scroll to the neighbouring page/chapter.
  if (isHtml) {
    return renderScrollPagerHtml(manifest);
  }
  return renderScrollPager(manifest);
}

// Continuous-scroll reader for PIXEL-LOCK page images (PDF / text / code / comics / single image).
// All pages live in one vertically-scrolling stage; pages load lazily in a window around the
// viewport (and unload when far, so a long document can't pin unbounded blob URLs). The page
// indicator follows the scroll position, and Prev/Next smooth-scroll to the neighbouring page.
async function renderScrollPager(manifest) {
  const unit = "Page";
  let total = Number.isInteger(manifest.total_pages) && manifest.total_pages > 0
    ? manifest.total_pages
    : 1;
  let current = 0;

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
  stage.className = "pager-stage pager-scroll";

  // One slot per page. Slots reserve vertical space (CSS min-height) before their image loads so
  // the scrollbar reflects the whole document and the scroll-position → page math is stable.
  const slots = []; // { slot, img, spinner, plabel, retry, url, loaded, pending, seen, attempts, nextRetry }
  let seq = 0; // monotonic "most recently wanted" stamp for LRU eviction
  // A page that keeps failing to decrypt must FAIL CLOSED with an explanation, not spin forever or
  // re-fetch on every scroll frame. Bounded attempts + backoff, then an explicit error + Retry.
  const MAX_PAGE_ATTEMPTS = 4;

  function setLoading(s, n) {
    s.slot.classList.remove("page-failed");
    s.spinner.hidden = false;
    s.retry.hidden = true;
    s.plabel.textContent = `Page ${n + 1}`;
  }

  function setFailed(s, n) {
    s.slot.classList.add("page-failed");
    s.spinner.hidden = true;
    s.retry.hidden = false;
    s.plabel.textContent = `Couldn't load page ${n + 1}`;
  }

  function makeSlot(n) {
    const slot = document.createElement("div");
    slot.className = "pager-page-slot";
    slot.dataset.page = String(n);
    // A real loading state (spinner + page number) shown UNTIL the page has actually decrypted +
    // decoded — so a not-yet-ready page never flashes a broken-image glyph while it streams in.
    const loader = document.createElement("div");
    loader.className = "page-loading";
    const spinner = document.createElement("span");
    spinner.className = "page-spinner";
    spinner.setAttribute("aria-hidden", "true");
    const plabel = document.createElement("span");
    plabel.className = "page-loading-label";
    plabel.textContent = `Page ${n + 1}`;
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "page-retry";
    retry.textContent = "Retry";
    retry.hidden = true;
    loader.append(spinner, plabel, retry);
    const img = document.createElement("img");
    img.className = "object-image object-page";
    // Empty alt + hidden-until-loaded (CSS): an <img> with no usable src never shows the broken
    // glyph; the page only becomes visible once it has fully decoded (the load event below).
    img.alt = "";
    img.decoding = "async";
    img.addEventListener("load", () => {
      slot.classList.add("is-loaded");
      // Pin the rendered height so the slot keeps its geometry even if the image is later evicted
      // (no scroll jump when an off-screen page is dropped or re-loaded).
      const h = img.getBoundingClientRect().height;
      if (h > 0) slot.style.minHeight = h + "px";
    });
    slot.append(loader, img);
    const s = { slot, img, spinner, plabel, retry, url: null, loaded: false, pending: false, seen: 0, attempts: 0, nextRetry: 0 };
    retry.addEventListener("click", () => {
      s.attempts = 0;
      s.nextRetry = 0;
      setLoading(s, n);
      ensureLoaded(n);
    });
    return s;
  }

  function ensureSlots() {
    for (let n = slots.length; n < total; n++) {
      const s = makeSlot(n);
      slots.push(s);
      stage.append(s.slot);
    }
  }

  // Shared in-flight de-dup so a scroll pass + a Prev/Next don't double-fetch the same page.
  const inflight = new Map();
  function fetchPageUrl(n) {
    if (inflight.has(n)) return inflight.get(n);
    const p = (async () => {
      const resp = await fetch(objectUrlFor("/page?n=" + encodeURIComponent(n)), {
        headers: { ...launchHeaders() },
      });
      if (!resp.ok) throw new Error("page fetch failed: " + resp.status);
      // The page render is authoritative for the page count — grow the slot list if it differs
      // from the manifest's estimate.
      const pages = parseInt(resp.headers.get("x-asset-pages") || "", 10);
      if (Number.isInteger(pages) && pages > 0 && pages !== total) {
        total = pages;
        ensureSlots();
        syncControls();
      }
      const blob = await resp.blob();
      return URL.createObjectURL(blob);
    })();
    inflight.set(n, p);
    p.catch(() => {}).finally(() => inflight.delete(n));
    return p;
  }

  async function ensureLoaded(n) {
    const s = slots[n];
    if (!s) return;
    s.seen = ++seq; // freshly wanted — protects it from LRU eviction
    if (s.loaded || s.pending) return;
    if (s.attempts >= MAX_PAGE_ATTEMPTS) return; // failed closed — waits for an explicit Retry
    if (Date.now() < s.nextRetry) return; // backing off between attempts (no per-frame hammering)
    s.pending = true;
    try {
      const url = await fetchPageUrl(n);
      s.url = url;
      s.img.src = url;
      s.loaded = true;
      s.attempts = 0;
    } catch (_e) {
      s.attempts++;
      if (s.attempts >= MAX_PAGE_ATTEMPTS) {
        setFailed(s, n);
      } else {
        // Backoff grows with attempts so a transiently-flaky page retries soon but a hard failure
        // can't refetch on every scroll frame.
        s.nextRetry = Date.now() + s.attempts * 900;
      }
    } finally {
      s.pending = false;
    }
  }

  function unload(n) {
    const s = slots[n];
    if (!s || !s.loaded) return;
    if (s.url) {
      try { URL.revokeObjectURL(s.url); } catch (_e) { /* best effort */ }
    }
    s.url = null;
    s.loaded = false;
    s.img.removeAttribute("src");
    // Keep the pinned inline min-height so the slot holds its place; just show the loader again.
    s.slot.classList.remove("is-loaded");
  }

  // Hold up to KEEP decoded pages resident at once. A normal-length document (comic / PDF) fits
  // entirely under the cap, so once a page is decoded it STAYS decoded — scrolling back up never
  // re-decrypts it. Only a very long document exceeds the cap, and then we evict the decoded pages
  // FURTHEST from the current view (LRU-by-distance), never one near where the reader is.
  const KEEP = 32;
  const BEHIND = 3;
  const AHEAD = 4;
  function evictIfNeeded() {
    let loadedCount = 0;
    for (const s of slots) if (s.loaded) loadedCount++;
    while (loadedCount > KEEP) {
      let victim = -1;
      let bestDist = -1;
      for (let n = 0; n < slots.length; n++) {
        if (!slots[n].loaded) continue;
        const d = Math.abs(n - current);
        if (d > bestDist) { bestDist = d; victim = n; }
      }
      if (victim < 0) break;
      unload(victim);
      loadedCount--;
    }
  }

  function refreshWindow() {
    const lo = Math.max(0, current - BEHIND);
    const hi = Math.min(slots.length - 1, current + AHEAD);
    for (let n = lo; n <= hi; n++) ensureLoaded(n);
    evictIfNeeded();
  }

  function syncControls() {
    bar.hidden = total <= 1;
    label.textContent = `${unit} ${current + 1} of ${total}`;
    prev.disabled = current <= 0;
    next.disabled = current >= total - 1;
  }

  // Which page is the reader "on"? The page whose top sits just above a probe line a third of the
  // way down the stage. Slot tops increase monotonically, so we walk OUTWARD from the last known
  // page instead of rescanning from 0 — incremental scroll reads only a couple of rects per frame,
  // even in a thousand-page document (no O(n) layout thrash).
  function computeCurrent() {
    const sr = stage.getBoundingClientRect();
    const probe = sr.top + sr.height * 0.35;
    let cur = Math.max(0, Math.min(current, slots.length - 1));
    while (cur + 1 < slots.length && slots[cur + 1].slot.getBoundingClientRect().top <= probe) cur++;
    while (cur > 0 && slots[cur].slot.getBoundingClientRect().top > probe) cur--;
    if (cur !== current) {
      current = cur;
      syncControls();
    }
    refreshWindow();
  }

  let rafPending = 0;
  stage.addEventListener("scroll", () => {
    if (rafPending) return;
    rafPending = requestAnimationFrame(() => {
      rafPending = 0;
      computeCurrent();
    });
  });

  function scrollToPage(n) {
    const s = slots[n];
    if (s) s.slot.scrollIntoView({ behavior: "smooth", block: "start" });
  }
  prev.addEventListener("click", () => { if (current > 0) scrollToPage(current - 1); });
  next.addEventListener("click", () => { if (current < total - 1) scrollToPage(current + 1); });

  ensureSlots();
  renderRoot.replaceChildren(bar, stage);
  syncControls();
  await ensureLoaded(0);
  refreshWindow();
}

// Continuous-scroll reader for HTML-LOCK chapters (EPUB). Each chapter is a self-contained,
// sanitised, CSP-bearing document (default-src 'none'; no scripts; img/font data: only) shown in
// a SANDBOX iframe. The sandbox is `allow-same-origin` ONLY — scripts stay DENIED (no
// `allow-scripts`), so the chapter is inert and the strict CSP blocks all network; the one thing
// same-origin buys us is that the PARENT can read the chapter's rendered height to size each
// iframe to its content, letting variable-length chapters stack into one continuous scroll.
async function renderScrollPagerHtml(manifest) {
  const unit = "Chapter";
  let total = Number.isInteger(manifest.total_pages) && manifest.total_pages > 0
    ? manifest.total_pages
    : 1;
  let current = 0;

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
  stage.className = "pager-stage pager-scroll";

  // One slot per chapter. `measured` persists a chapter's rendered height so the scrollbar
  // geometry stays stable even after its iframe content is unloaded out of the window.
  const slots = []; // { slot, frame, html, loaded, pending, measured }

  function makeSlot(n) {
    const slot = document.createElement("div");
    slot.className = "pager-page-slot epub-slot";
    slot.dataset.page = String(n);
    const frame = document.createElement("iframe");
    frame.className = "object-epub";
    // allow-same-origin ONLY (NO allow-scripts): the chapter cannot run code or reach the
    // network (its own CSP is default-src 'none'); same-origin merely lets us measure its height.
    frame.setAttribute("sandbox", "allow-same-origin");
    frame.referrerPolicy = "no-referrer";
    frame.title = `Owned book chapter ${n + 1}`;
    slot.append(frame);
    return { slot, frame, html: null, loaded: false, pending: false, measured: 0 };
  }

  function ensureSlots() {
    for (let n = slots.length; n < total; n++) {
      const s = makeSlot(n);
      slots.push(s);
      stage.append(s.slot);
    }
  }

  // Size a chapter iframe (and reserve its slot height) to its rendered content. Re-measured a
  // couple of times because data: images/fonts can settle layout a frame or two after load.
  function measure(s) {
    try {
      const doc = s.frame.contentDocument;
      if (!doc) return;
      const h = Math.max(
        doc.documentElement ? doc.documentElement.scrollHeight : 0,
        doc.body ? doc.body.scrollHeight : 0,
      );
      if (h > 0) {
        s.measured = h;
        s.frame.style.height = h + "px";
        s.slot.style.minHeight = h + "px";
      }
    } catch (_e) {
      /* same-origin read blocked — leave the reserved default height */
    }
  }

  const inflight = new Map();
  function fetchHtml(n) {
    if (inflight.has(n)) return inflight.get(n);
    const p = (async () => {
      const resp = await fetch(objectUrlFor("/page?n=" + encodeURIComponent(n)), {
        headers: { ...launchHeaders() },
      });
      if (!resp.ok) throw new Error("page fetch failed: " + resp.status);
      const pages = parseInt(resp.headers.get("x-asset-pages") || "", 10);
      if (Number.isInteger(pages) && pages > 0 && pages !== total) {
        total = pages;
        ensureSlots();
        syncControls();
      }
      return resp.text();
    })();
    inflight.set(n, p);
    p.catch(() => {}).finally(() => inflight.delete(n));
    return p;
  }

  async function ensureLoaded(n) {
    const s = slots[n];
    if (!s || s.loaded || s.pending) return;
    s.pending = true;
    try {
      if (s.html === null) s.html = await fetchHtml(n);
      s.frame.addEventListener("load", () => {
        measure(s);
        // data: images settle layout a frame or two later — re-measure as each finishes (we can
        // read the same-origin doc) plus a couple of timed passes as a belt-and-braces fallback.
        try {
          const doc = s.frame.contentDocument;
          if (doc) {
            doc.querySelectorAll("img").forEach((img) => {
              if (!img.complete) img.addEventListener("load", () => measure(s), { once: true });
            });
          }
        } catch (_e) { /* same-origin read blocked — timed passes still cover it */ }
        setTimeout(() => measure(s), 150);
        setTimeout(() => measure(s), 500);
      }, { once: true });
      // Inert text — only ever assigned as srcdoc of a script-less, network-less sandbox iframe.
      s.frame.srcdoc = s.html;
      s.loaded = true;
    } catch (_e) {
      /* leave the placeholder; a later scroll pass retries */
    } finally {
      s.pending = false;
    }
  }

  function unload(n) {
    const s = slots[n];
    if (!s || !s.loaded) return;
    // Drop the heavy rendered document but KEEP the reserved height (s.measured via min-height)
    // so the scrollbar geometry doesn't jump as chapters page in and out of the window.
    s.frame.removeAttribute("srcdoc");
    s.loaded = false;
  }

  const BEHIND = 1;
  const AHEAD = 2;
  function refreshWindow() {
    const lo = Math.max(0, current - BEHIND);
    const hi = Math.min(slots.length - 1, current + AHEAD);
    for (let n = lo; n <= hi; n++) ensureLoaded(n);
    for (let n = 0; n < slots.length; n++) {
      if (n < lo - 1 || n > hi + 1) unload(n);
    }
  }

  function syncControls() {
    bar.hidden = total <= 1;
    label.textContent = `${unit} ${current + 1} of ${total}`;
    prev.disabled = current <= 0;
    next.disabled = current >= total - 1;
  }

  // Walk outward from the last known chapter (monotonic slot tops) instead of rescanning from 0,
  // so scroll tracking stays O(1) per frame regardless of chapter count.
  function computeCurrent() {
    const sr = stage.getBoundingClientRect();
    const probe = sr.top + sr.height * 0.35;
    let cur = Math.max(0, Math.min(current, slots.length - 1));
    while (cur + 1 < slots.length && slots[cur + 1].slot.getBoundingClientRect().top <= probe) cur++;
    while (cur > 0 && slots[cur].slot.getBoundingClientRect().top > probe) cur--;
    if (cur !== current) {
      current = cur;
      syncControls();
    }
    refreshWindow();
  }

  let rafPending = 0;
  stage.addEventListener("scroll", () => {
    if (rafPending) return;
    rafPending = requestAnimationFrame(() => {
      rafPending = 0;
      computeCurrent();
    });
  });

  function scrollToPage(n) {
    const s = slots[n];
    if (s) s.slot.scrollIntoView({ behavior: "smooth", block: "start" });
  }
  prev.addEventListener("click", () => { if (current > 0) scrollToPage(current - 1); });
  next.addEventListener("click", () => { if (current < total - 1) scrollToPage(current + 1); });

  ensureSlots();
  renderRoot.replaceChildren(bar, stage);
  syncControls();
  await ensureLoaded(0);
  refreshWindow();
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

// Honest fallback when a model can't be rendered (unsupported format, no WebGL, parse error).
// The bytes still opened through the decrypt boundary — we just can't draw them here.
function renderModelPlaceholder(mime, byteLength, reason) {
  const box = document.createElement("div");
  box.className = "object-placeholder";
  const h = document.createElement("p");
  h.className = "placeholder-title";
  h.textContent = "3D asset decrypted";
  const p = document.createElement("p");
  p.className = "placeholder-sub";
  const why = reason ? ` (${reason})` : "";
  p.textContent = `${mime} · ${byteLength} bytes — opened through the decrypt boundary`
    + `, but this 3D format could not be displayed${why}.`;
  box.append(h, p);
  renderRoot.replaceChildren(box);
}

// Pick the bundled loader for a 3D mime/extension. Returns null for unsupported formats.
function modelFormatFor(mime, fileName) {
  const m = (mime || "").toLowerCase();
  const ext = (fileName || "").toLowerCase().split(".").pop();
  if (m === "model/gltf-binary" || m === "model/gltf+json" || ext === "glb" || ext === "gltf") {
    return "gltf";
  }
  if (m === "model/stl" || ext === "stl") return "stl";
  if (m === "model/obj" || ext === "obj") return "obj";
  return null;
}

// 3D PASSTHROUGH tier (mirrors PC2's PassthroughRenderer + Three.js viewer): the protected model
// is decrypted in the boundary and the cleartext bytes are handed to this bundled, LOCAL Three.js
// viewer (no CDN). Unlike pixel-lock/html-lock, the raw model bytes do reach the browser here —
// the same trade-off PC2 makes for interactive 3D. Renders glTF/GLB, STL, and OBJ in a WebGL
// canvas with orbit controls; falls back to an honest placeholder on any failure.
async function renderModel3D(bytes, mime, fileName) {
  const format = modelFormatFor(mime, fileName);
  if (!format) {
    renderModelPlaceholder(mime, bytes.byteLength, "unsupported format");
    return;
  }

  let THREE;
  try {
    THREE = await import("three");
  } catch (_e) {
    renderModelPlaceholder(mime, bytes.byteLength, "3D engine unavailable");
    return;
  }

  const stage = document.createElement("div");
  stage.className = "object-3d";
  renderRoot.replaceChildren(stage);
  const width = stage.clientWidth || renderRoot.clientWidth || 640;
  const height = stage.clientHeight || renderRoot.clientHeight || 480;

  let renderer;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  } catch (_e) {
    renderModelPlaceholder(mime, bytes.byteLength, "WebGL unavailable");
    return;
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.setSize(width, height);
  stage.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(45, width / height, 0.01, 10000);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x404050, 1.1));
  const key = new THREE.DirectionalLight(0xffffff, 1.4);
  key.position.set(2.5, 4, 3);
  scene.add(key);

  // Add a parsed object/scene, then frame the camera around its bounding box.
  function addAndFrame(obj3d) {
    scene.add(obj3d);
    const box = new THREE.Box3().setFromObject(obj3d);
    if (box.isEmpty()) throw new Error("empty model");
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const maxDim = Math.max(size.x, size.y, size.z) || 1;
    const dist = (maxDim / (2 * Math.tan((Math.PI * camera.fov) / 360))) * 1.6;
    camera.position.set(center.x + dist * 0.5, center.y + dist * 0.4, center.z + dist);
    camera.near = dist / 100;
    camera.far = dist * 100;
    camera.updateProjectionMatrix();
    camera.lookAt(center);
    return center;
  }

  try {
    let center = new THREE.Vector3();
    if (format === "gltf") {
      const { GLTFLoader } = await import("./vendor/loaders/GLTFLoader.js");
      const gltf = await new Promise((resolve, reject) => {
        new GLTFLoader().parse(bytes, "", resolve, reject);
      });
      center = addAndFrame(gltf.scene);
    } else if (format === "stl") {
      const { STLLoader } = await import("./vendor/loaders/STLLoader.js");
      const geometry = new STLLoader().parse(bytes);
      geometry.computeVertexNormals();
      const mesh = new THREE.Mesh(
        geometry,
        new THREE.MeshStandardMaterial({ color: 0xb0b6c0, metalness: 0.1, roughness: 0.7 }),
      );
      center = addAndFrame(mesh);
    } else if (format === "obj") {
      const { OBJLoader } = await import("./vendor/loaders/OBJLoader.js");
      const text = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
      center = addAndFrame(new OBJLoader().parse(text));
    }

    const { OrbitControls } = await import("./vendor/controls/OrbitControls.js");
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(center);
    controls.enableDamping = true;
    controls.update();

    let raf = 0;
    const tick = () => {
      controls.update();
      renderer.render(scene, camera);
      raf = requestAnimationFrame(tick);
    };
    tick();

    // Keep the canvas sized to the frame.
    const onResize = () => {
      const w = stage.clientWidth || width;
      const h = stage.clientHeight || height;
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h);
    };
    window.addEventListener("resize", onResize);
    window.addEventListener("beforeunload", () => {
      cancelAnimationFrame(raf);
      renderer.dispose();
    });
  } catch (e) {
    try { renderer.dispose(); } catch (_e) { /* best effort */ }
    // Surface the real cause (a rejected loader, a missing vendor dependency, an empty scene…)
    // instead of a generic message — an honest failure is debuggable and never hides a problem.
    console.error("3D render failed:", e);
    const detail = (e && (e.message || String(e))) || "could not parse the model";
    renderModelPlaceholder(mime, bytes.byteLength, detail);
  }
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

  // RENDER-LOCK assets (PDF/images/text/comics → page images; EPUB → sanitised chapter HTML)
  // are served as a locked per-page representation — never raw bytes. Take the page route and
  // return; the /bytes egress is refused server-side for these.
  if (manifest.pixel_locked) {
    try {
      await renderLockedPager(manifest);
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
        // PDFs are served pixel-locked (handled above). A non-pixel-lock PDF should not occur;
        // show an honest placeholder rather than a raw embed (no plaintext PDF in the browser).
        renderModelPlaceholder(manifest.mime, manifest.byte_length);
        break;
      case "text":
        renderText(bytes);
        break;
      case "model":
        await renderModel3D(bytes, manifest.mime, manifest.file_name || "");
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

// Release the decrypt session (and the authority subprocess it pins) the moment this
// viewer goes away, instead of waiting for the server-side TTL. keepalive lets the
// request survive iframe/tab teardown while still carrying the launch-token header
// (sendBeacon cannot). Best effort by design: if it never arrives, the Home shell's
// window-close hook and the server's TTL sweep are the backstops.
window.addEventListener("pagehide", () => {
  if (!sessionId) return;
  try {
    fetch(objectUrlFor("/close"), {
      method: "POST",
      keepalive: true,
      headers: { ...launchHeaders() },
    }).catch(() => {});
  } catch (_e) {
    /* best effort — TTL is the backstop */
  }
});

start();
