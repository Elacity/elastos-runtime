//! Scoped NON-MEDIA object routes (the document/image/3D side of the dDRM viewer
//! seam) — the whole-object analogue of [`super::viewer_media`].
//!
//! These routes are how the `ddrm-viewer` capsule renders an owned, protected asset
//! without ever touching key material. The runtime holds, per open decrypt session,
//! ONLY public metadata (mime / byte_length / expiry) plus a handle to the gateway-
//! spawned local key-authority. The cleartext bytes are produced on demand by the
//! decrypt-provider (in its sandbox) and proxied straight to the viewer; the CEK/IV
//! never cross any boundary.
//!
//!   GET /api/viewers/:viewer/object/:session
//!        -> { schema:"elastos.viewer.object/v1", mime, byte_length, is_protected,
//!             expires_at }                                 (metadata only, NO key)
//!   GET /api/viewers/:viewer/object/:session/bytes
//!        -> decrypted object bytes; expired / unauthorized => 4xx (fail closed)

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::gateway::{require_home_launch_token_for_any_context, GatewayState};

/// The browser-facing object manifest schema (matches `ddrm-viewer`'s contract).
const OBJECT_MANIFEST_SCHEMA: &str = "elastos.viewer.object/v1";

/// The protected-content viewer interface a NON-MEDIA object declares, and the
/// viewer capsule that satisfies it.
pub const OBJECT_VIEWER_INTERFACE: &str = "elastos.viewer/document@1";
pub const OBJECT_VIEWER_CAPSULE: &str = "ddrm-viewer";

/// Build the in-shell view route for an object session. Home appends the launch
/// token (`home_token`) before handing it to the viewer iframe; the viewer reads
/// `session` + `home_token` from its query string. `session_id` is opaque + URL-safe.
pub fn object_view_route(session_id: &str) -> String {
    format!("/apps/{OBJECT_VIEWER_CAPSULE}/?session={session_id}")
}

/// Field names that must NEVER appear on an object route — their presence means key
/// material escaped a boundary, so we refuse rather than surface it.
const FORBIDDEN_OBJECT_FIELDS: &[&str] = &[
    "cek",
    "raw_cek",
    "wrapped_cek",
    "sealed_cek",
    "iv",
    "key",
    "private_key",
    "release_receipt",
    "kms_node_credentials",
    "provider_credentials",
    "wallet_rpc",
    "chain_rpc",
];

/// A live object decrypt session held by the runtime. Carries ONLY public metadata
/// plus a handle to the gateway-spawned local key-authority — never the CEK, never
/// the cleartext bytes (those are fetched on demand and streamed straight through).
pub struct ObjectSession {
    /// The viewer capsule authorized to read this session (e.g. `ddrm-viewer`).
    pub viewer: String,
    /// The owning principal; a launch token for any other principal is refused.
    pub principal_id: String,
    /// The asset's real content type.
    pub mime: String,
    /// The cleartext object length in bytes.
    pub byte_length: usize,
    /// Whether the asset is protected (always true on this path; surfaced for the UI).
    pub is_protected: bool,
    /// Pixel-lock: the asset is viewed as flattened, watermarked page images served by the
    /// page route; the raw `/bytes` egress is refused (the plaintext never leaves the boundary).
    pub pixel_locked: bool,
    /// For pixel-lock assets, the document's page count.
    pub total_pages: u32,
    /// For pixel-lock assets, the content type of each rendered page (e.g. `image/jpeg`).
    pub page_content_type: String,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
    /// The gateway-spawned LOCAL KEY-AUTHORITY subprocess (which owns a SEPARATE rail
    /// `decrypt-provider` boundary and sealed the CEK to it). The object read relays
    /// to this helper, which returns the already-decrypted bytes.
    pub authority: Arc<super::object_authority::ObjectAuthorityProc>,
}

impl ObjectSession {
    /// Build an object session served by a gateway-spawned local key-authority,
    /// deriving public metadata from the helper's key-free session descriptor.
    pub fn from_authority(
        viewer: impl Into<String>,
        principal_id: impl Into<String>,
        authority: Arc<super::object_authority::ObjectAuthorityProc>,
    ) -> Self {
        Self {
            viewer: viewer.into(),
            principal_id: principal_id.into(),
            mime: authority.mime.clone(),
            byte_length: authority.byte_length,
            is_protected: true,
            pixel_locked: authority.pixel_locked,
            total_pages: authority.total_pages,
            page_content_type: authority.page_content_type.clone(),
            expires_at: authority.expires_at,
            authority,
        }
    }
}

type SessionStore = Mutex<HashMap<String, Arc<ObjectSession>>>;

fn store() -> &'static SessionStore {
    static STORE: OnceLock<SessionStore> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register an object session under an opaque `session_id`. BOUNDED (Sprint 47): admission
/// first sweeps expired sessions and, at the cap, evicts the soonest-to-expire — see
/// `session_bounds` for the discipline (an evicted session's next read is a plain re-open;
/// eviction can never grant access). HONEST TEST GAP (council S47 guardian F5b): this store has
/// no wiring test — `ObjectSession.authority` is a non-optional `Arc<ObjectAuthorityProc>`
/// holding a real `Child`, so a session cannot be built without spawning; the wiring is
/// byte-identical to `viewer_media` (which has the test) and the policy is unit-ratcheted in
/// `session_bounds`.
pub fn put_object_session(session_id: impl Into<String>, session: ObjectSession) {
    // Deferred-drop contract (see `session_bounds`): removed sessions are dropped AFTER the lock
    // releases — a drop can reap an authority subprocess (~1s grace), which must never run under
    // the store Mutex where it would stall every viewer's lookup.
    let outcome = {
        let mut map = store().lock().expect("object session store poisoned");
        let outcome = super::session_bounds::sweep_and_make_room(
            &mut map,
            crate::auth::now_ts(),
            super::session_bounds::MAX_VIEWER_SESSIONS,
            |s| s.expires_at,
        );
        map.insert(session_id.into(), Arc::new(session));
        outcome
    };
    if outcome.live_evicted > 0 {
        tracing::warn!(
            evicted = outcome.live_evicted,
            cap = super::session_bounds::MAX_VIEWER_SESSIONS,
            "object session cap reached — evicted the soonest-to-expire live session(s); \
             sustained pressure here means abandoned viewers or an open loop"
        );
    }
    drop(outcome); // subprocess reaps happen here, off the lock
}

/// Drop an object session (on close/expiry). Returns true if a session was removed.
pub fn remove_object_session(session_id: &str) -> bool {
    // Deferred drop (S47 guardian F1): the removed Arc may be the last — its subprocess reap
    // (~1s grace) must run after the lock releases, not under it.
    let removed = store()
        .lock()
        .expect("object session store poisoned")
        .remove(session_id);
    let was_live = removed.is_some();
    drop(removed);
    was_live
}

/// Release every expired object session NOW (the periodic-sweeper hook). Same sweep the
/// lookup path runs lazily; this one fires on the clock so an idle machine cannot keep an
/// expired authority subprocess alive until the next access.
pub(super) fn sweep_object_sessions(now: u64) -> usize {
    let swept = {
        let mut map = store().lock().expect("object session store poisoned");
        super::session_bounds::sweep_expired(&mut map, now, &|s: &Arc<ObjectSession>| s.expires_at)
    };
    let released = swept.len();
    drop(swept); // subprocess reaps happen here, off the lock
    released
}

/// The object store's registration with the session-lifecycle foundation.
pub(super) struct ObjectSessionLifecycle;
pub(super) static OBJECT_SESSION_LIFECYCLE: ObjectSessionLifecycle = ObjectSessionLifecycle;

impl super::session_lifecycle::SessionLifecycle for ObjectSessionLifecycle {
    fn kind(&self) -> &'static str {
        "object"
    }
    fn close(&self, session_id: &str) -> bool {
        remove_object_session(session_id)
    }
    fn sweep(&self, now: u64) -> usize {
        sweep_object_sessions(now)
    }
}

fn get_object_session(session_id: &str) -> Option<Arc<ObjectSession>> {
    // Lazy sweep on lookup (Sprint 47): expired sessions release what they pin (the authority
    // subprocess) on the next access, not only on the next open. O(len ≤ cap) under the lock;
    // the DROPS (which may reap subprocesses) happen after the lock releases — deferred-drop
    // contract, see `session_bounds`.
    let (found, swept) = {
        let mut map = store().lock().expect("object session store poisoned");
        let now = crate::auth::now_ts();
        let swept = super::session_bounds::sweep_expired(&mut map, now, &|s: &Arc<
            ObjectSession,
        >| s.expires_at);
        (map.get(session_id).cloned(), swept)
    };
    drop(swept); // subprocess reaps happen here, off the lock
    found
}

/// GET /api/viewers/:viewer/object/:session — the view manifest (metadata only).
pub async fn viewer_object_manifest(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_object_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    let manifest = object_manifest_value(&session);
    if assert_no_key_material(&manifest).is_err() {
        return object_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "object manifest carried key material",
        );
    }
    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(manifest),
    )
        .into_response()
}

/// POST /api/viewers/:viewer/object/:session/close — explicit release of an object session
/// (the viewer's `pagehide` beacon and the Home shell's window-close hook land here).
///
/// Same authorization gate as the reads (token + viewer + principal). IDEMPOTENT and
/// non-leaking: "closed", "already gone", and "not yours" all answer 204 — a close can only
/// cost a re-open, and distinguishing those outcomes would let a token holder probe other
/// principals' session ids. Only a missing/invalid token (401) or a malformed viewer (400)
/// is surfaced.
pub async fn viewer_object_close(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    match authorize_object_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(_session) => {
            super::session_lifecycle::close("object", &session_id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(resp) if resp.status() == StatusCode::NOT_FOUND => {
            // Gone (already closed / swept) or not owned — identical 204, no probe signal.
            StatusCode::NO_CONTENT.into_response()
        }
        Err(resp) => *resp,
    }
}

/// GET /api/viewers/:viewer/object/:session/bytes — decrypted object bytes.
pub async fn viewer_object_bytes(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_object_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    // Pixel-lock assets (e.g. PDF) never egress their raw plaintext — only watermarked page
    // images, via the page route. Refuse the raw bytes path fail-closed (one canonical path).
    if session.pixel_locked {
        return object_error(
            StatusCode::FORBIDDEN,
            "this asset is pixel-locked; fetch rendered pages via /page?n=",
        );
    }
    if crate::auth::now_ts() > session.expires_at {
        return object_error(StatusCode::FORBIDDEN, "this object session has expired");
    }
    let authority = session.authority.clone();
    let expected = session.byte_length;
    let read = tokio::task::spawn_blocking(move || authority.object()).await;
    match read {
        Ok(Ok(bytes)) => {
            if bytes.len() != expected {
                tracing::warn!(
                    expected,
                    got = bytes.len(),
                    "object byte length mismatch — fail closed"
                );
                return object_error(
                    StatusCode::BAD_GATEWAY,
                    "the decrypt provider returned an unexpected object length",
                );
            }
            // ③ serve-time content sniff (defence-in-depth over the mint-time `pixel_locked`
            // flag, which trusts the creator-declared mime): a renderable/scriptable document
            // mislabeled with a non-pixel-lock mime would otherwise egress here as raw plaintext.
            // If the decrypted bytes LOOK like a pixel-lockable type, fail closed. The one
            // exception is an explicitly-declared generic `application/zip` archive — a CBZ/EPUB
            // sniffs as zip too, so we only honour a raw zip the creator labelled as such.
            if let Some(sniffed) = sniffs_as_lockable(&bytes) {
                let declared = session.mime.trim().to_ascii_lowercase();
                let explicit_zip = sniffed == "application/zip" && declared == "application/zip";
                if !explicit_zip {
                    tracing::warn!(
                        declared = %declared,
                        sniffed,
                        "object content sniffs as a pixel-lockable type but was routed to the raw \
                         bytes path — fail closed (mislabeled asset)"
                    );
                    return object_error(
                        StatusCode::FORBIDDEN,
                        "this asset's content requires pixel-lock; fetch rendered pages via /page?n=",
                    );
                }
            }
            octet_stream(bytes, &session.mime)
        }
        Ok(Err(err)) => {
            tracing::warn!("object read fail-closed: {err}");
            object_error(
                StatusCode::BAD_GATEWAY,
                "the decrypt provider could not serve this object",
            )
        }
        Err(_) => object_error(
            StatusCode::BAD_GATEWAY,
            "the decrypt provider could not serve this object",
        ),
    }
}

/// GET /api/viewers/:viewer/object/:session/page?n=N — one rendered, watermarked page image
/// for a pixel-lock asset. The raw object never reaches the browser; only this image does.
/// `X-Asset-Pages` carries the page count so the viewer can page through. Fails closed for
/// non-pixel-lock sessions, expiry, and any render/relay error.
pub async fn viewer_object_page(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_object_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    if !session.pixel_locked {
        return object_error(
            StatusCode::BAD_REQUEST,
            "this asset is not pixel-locked; fetch it via /bytes",
        );
    }
    if crate::auth::now_ts() > session.expires_at {
        return object_error(StatusCode::FORBIDDEN, "this object session has expired");
    }
    let n: u32 = params.get("n").and_then(|v| v.parse().ok()).unwrap_or(0);
    if session.total_pages > 0 && n >= session.total_pages {
        return object_error(StatusCode::NOT_FOUND, "page out of range");
    }
    let authority = session.authority.clone();
    let page_mime = session.page_content_type.clone();
    let rendered = tokio::task::spawn_blocking(move || authority.object_page(n)).await;
    match rendered {
        Ok(Ok((bytes, total_pages))) => image_page(bytes, &page_mime, total_pages, n),
        Ok(Err(err)) => {
            tracing::warn!("object page render fail-closed: {err}");
            object_error(
                StatusCode::BAD_GATEWAY,
                "the decrypt provider could not render this page",
            )
        }
        Err(_) => object_error(
            StatusCode::BAD_GATEWAY,
            "the decrypt provider could not render this page",
        ),
    }
}

/// Compose the browser-facing view manifest — metadata ONLY, never key material.
fn object_manifest_value(session: &ObjectSession) -> Value {
    json!({
        "schema": OBJECT_MANIFEST_SCHEMA,
        "mime": session.mime,
        "byte_length": session.byte_length,
        "is_protected": session.is_protected,
        "pixel_locked": session.pixel_locked,
        "total_pages": session.total_pages,
        "page_content_type": session.page_content_type,
        "expires_at": session.expires_at,
    })
}

/// Authorize an object read: the path viewer must be a real viewer capsule, the
/// request must carry a valid Home launch token FOR that viewer, the session must
/// exist + be served by that viewer, and the token principal must own the session.
fn authorize_object_session(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    session_id: &str,
) -> Result<Arc<ObjectSession>, Box<Response>> {
    let viewer = clean_capsule_ref(viewer)
        .map_err(|_| Box::new(object_error(StatusCode::BAD_REQUEST, "invalid viewer")))?;
    // An object session is created ONLY by the open path, which resolved + validated the viewer
    // capsule at creation and bound it into `session.viewer`. On a bytes/page request that proof is
    // already in hand: the home-token below is scoped to `viewer` and the `session.viewer` match
    // seals it — so we skip re-resolving the capsule manifest from disk on every read. (Trade-off:
    // an active view keeps serving if the viewer capsule is uninstalled mid-session; the principal
    // still owns the content and the token + session + principal gates below are unchanged.)
    let context = require_home_launch_token_for_any_context(data_dir, headers, &[viewer.as_str()])
        .map_err(|err| {
            tracing::warn!(
                "authorize_object_session: launch-token rejected for viewer='{viewer}' session='{session_id}' (401): {err}"
            );
            Box::new(object_error(
                StatusCode::UNAUTHORIZED,
                "missing or invalid home launch token",
            ))
        })?;
    let Some(session) = get_object_session(session_id) else {
        return Err(Box::new(object_error(
            StatusCode::NOT_FOUND,
            "object session not found",
        )));
    };
    if session.viewer != viewer || session.principal_id != context.principal_id {
        // Never reveal another principal's / viewer's session exists — same 404 shape.
        return Err(Box::new(object_error(
            StatusCode::NOT_FOUND,
            "object session not found",
        )));
    }
    Ok(session)
}

/// Recursively refuse a value that carries any forbidden key field.
fn assert_no_key_material(value: &Value) -> anyhow::Result<()> {
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if FORBIDDEN_OBJECT_FIELDS
                        .iter()
                        .any(|forbidden| key.eq_ignore_ascii_case(forbidden))
                    {
                        anyhow::bail!("forbidden key field on object route: {key}");
                    }
                    stack.push(child);
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    Ok(())
}

fn octet_stream(bytes: Vec<u8>, mime: &str) -> Response {
    // The viewer renders by the manifest mime; we still send octet-stream + no-store
    // so the bytes are never cached or content-sniffed into an active context.
    let _ = mime;
    (
        StatusCode::OK,
        [
            ("content-type", "application/octet-stream"),
            ("cache-control", "no-store"),
        ],
        bytes,
    )
        .into_response()
}

/// Best-effort magic-byte sniff of DECRYPTED object bytes: does the content look like a type that
/// MUST be pixel-locked (a renderable/scriptable document)? The serve-time backstop for a
/// mislabeled asset — every type below is in `is_pixel_lock`, so its bytes reaching the RAW path
/// means the declared mime lied. Pure (no `image` dependency) so it adds no footprint to the
/// gateway. Returns the sniffed family (`application/pdf` / `application/zip` / `image/*` /
/// `image/svg+xml`), or `None` for genuinely non-renderable bytes (3D models, media, arbitrary).
///
/// Fail-closed bias: a false positive merely refuses a RAW byte download (the creator relabels and
/// the asset gets pixel-locked); a false negative would leak source. We err toward refusing.
fn sniffs_as_lockable(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.len() >= sig.len() && &bytes[..sig.len()] == sig;

    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    // ZIP container — CBZ / EPUB / any zip-based document (local-file / central-dir / spanned sig).
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
        return Some("application/zip");
    }
    // Raster image signatures. (BMP's `BM` is only 2 bytes — the weakest signal — so we also
    // require a plausible header length to avoid matching short non-image blobs.)
    if starts(b"\x89PNG\r\n\x1a\n")                                  // PNG
        || starts(b"\xFF\xD8\xFF")                                  // JPEG
        || starts(b"GIF87a") || starts(b"GIF89a")                   // GIF
        || starts(b"II*\x00") || starts(b"MM\x00*")                 // TIFF (little/big-endian)
        || (starts(b"BM") && bytes.len() >= 26)                     // BMP
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
    // WebP
    {
        return Some("image/*");
    }
    // SVG / XML (scriptable markup) — skip a UTF-8 BOM + leading whitespace, then look for the tag.
    let mut s = bytes;
    if s.starts_with(b"\xEF\xBB\xBF") {
        s = &s[3..];
    }
    let nws = s.iter().take_while(|b| b.is_ascii_whitespace()).count();
    let s = &s[nws..];
    if s.starts_with(b"<?xml") || s.starts_with(b"<svg") {
        return Some("image/svg+xml");
    }
    None
}

/// A rendered page image response: the watermarked bytes, the page's content type, no-store,
/// and `X-Asset-Pages`/`X-Asset-Page` so the viewer can build its pager. The bytes are an
/// opaque flattened image — never the source document.
fn image_page(bytes: Vec<u8>, content_type: &str, total_pages: u32, page_index: u32) -> Response {
    let content_type = if content_type.is_empty() {
        "image/jpeg".to_string()
    } else {
        content_type.to_string()
    };
    // ⑤ The EPUB "html-lock" tier serves sanitised chapter HTML. Enforce the sandbox at the
    // RESOURCE level with an HTTP `Content-Security-Policy` (not merely the inline `<meta>` + the
    // viewer's iframe attribute): even if this document is loaded directly or framed without
    // `sandbox`, the `sandbox` directive means no script runs, no same-origin access, no form
    // post, and `default-src 'none'` means no network — so the hand-rolled sanitiser is strictly
    // defence-in-depth BEHIND this, not the sole containment. JPEG pages get no CSP (an image
    // never executes) but DO get `nosniff` so a response is never content-sniffed into an active
    // type.
    if content_type.starts_with("text/html") {
        return (
            StatusCode::OK,
            [
                ("content-type", content_type),
                ("cache-control", "no-store".to_string()),
                ("x-asset-pages", total_pages.to_string()),
                ("x-asset-page", page_index.to_string()),
                ("x-content-type-options", "nosniff".to_string()),
                ("referrer-policy", "no-referrer".to_string()),
                (
                    "content-security-policy",
                    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; \
                     base-uri 'none'; form-action 'none'; frame-ancestors 'self'; sandbox"
                        .to_string(),
                ),
            ],
            bytes,
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [
            ("content-type", content_type),
            ("cache-control", "no-store".to_string()),
            ("x-asset-pages", total_pages.to_string()),
            ("x-asset-page", page_index.to_string()),
            ("x-content-type-options", "nosniff".to_string()),
        ],
        bytes,
    )
        .into_response()
}

fn object_error(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

fn clean_capsule_ref(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        anyhow::bail!("invalid capsule reference");
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_view_route_targets_the_viewer_with_the_session() {
        let route = object_view_route("sess-abc123");
        assert_eq!(route, "/apps/ddrm-viewer/?session=sess-abc123");
        assert!(route.contains('?'));
    }

    #[test]
    fn key_material_anywhere_in_a_response_fails_closed() {
        assert!(assert_no_key_material(&json!({ "byte_length": 10, "cek": "leak" })).is_err());
        assert!(assert_no_key_material(&json!({ "a": { "b": { "raw_cek": "leak" } } })).is_err());
        assert!(assert_no_key_material(&json!({ "items": [ { "iv": "leak" } ] })).is_err());
        assert!(assert_no_key_material(&json!({ "CEK": "leak" })).is_err());
        assert!(assert_no_key_material(&json!({
            "schema": OBJECT_MANIFEST_SCHEMA,
            "mime": "image/png",
            "byte_length": 1234
        }))
        .is_ok());
    }

    #[test]
    fn clean_capsule_ref_rejects_traversal() {
        assert!(clean_capsule_ref("ddrm-viewer").is_ok());
        assert!(clean_capsule_ref("../secret").is_err());
        assert!(clean_capsule_ref("a/b").is_err());
        assert!(clean_capsule_ref("").is_err());
        assert!(clean_capsule_ref("..").is_err());
    }

    #[test]
    fn sniff_flags_renderable_document_types() {
        assert_eq!(
            sniffs_as_lockable(b"%PDF-1.7\n..."),
            Some("application/pdf")
        );
        assert_eq!(
            sniffs_as_lockable(b"PK\x03\x04rest"),
            Some("application/zip")
        );
        assert_eq!(
            sniffs_as_lockable(b"\x89PNG\r\n\x1a\n...."),
            Some("image/*")
        );
        assert_eq!(sniffs_as_lockable(b"\xFF\xD8\xFF\xE0...."), Some("image/*"));
        assert_eq!(sniffs_as_lockable(b"GIF89a...."), Some("image/*"));
        let mut webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        webp.extend_from_slice(&[0u8; 8]);
        assert_eq!(sniffs_as_lockable(&webp), Some("image/*"));
        assert_eq!(
            sniffs_as_lockable(b"  <?xml version=\"1.0\"?><svg/>"),
            Some("image/svg+xml")
        );
        assert_eq!(
            sniffs_as_lockable(b"<svg xmlns=...>"),
            Some("image/svg+xml")
        );
        // UTF-8 BOM before the markup must not hide it.
        assert_eq!(
            sniffs_as_lockable(b"\xEF\xBB\xBF<svg>"),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn html_lock_page_enforces_sandbox_csp_and_nosniff() {
        let resp = image_page(
            b"<html><body>chapter</body></html>".to_vec(),
            "text/html; charset=utf-8",
            5,
            1,
        );
        let h = resp.headers();
        let csp = h
            .get("content-security-policy")
            .expect("html-lock must carry an HTTP CSP")
            .to_str()
            .unwrap();
        assert!(
            csp.contains("sandbox"),
            "CSP must enforce the sandbox at the resource level"
        );
        assert!(
            csp.contains("default-src 'none'"),
            "CSP must lock default-src to none"
        );
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
    }

    #[test]
    fn image_page_has_nosniff_but_no_csp() {
        let resp = image_page(vec![0xFF, 0xD8, 0xFF], "image/jpeg", 1, 0);
        let h = resp.headers();
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert!(
            h.get("content-security-policy").is_none(),
            "a flattened JPEG never executes; it needs no CSP"
        );
    }

    #[test]
    fn sniff_passes_genuinely_non_renderable_bytes() {
        // glTF binary (3D) and JSON glTF are legit raw passthrough — must NOT be flagged.
        assert_eq!(sniffs_as_lockable(b"glTF\x02\x00\x00\x00"), None);
        assert_eq!(
            sniffs_as_lockable(b"{\"asset\":{\"version\":\"2.0\"}}"),
            None
        );
        // An fMP4 media segment (box-size dword + `ftyp`) is not a document.
        assert_eq!(sniffs_as_lockable(b"\x00\x00\x00\x18ftypisom"), None);
        // Short non-image blob starting with "BM" must not trip the BMP magic.
        assert_eq!(sniffs_as_lockable(b"BMP file? no."), None);
        assert_eq!(sniffs_as_lockable(b""), None);
        assert_eq!(sniffs_as_lockable(b"just some text"), None);
    }
}
