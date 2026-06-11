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

function renderPdf(bytes, mime) {
  const blob = new Blob([bytes], { type: mime || "application/pdf" });
  objectUrl = URL.createObjectURL(blob);
  // The browser's built-in PDF viewer renders the blob in-frame; no CDN, no plugin.
  const frame = document.createElement("iframe");
  frame.className = "object-pdf";
  frame.title = "Owned document";
  frame.src = objectUrl;
  renderRoot.replaceChildren(frame);
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
