//! Scoped media stream routes (B2 of the dDRM viewer seam).
//!
//! These routes are how the `elacity-player` viewer capsule plays an owned,
//! protected video without ever touching key material. The runtime holds, per open
//! decrypt session, ONLY the CEK-free sealed material + the clear init segment +
//! public metadata (mime/segment_count/expiry). It never holds the CEK and never
//! holds decrypted media; each media segment is decrypted on demand by the
//! decrypt-provider (the `stream_segment` op, feature `rail-stream`) and proxied
//! straight to the player's MSE SourceBuffer.
//!
//!   GET /api/viewers/:viewer/media/:session
//!        -> { schema:"elastos.viewer.media/v1", mime, segment_count, has_init,
//!             is_protected, expires_at }                  (metadata only, NO key)
//!   GET /api/viewers/:viewer/media/:session/init
//!        -> clear init segment bytes (CENC init is unencrypted)
//!   GET /api/viewers/:viewer/media/:session/segment/:index
//!        -> decrypted media segment bytes; out-of-range / expired / unauthorized
//!           => 4xx (fail closed)
//!
//! Containment invariant (mirrors PC2 + the decrypt boundary): the CEK/IV never
//! cross any boundary; only one segment's plaintext is ever in flight; and the
//! runtime additionally fail-closes if the decrypt-provider ever returned a key
//! field on these routes (defense in depth over the boundary's own guarantee).

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use elastos_runtime::provider::ProviderRegistry;
use serde_json::{json, Value};

use super::gateway::{require_home_launch_token_for_any_context, GatewayState};

/// The browser-facing media manifest schema (matches `elacity-player`'s contract).
const MEDIA_MANIFEST_SCHEMA: &str = "elastos.viewer.media/v1";

/// The protected-content viewer interface a media (video/audio) object declares,
/// and the viewer capsule that satisfies it. A sealed object's
/// `viewer.required_interface` is the single source of truth for which viewer plays
/// it (Anders' "trust + access travel with signed content").
pub const MEDIA_VIEWER_INTERFACE: &str = "elastos.viewer/media@1";
pub const MEDIA_VIEWER_CAPSULE: &str = "elacity-player";

/// Map a sealed object's `viewer.required_interface` to the viewer capsule that
/// plays it. Media maps to `elacity-player`; any other interface returns `None`
/// (documents/archives route through the existing viewer-bound path, and an unknown
/// interface fails closed rather than guessing a viewer).
pub fn viewer_for_required_interface(required_interface: &str) -> Option<&'static str> {
    match required_interface.trim() {
        MEDIA_VIEWER_INTERFACE => Some(MEDIA_VIEWER_CAPSULE),
        _ => None,
    }
}

/// Build the in-shell launch route for a media session. Home appends the launch
/// token (`home_token`) to this route before handing it to the player iframe; the
/// player reads `session` + `home_token` from its query string. `session_id` is an
/// opaque, URL-safe runtime token (no user input), so it needs no escaping.
pub fn media_play_route(session_id: &str) -> String {
    format!("/apps/{MEDIA_VIEWER_CAPSULE}/?session={session_id}")
}

/// Field names that must NEVER appear on a media route — their presence means key
/// material or raw content escaped a boundary, so we refuse rather than surface it.
const FORBIDDEN_MEDIA_FIELDS: &[&str] = &[
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

/// A live media decrypt session held by the runtime. Carries ONLY CEK-free sealed
/// material + the clear init segment + public metadata — never the CEK, never
/// decrypted media segments.
pub struct MediaSession {
    /// The viewer capsule authorized to read this session (e.g. `elacity-player`).
    pub viewer: String,
    /// The owning principal; a launch token for any other principal is refused.
    pub principal_id: String,
    /// MSE `addSourceBuffer` mime/codecs string (from object metadata, not the CEK).
    pub mime: String,
    /// Number of media segments addressable as `/segment/{0..segment_count}`.
    pub segment_count: usize,
    /// Whether a clear init segment is available at `/init`.
    pub has_init: bool,
    /// The clear init segment bytes (CENC init/`moov` is unencrypted).
    pub init_bytes: Vec<u8>,
    /// Whether the asset is protected (always true on this path; surfaced for the UI).
    pub is_protected: bool,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
    /// The decrypt-provider line-protocol request to relay per segment.
    pub decrypt_request: Value,
    /// The CEK-free sealed decrypt material relayed per segment.
    pub sealed_material: Value,
    /// When set, this session is served by a gateway-spawned LOCAL KEY-AUTHORITY
    /// subprocess (which owns a SEPARATE rail `decrypt-provider` boundary and
    /// sealed the CEK to it). Segment reads relay to that helper, which returns
    /// already-decrypted, `senc`-stripped bytes. The CEK never reaches the gateway.
    pub authority: Option<Arc<super::media_authority::MediaAuthorityProc>>,
}

impl MediaSession {
    /// Build a media session served by a gateway-spawned local key-authority,
    /// deriving public metadata from the helper's key-free session descriptor.
    pub fn from_authority(
        viewer: impl Into<String>,
        principal_id: impl Into<String>,
        authority: Arc<super::media_authority::MediaAuthorityProc>,
    ) -> Self {
        Self {
            viewer: viewer.into(),
            principal_id: principal_id.into(),
            mime: authority.mime.clone(),
            segment_count: authority.segment_count,
            has_init: !authority.init_bytes.is_empty(),
            init_bytes: authority.init_bytes.clone(),
            is_protected: true,
            expires_at: authority.expires_at,
            decrypt_request: Value::Null,
            sealed_material: Value::Null,
            authority: Some(authority),
        }
    }
}

type SessionStore = Mutex<HashMap<String, Arc<MediaSession>>>;

fn store() -> &'static SessionStore {
    static STORE: OnceLock<SessionStore> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a media session under an opaque `session_id` (called by the Library
/// open orchestration once the decrypt session is established — B3).
pub fn put_media_session(session_id: impl Into<String>, session: MediaSession) {
    store()
        .lock()
        .expect("media session store poisoned")
        .insert(session_id.into(), Arc::new(session));
}

/// Drop a media session (on close/expiry) so its sealed material is no longer held.
pub fn remove_media_session(session_id: &str) {
    store()
        .lock()
        .expect("media session store poisoned")
        .remove(session_id);
}

fn get_media_session(session_id: &str) -> Option<Arc<MediaSession>> {
    store()
        .lock()
        .expect("media session store poisoned")
        .get(session_id)
        .cloned()
}

/// GET /api/viewers/:viewer/media/:session — the play manifest (metadata only).
pub async fn viewer_media_manifest(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_media_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    let manifest = media_manifest_value(&session);
    // Defense in depth: never emit a manifest that somehow carries key material.
    if assert_no_key_material(&manifest).is_err() {
        return media_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "media manifest carried key material",
        );
    }
    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(manifest),
    )
        .into_response()
}

/// GET /api/viewers/:viewer/media/:session/init — clear init segment bytes.
pub async fn viewer_media_init(
    State(state): State<GatewayState>,
    Path((viewer, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_media_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    if !session.has_init || session.init_bytes.is_empty() {
        return media_error(StatusCode::NOT_FOUND, "this stream has no init segment");
    }
    octet_stream(session.init_bytes.clone())
}

/// GET /api/viewers/:viewer/media/:session/segment/:index — decrypted segment bytes.
pub async fn viewer_media_segment(
    State(state): State<GatewayState>,
    Path((viewer, session_id, index)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize_media_session(&state.data_dir, &headers, &viewer, &session_id) {
        Ok(session) => session,
        Err(resp) => return *resp,
    };
    let index: usize = match index.parse() {
        Ok(index) => index,
        Err(_) => {
            return media_error(
                StatusCode::BAD_REQUEST,
                "segment index must be a non-negative integer",
            )
        }
    };
    // Range + expiry are enforced BEFORE any decrypt relay — fail closed.
    if let Err(resp) = check_segment_readable(&session, index, crate::auth::now_ts()) {
        return *resp;
    }
    // Gateway-spawned local key-authority (the dDRM viewer seam): relay the read to
    // the helper, which decrypts in-VM through its own boundary and strips `senc`.
    if let Some(authority) = session.authority.clone() {
        let decrypted = tokio::task::spawn_blocking(move || authority.segment(index)).await;
        return match decrypted {
            Ok(Ok(bytes)) => octet_stream(bytes),
            Ok(Err(err)) => {
                tracing::warn!(index, "media segment fail-closed: {err}");
                media_error(
                    StatusCode::BAD_GATEWAY,
                    "the decrypt provider could not serve this segment",
                )
            }
            Err(_) => media_error(
                StatusCode::BAD_GATEWAY,
                "the decrypt provider could not serve this segment",
            ),
        };
    }
    let Some(registry) = state.provider_registry.as_ref() else {
        return media_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "decrypt provider unavailable",
        );
    };
    match relay_decrypted_segment(registry, &session, index).await {
        Ok(bytes) => octet_stream(bytes),
        // A substituted/expired/unauthorized read, or an unconfigured decrypt
        // backend, all land here — refuse rather than surface a partial stream.
        Err(_) => media_error(
            StatusCode::BAD_GATEWAY,
            "the decrypt provider could not serve this segment",
        ),
    }
}

/// Compose the browser-facing play manifest — metadata ONLY, never key material.
fn media_manifest_value(session: &MediaSession) -> Value {
    json!({
        "schema": MEDIA_MANIFEST_SCHEMA,
        "mime": session.mime,
        "segment_count": session.segment_count,
        "has_init": session.has_init,
        "is_protected": session.is_protected,
        "expires_at": session.expires_at,
    })
}

/// Enforce the per-segment read guard: in-range index AND a live (un-expired) grant.
/// Returns a fail-closed response on violation; `Ok(())` when the read may proceed.
fn check_segment_readable(
    session: &MediaSession,
    index: usize,
    now: u64,
) -> Result<(), Box<Response>> {
    if now > session.expires_at {
        return Err(Box::new(media_error(
            StatusCode::FORBIDDEN,
            "this media session has expired",
        )));
    }
    if index >= session.segment_count {
        return Err(Box::new(media_error(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "segment index is past the end of the asset",
        )));
    }
    Ok(())
}

/// Relay one segment read to the decrypt-provider (`stream_segment` op) and return
/// its decrypted bytes. The runtime holds only the CEK-free sealed material; the
/// provider unwraps the CEK in-VM, decrypts ONLY this segment, and returns its
/// bytes. We additionally fail closed if the provider response carries any key
/// field (it must not).
async fn relay_decrypted_segment(
    registry: &ProviderRegistry,
    session: &MediaSession,
    index: usize,
) -> anyhow::Result<Vec<u8>> {
    let request = json!({
        "op": "stream_segment",
        "request": session.decrypt_request,
        "material": session.sealed_material,
        "index": index,
        "now_unix": crate::auth::now_ts(),
    });
    let response = registry
        .send_raw("decrypt", &request)
        .await
        .map_err(|err| anyhow::anyhow!("decrypt provider unavailable: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("decrypt provider returned error");
        anyhow::bail!("decrypt provider rejected stream read: {message}");
    }
    let data = response
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("stream read response missing data"))?;
    assert_no_key_material(data)?;
    let segment_b64 = data
        .get("segment_b64")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("stream read response missing segment_b64"))?;
    base64::engine::general_purpose::STANDARD
        .decode(segment_b64)
        .map_err(|_| anyhow::anyhow!("stream read returned invalid base64"))
}

/// Authorize a media read: the path viewer must be a real viewer capsule, the
/// request must carry a valid Home launch token FOR that viewer, the session must
/// exist + be served by that viewer, and the token principal must own the session.
fn authorize_media_session(
    data_dir: &FsPath,
    headers: &HeaderMap,
    viewer: &str,
    session_id: &str,
) -> Result<Arc<MediaSession>, Box<Response>> {
    let viewer = clean_capsule_ref(viewer)
        .map_err(|_| Box::new(media_error(StatusCode::BAD_REQUEST, "invalid viewer")))?;
    if !super::browser_capsules::is_viewer_capsule(data_dir, &viewer) {
        return Err(Box::new(media_error(
            StatusCode::NOT_FOUND,
            "viewer capsule not found",
        )));
    }
    let context = require_home_launch_token_for_any_context(data_dir, headers, &[viewer.as_str()])
        .map_err(|_| {
            Box::new(media_error(
                StatusCode::UNAUTHORIZED,
                "missing or invalid home launch token",
            ))
        })?;
    let Some(session) = get_media_session(session_id) else {
        return Err(Box::new(media_error(
            StatusCode::NOT_FOUND,
            "media session not found",
        )));
    };
    if session.viewer != viewer {
        return Err(Box::new(media_error(
            StatusCode::NOT_FOUND,
            "media session not found",
        )));
    }
    if session.principal_id != context.principal_id {
        // Never reveal another principal's session exists — same 404 shape.
        return Err(Box::new(media_error(
            StatusCode::NOT_FOUND,
            "media session not found",
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
                    if FORBIDDEN_MEDIA_FIELDS
                        .iter()
                        .any(|forbidden| key.eq_ignore_ascii_case(forbidden))
                    {
                        anyhow::bail!("forbidden key field on media route: {key}");
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

fn octet_stream(bytes: Vec<u8>) -> Response {
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

fn media_error(status: StatusCode, message: &str) -> Response {
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

    fn session(segment_count: usize, expires_at: u64) -> MediaSession {
        MediaSession {
            viewer: "elacity-player".to_string(),
            principal_id: "principal:test".to_string(),
            mime: "video/mp4; codecs=\"avc1.640028, mp4a.40.2\"".to_string(),
            segment_count,
            has_init: true,
            init_bytes: vec![0x00, 0x01, 0x02],
            is_protected: true,
            expires_at,
            decrypt_request: json!({ "session_id": "s1", "output_kind": "stream" }),
            sealed_material: json!({ "suite": "elastos-pq-hybrid-threshold-v0" }),
            authority: None,
        }
    }

    #[test]
    fn manifest_is_metadata_only_and_carries_no_key_material() {
        let s = session(3, 9_000_000_000);
        let manifest = media_manifest_value(&s);
        assert_eq!(manifest["schema"], json!(MEDIA_MANIFEST_SCHEMA));
        assert_eq!(manifest["mime"], json!(s.mime));
        assert_eq!(manifest["segment_count"], json!(3));
        assert_eq!(manifest["has_init"], json!(true));
        assert_eq!(manifest["is_protected"], json!(true));
        assert!(
            assert_no_key_material(&manifest).is_ok(),
            "manifest must be key-free"
        );
    }

    #[test]
    fn segment_read_guard_fails_closed_out_of_range_and_expired() {
        let s = session(2, 9_000_000_000);
        // In range, live grant: readable.
        assert!(check_segment_readable(&s, 0, 1_000).is_ok());
        assert!(check_segment_readable(&s, 1, 1_000).is_ok());
        // Out of range: refused.
        assert!(
            check_segment_readable(&s, 2, 1_000).is_err(),
            "index == count is out of range"
        );
        assert!(check_segment_readable(&s, 99, 1_000).is_err());
        // Expired grant: refused even for a valid index.
        let expired = session(2, 100);
        assert!(
            check_segment_readable(&expired, 0, 1_000).is_err(),
            "an expired grant is refused"
        );
    }

    #[test]
    fn key_material_anywhere_in_a_response_fails_closed() {
        // Top-level forbidden field.
        assert!(assert_no_key_material(&json!({ "segment_b64": "AAAA", "cek": "leak" })).is_err());
        // Nested forbidden field.
        assert!(assert_no_key_material(&json!({ "a": { "b": { "raw_cek": "leak" } } })).is_err());
        // Inside an array.
        assert!(assert_no_key_material(&json!({ "items": [ { "iv": "leak" } ] })).is_err());
        // Case-insensitive.
        assert!(assert_no_key_material(&json!({ "CEK": "leak" })).is_err());
        // A clean segment response passes.
        assert!(assert_no_key_material(&json!({
            "schema": "elastos.decrypt.stream-segment/v1",
            "segment_index": 0,
            "segment_count": 2,
            "segment_b64": "AAAA"
        }))
        .is_ok());
    }

    #[test]
    fn store_round_trips_and_removes() {
        let id = "session-store-test-unique";
        assert!(get_media_session(id).is_none());
        put_media_session(id, session(1, 9_000_000_000));
        assert!(get_media_session(id).is_some());
        remove_media_session(id);
        assert!(get_media_session(id).is_none());
    }

    #[test]
    fn media_interface_maps_to_the_player_and_unknowns_fail_closed() {
        assert_eq!(
            viewer_for_required_interface(MEDIA_VIEWER_INTERFACE),
            Some(MEDIA_VIEWER_CAPSULE)
        );
        // Tolerates surrounding whitespace from a manifest field.
        assert_eq!(
            viewer_for_required_interface("  elastos.viewer/media@1  "),
            Some("elacity-player")
        );
        // Non-media / unknown interfaces are not routed to the player.
        assert_eq!(
            viewer_for_required_interface("elastos.viewer/document@1"),
            None
        );
        assert_eq!(viewer_for_required_interface(""), None);
        assert_eq!(
            viewer_for_required_interface("elastos.viewer/media@2"),
            None
        );
    }

    #[test]
    fn media_play_route_targets_the_player_with_the_session() {
        let route = media_play_route("sess-abc123");
        assert_eq!(route, "/apps/elacity-player/?session=sess-abc123");
        // The route carries a query, so the launch-token machinery appends with `&`.
        assert!(route.contains('?'));
    }

    #[test]
    fn clean_capsule_ref_rejects_traversal() {
        assert!(clean_capsule_ref("elacity-player").is_ok());
        assert!(clean_capsule_ref("../secret").is_err());
        assert!(clean_capsule_ref("a/b").is_err());
        assert!(clean_capsule_ref("").is_err());
        assert!(clean_capsule_ref("..").is_err());
    }
}
