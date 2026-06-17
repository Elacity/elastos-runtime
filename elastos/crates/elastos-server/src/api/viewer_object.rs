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

/// Register an object session under an opaque `session_id`.
pub fn put_object_session(session_id: impl Into<String>, session: ObjectSession) {
    store()
        .lock()
        .expect("object session store poisoned")
        .insert(session_id.into(), Arc::new(session));
}

/// Drop an object session (on close/expiry).
pub fn remove_object_session(session_id: &str) {
    store()
        .lock()
        .expect("object session store poisoned")
        .remove(session_id);
}

fn get_object_session(session_id: &str) -> Option<Arc<ObjectSession>> {
    store()
        .lock()
        .expect("object session store poisoned")
        .get(session_id)
        .cloned()
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
    if !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        return Err(Box::new(object_error(
            StatusCode::NOT_FOUND,
            "viewer capsule not found",
        )));
    }
    let context = require_home_launch_token_for_any_context(data_dir, headers, &[viewer.as_str()])
        .map_err(|_| {
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

/// A rendered page image response: the watermarked bytes, the page's content type, no-store,
/// and `X-Asset-Pages`/`X-Asset-Page` so the viewer can build its pager. The bytes are an
/// opaque flattened image — never the source document.
fn image_page(bytes: Vec<u8>, content_type: &str, total_pages: u32, page_index: u32) -> Response {
    let content_type = if content_type.is_empty() {
        "image/jpeg".to_string()
    } else {
        content_type.to_string()
    };
    (
        StatusCode::OK,
        [
            ("content-type", content_type),
            ("cache-control", "no-store".to_string()),
            ("x-asset-pages", total_pages.to_string()),
            ("x-asset-page", page_index.to_string()),
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
}
