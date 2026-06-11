//! Library-bound owned-object open (the dDRM viewer seam, wired to REAL content).
//!
//! This is the production seam the demo `media_authority` / `object_authority` opens
//! prefigured: instead of a sample asset, it opens an object the signed-in principal
//! actually owns from their Library, routes it to the correct viewer by content type,
//! binds the decrypt session to that object's content identity, and returns a view URL.
//!
//!   Home (Library open) --POST /api/viewers/open { uri }-->
//!     gateway resolves the URI inside the principal's OWN library root (the local-
//!     sovereign ownership gate — a not-owned / traversal URI never resolves), reads
//!     the PLAINTEXT bytes, picks media -> elacity-player or non-media -> ddrm-viewer,
//!     spawns the local key-authority bound to the object's content CID (-> a SEPARATE
//!     decrypt-provider boundary), registers a principal-bound session, mints the
//!     viewer's launch token, and returns { viewer, session, play_url }.
//!
//! Ownership today is the local-sovereign gate (the principal's own root). The chain
//! rights-provider (`has_access_by_content_id`) is the production gate for *purchased*
//! access and lands with the live-chain slice; this seam already passes the object's
//! content CID through the transcript so that gate slots in without a contract change.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;

use super::gateway::{
    issue_home_launch_token_with_context, require_home_token_context, GatewayState,
    HomeLaunchTokenContext,
};
use super::media_authority::{resolve_decrypt_bin, resolve_helper_bin, MediaAuthorityProc};
use super::object_authority::ObjectAuthorityProc;
use super::viewer_media::{media_play_route, put_media_session, MediaSession, MEDIA_VIEWER_CAPSULE};
use super::viewer_object::{
    object_view_route, put_object_session, ObjectSession, OBJECT_VIEWER_CAPSULE,
};

#[derive(Debug, Deserialize)]
pub struct OpenOwnedRequest {
    /// The Library object URI to open (e.g. `localhost://Users/<principal>/…/clip.mp4`).
    pub uri: String,
}

/// True when the asset should play through the media viewer (MSE) rather than the
/// whole-object viewer. Kept narrow (video) so non-playable types render as objects.
fn is_media_mime(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    m.starts_with("video/")
}

/// Best-effort file extension for the helper's temp input (so its mime fallback +
/// any container probing see a sensible name). Derived from the object name.
fn extension_for(name: &str, mime: &str) -> String {
    if let Some((_, ext)) = name.rsplit_once('.') {
        if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return ext.to_ascii_lowercase();
        }
    }
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "application/pdf" => "pdf",
        "video/mp4" => "mp4",
        "text/plain" => "txt",
        _ => "bin",
    }
    .to_string()
}

/// A plaintext temp file that is unlinked on drop. The local key-authority reads it
/// fully at launch (before publishing its descriptor), so it is deleted as soon as
/// `launch` returns — the cleartext never lingers on disk beyond the seal handshake.
/// (Future hardening: stream the bytes to the helper over stdin to avoid disk at all.)
struct PlaintextTemp {
    path: PathBuf,
}

impl PlaintextTemp {
    fn write(bytes: &[u8], ext: &str) -> Result<Self, String> {
        use rand::RngCore;
        let mut rnd = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut rnd);
        let name = format!(
            "elastos-owned-{}.{ext}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rnd)
        );
        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).map_err(|e| format!("temp create: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes).map_err(|e| format!("temp write: {e}"))?;
        file.flush().map_err(|e| format!("temp flush: {e}"))?;
        Ok(Self { path })
    }

    fn path_str(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

impl Drop for PlaintextTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// POST /api/viewers/open — open an owned Library object in the right protected viewer.
pub async fn open_owned_in_viewer(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<OpenOwnedRequest>,
) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();

    // Ownership gate: resolve + read the object inside the principal's OWN root.
    let data_dir = state.data_dir.clone();
    let uri = req.uri.clone();
    let owned = match tokio::task::spawn_blocking(move || {
        crate::library::read_owned_object_for_viewer(&data_dir, &principal_id, &uri)
    })
    .await
    {
        Ok(Ok(owned)) => owned,
        Ok(Err(err)) => {
            tracing::warn!("owned object open refused: {err}");
            // Do not distinguish not-found from not-owned — same fail-closed shape.
            return (StatusCode::NOT_FOUND, "owned object not found").into_response();
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "owned object read panicked")
                .into_response()
        }
    };

    let helper_bin = resolve_helper_bin();
    let decrypt_bin = resolve_decrypt_bin();
    if !std::path::Path::new(&helper_bin).is_file() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "media-authority helper not found; build scripts/dev/ddrm-media-authority",
        )
            .into_response();
    }

    let object_cid = owned
        .content_cid
        .clone()
        .unwrap_or_else(|| format!("owned:{}", owned.name));
    let ext = extension_for(&owned.name, &owned.mime);

    // Live-chain gate: the rights-provider capsule decides access (Anders' rule —
    // the DECISION lives in the capsule, not here). On this host the on-chain
    // attestation is a dev local-attestation; deny -> fail closed. The minted receipt
    // hash is welded into the decrypt transcript by the authority (replay binding).
    let now = now_unix();
    let session_id = random_session_id();
    let rights = {
        let principal_id = context.principal_id.clone();
        let content_id = object_cid.clone();
        let session = session_id.clone();
        tokio::task::spawn_blocking(move || {
            super::rights_authority::decide_owned_access(
                &principal_id,
                &session,
                &content_id,
                "view",
                "owned object render",
                None,
                now,
                3600,
            )
        })
        .await
    };
    let rights = match rights {
        Ok(Ok(decision)) => decision,
        Ok(Err(err)) => {
            tracing::warn!("rights gate unavailable: {err}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "rights provider unavailable; cannot authorize open",
            )
                .into_response();
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "rights gate task panicked").into_response()
        }
    };
    if !rights.allowed {
        tracing::info!("owned open denied by rights for cid {object_cid}");
        return (
            StatusCode::FORBIDDEN,
            "no valid access token for this content (rights provider denied)",
        )
            .into_response();
    }
    let rights_binding = rights.receipt_hash_hex.clone();

    if is_media_mime(&owned.mime) {
        open_media(
            state, context, owned, object_cid, ext, helper_bin, decrypt_bin, session_id,
            rights_binding,
        )
        .await
    } else {
        open_object(
            state, context, owned, object_cid, ext, helper_bin, decrypt_bin, session_id,
            rights_binding,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn open_media(
    state: GatewayState,
    context: HomeLaunchTokenContext,
    owned: crate::library::OwnedObjectForViewer,
    object_cid: String,
    ext: String,
    helper_bin: String,
    decrypt_bin: String,
    session_id: String,
    rights_binding: String,
) -> Response {
    let principal_id = context.principal_id.clone();
    let title = owned.name.clone();
    let binding = rights_binding.clone();
    let built = tokio::task::spawn_blocking(move || {
        let temp = PlaintextTemp::write(&owned.bytes, &ext)?;
        let video = temp.path_str();
        // `temp` is dropped (unlinked) when this closure returns — after launch has
        // read + transcoded + sealed the bytes.
        MediaAuthorityProc::launch(
            &helper_bin,
            &decrypt_bin,
            &principal_id,
            Some(&video),
            Some(&object_cid),
            Some(&binding),
        )
    })
    .await;
    let proc = match built {
        Ok(Ok(proc)) => Arc::new(proc),
        Ok(Err(err)) => {
            tracing::warn!("owned media open failed: {err}");
            return (StatusCode::BAD_GATEWAY, "could not open owned media").into_response();
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "owned media task panicked").into_response()
        }
    };

    let session =
        MediaSession::from_authority(MEDIA_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    put_media_session(session_id.clone(), session);

    let token =
        match issue_home_launch_token_with_context(&state.data_dir, MEDIA_VIEWER_CAPSULE, &context) {
            Ok(token) => token,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("could not mint launch token: {err}"),
                )
                    .into_response()
            }
        };
    let play_url = format!("{}&home_token={}", media_play_route(&session_id), token);
    open_ok(MEDIA_VIEWER_CAPSULE, &session_id, &title, &play_url, &rights_binding)
}

#[allow(clippy::too_many_arguments)]
async fn open_object(
    state: GatewayState,
    context: HomeLaunchTokenContext,
    owned: crate::library::OwnedObjectForViewer,
    object_cid: String,
    ext: String,
    helper_bin: String,
    decrypt_bin: String,
    session_id: String,
    rights_binding: String,
) -> Response {
    let principal_id = context.principal_id.clone();
    let title = owned.name.clone();
    let mime = owned.mime.clone();
    let binding = rights_binding.clone();
    let built = tokio::task::spawn_blocking(move || {
        let temp = PlaintextTemp::write(&owned.bytes, &ext)?;
        let object_file = temp.path_str();
        ObjectAuthorityProc::launch(
            &helper_bin,
            &decrypt_bin,
            &principal_id,
            Some(&object_file),
            Some(&mime),
            Some(&object_cid),
            Some(&binding),
        )
    })
    .await;
    let proc = match built {
        Ok(Ok(proc)) => Arc::new(proc),
        Ok(Err(err)) => {
            tracing::warn!("owned object open failed: {err}");
            return (StatusCode::BAD_GATEWAY, "could not open owned asset").into_response();
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "owned object task panicked").into_response()
        }
    };

    let session =
        ObjectSession::from_authority(OBJECT_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    put_object_session(session_id.clone(), session);

    let token =
        match issue_home_launch_token_with_context(&state.data_dir, OBJECT_VIEWER_CAPSULE, &context)
        {
            Ok(token) => token,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("could not mint launch token: {err}"),
                )
                    .into_response()
            }
        };
    let view_url = format!("{}&home_token={}", object_view_route(&session_id), token);
    open_ok(OBJECT_VIEWER_CAPSULE, &session_id, &title, &view_url, &rights_binding)
}

fn open_ok(
    viewer: &str,
    session_id: &str,
    title: &str,
    play_url: &str,
    rights_binding: &str,
) -> Response {
    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(json!({
            "schema": "elastos.viewer.open/v1",
            "viewer": viewer,
            "session": session_id,
            "title": title,
            "play_url": play_url,
            // The rights-decision binding welded into the decrypt transcript (audit).
            "rights_binding": rights_binding,
        })),
    )
        .into_response()
}

fn random_session_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 18];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
