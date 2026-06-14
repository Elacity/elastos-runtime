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
//! Ownership has TWO gates, both enforced before anything is sealed: the local-sovereign
//! gate (the principal's own root — a not-owned/traversal URI never resolves) AND the
//! live-chain rights gate (`rights-provider.decide_access_from_chain` over a
//! `chain-provider.has_access_by_content_id` answer — dev, real Base RPC, or in-process
//! mock per `ELASTOS_DDRM_RIGHTS`). The minted rights-receipt hash is welded into the
//! decrypt transcript, so a seal is bound to the exact decision that authorized it.

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
use super::media_authority::{
    resolve_decrypt_bin, resolve_helper_bin, resolve_key_bin, MediaAuthorityProc,
};
use super::object_authority::ObjectAuthorityProc;
use super::viewer_media::{
    media_play_route, put_media_session, MediaSession, MEDIA_VIEWER_CAPSULE,
};
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
        file.write_all(bytes)
            .map_err(|e| format!("temp write: {e}"))?;
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "owned object read panicked",
            )
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

    // A minted dKMS asset is persisted AS its `.ddrm` capsule (no plaintext at rest). Detect it
    // up front: the openable item carries the dKMS escrow + the sealed ciphertext, and the open
    // is a 2-of-3 quorum RECOVER (production) rather than the local-test-KMS re-seal. The capsule
    // pins the on-chain content identity (for the rights gate) and the real presentation mime.
    let capsule = dkms_capsule(&owned.bytes);
    let render_mime = capsule
        .as_ref()
        .and_then(|c| c.get("mime").and_then(serde_json::Value::as_str))
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| owned.mime.clone());
    let object_cid = capsule
        .as_ref()
        .and_then(|c| {
            c.get("content_id")
                .or_else(|| c.get("kid"))
                .and_then(serde_json::Value::as_str)
        })
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            owned
                .content_cid
                .clone()
                .unwrap_or_else(|| format!("owned:{}", owned.name))
        });
    let ext = extension_for(&owned.name, &render_mime);

    // Live-chain gate: the rights-provider capsule decides access (Anders' rule —
    // the DECISION lives in the capsule, not here). The on-chain ownership answer comes
    // from ELASTOS_DDRM_RIGHTS: dev (local attestation), chain (real chain-provider vs
    // Base RPC), or chain-mock (real chain-provider vs in-process mock). Deny -> fail
    // closed. The minted receipt hash is welded into the decrypt transcript (replay
    // binding). The gateway never does chain RPC itself.
    let now = now_unix();
    let session_id = random_session_id();
    // The on-chain subject is the principal's linked EVM wallet (or ELASTOS_DDRM_SUBJECT
    // override). Empty in dev mode is fine (a placeholder is derived); chain mode fails
    // closed without it.
    let subject = resolve_subject_address(&state, &context.principal_id).await;
    let rights = {
        let principal_id = context.principal_id.clone();
        let content_id = object_cid.clone();
        let session = session_id.clone();
        let subject = subject.clone();
        tokio::task::spawn_blocking(move || {
            super::rights_authority::decide_owned_access(
                &principal_id,
                &session,
                &content_id,
                &subject,
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
            // A missing wallet linkage is an authorization fail-closed (403), not an
            // outage; a missing/misconfigured provider is a 503.
            if err.contains("wallet not linked") {
                tracing::info!("owned open denied: {err}");
                return (
                    StatusCode::FORBIDDEN,
                    "link an EVM wallet to open protected content",
                )
                    .into_response();
            }
            tracing::warn!("rights gate unavailable: {err}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "rights provider unavailable; cannot authorize open",
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "rights gate task panicked",
            )
                .into_response()
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

    if let Some(capsule) = capsule {
        return open_quorum(
            state,
            context,
            capsule,
            object_cid,
            render_mime,
            helper_bin,
            decrypt_bin,
            session_id,
            rights_binding,
        )
        .await;
    }

    if is_media_mime(&owned.mime) {
        open_media(
            state,
            context,
            owned,
            object_cid,
            ext,
            helper_bin,
            decrypt_bin,
            session_id,
            rights_binding,
        )
        .await
    } else {
        open_object(
            state,
            context,
            owned,
            object_cid,
            ext,
            helper_bin,
            decrypt_bin,
            session_id,
            rights_binding,
        )
        .await
    }
}

/// POST /api/market/buy — buy an access token for an owned Library object, then report
/// whether the rights gate will now allow the open.
///
/// This resolves the object EXACTLY as `/api/viewers/open` does (same root-scoped read,
/// same `content_id` derivation, same wallet `subject`), so the purchase is keyed on the
/// identifier the rights gate reads back. The buy itself (assemble → sign → broadcast →
/// await) lives in `buy_authority`; here we only authenticate, resolve, and report.
pub async fn buy_owned_access(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<OpenOwnedRequest>,
) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();

    // Resolve the object inside the principal's OWN root (the same ownership gate the
    // open uses) so we buy access for an object they can actually address.
    let data_dir = state.data_dir.clone();
    let uri = req.uri.clone();
    let pid = principal_id.clone();
    let owned = match tokio::task::spawn_blocking(move || {
        crate::library::read_owned_object_for_viewer(&data_dir, &pid, &uri)
    })
    .await
    {
        Ok(Ok(owned)) => owned,
        Ok(Err(err)) => {
            tracing::warn!("buy: owned object resolve refused: {err}");
            return (StatusCode::NOT_FOUND, "owned object not found").into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "owned object read panicked",
            )
                .into_response()
        }
    };

    let content_id = owned
        .content_cid
        .clone()
        .unwrap_or_else(|| format!("owned:{}", owned.name));
    let subject = resolve_subject_address(&state, &principal_id).await;
    let now = now_unix();

    let bought = {
        let principal_id = principal_id.clone();
        let content_id = content_id.clone();
        let subject = subject.clone();
        tokio::task::spawn_blocking(move || {
            super::buy_authority::buy_access(&principal_id, &content_id, &subject, now)
        })
        .await
    };
    match bought {
        Ok(Ok(outcome)) => (
            StatusCode::OK,
            [("cache-control", "no-store")],
            Json(json!({
                "schema": "elastos.market.buy/v1",
                "content_id": content_id,
                "subject": subject,
                "transaction_hash": outcome.tx_hash,
                // True when the open can now proceed (dev / chain-mock). On real chain the
                // open re-reads `hasAccessByContentId` once the tx confirms.
                "owned_now": outcome.owned_now,
                "mode": outcome.mode,
                "unsigned_tx": outcome.unsigned_tx,
            })),
        )
            .into_response(),
        Ok(Err(err)) => {
            if err.contains("wallet not linked") {
                return (StatusCode::FORBIDDEN, "link an EVM wallet to buy access").into_response();
            }
            // A live buy that needs an external signer is a precondition (409), not an
            // outage — surface the assembled tx so the caller can sign it.
            if err.contains("needs a signature") || err.contains("needs a signed transaction") {
                tracing::info!("buy requires external signature: {err}");
                return (StatusCode::CONFLICT, err).into_response();
            }
            tracing::warn!("buy failed: {err}");
            (StatusCode::BAD_GATEWAY, "could not complete buy").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "buy task panicked").into_response(),
    }
}

/// POST /api/create/mint — mint a dDRM content asset on chain (PC2 "Create" stage 2).
///
/// The body is a `chain-provider` MintAssembly (publish-provider `UnsignedMintV1`: `to`,
/// `token_uri`, `op_type_code`, `content_id`, optional `value_wei`/`op_raw`/`sell`); the
/// mint selector is pinned by the runtime. The orchestration (assemble → sign with a
/// managed account → broadcast) lives in `mint_authority`; here we only authenticate and
/// forward. The minter is the principal's managed account — the key never leaves
/// `wallet-provider`.
pub async fn mint_create_asset(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(assembly): Json<serde_json::Value>,
) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();
    let now = now_unix();

    let minted = tokio::task::spawn_blocking(move || {
        super::mint_authority::mint_asset(&principal_id, assembly, now)
    })
    .await;

    match minted {
        Ok(Ok(outcome)) => (
            StatusCode::OK,
            [("cache-control", "no-store")],
            Json(json!({
                "schema": "elastos.create.mint/v1",
                "transaction_hash": outcome.tx_hash,
                "mode": outcome.mode,
                "to": outcome.to,
                "content_id": outcome.content_id,
                "signer": outcome.signer,
                "assembled": outcome.assembled,
            })),
        )
            .into_response(),
        Ok(Err(err)) => {
            // A malformed mint is the caller's fault (400); a missing capsule binary or a
            // broadcast failure is an upstream/outage condition (502).
            if err.contains("invalid_mint")
                || err.contains("JSON object")
                || err.contains("must not be empty")
                || err.contains("MintAssembly")
            {
                tracing::info!("mint rejected (bad request): {err}");
                return (StatusCode::BAD_REQUEST, err).into_response();
            }
            tracing::warn!("mint failed: {err}");
            (StatusCode::BAD_GATEWAY, "could not complete mint").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "mint task panicked").into_response(),
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "owned media task panicked",
            )
                .into_response()
        }
    };

    let session =
        MediaSession::from_authority(MEDIA_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    put_media_session(session_id.clone(), session);

    let token =
        match issue_home_launch_token_with_context(&state.data_dir, MEDIA_VIEWER_CAPSULE, &context)
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
    let play_url = format!("{}&home_token={}", media_play_route(&session_id), token);
    open_ok(
        MEDIA_VIEWER_CAPSULE,
        &session_id,
        &title,
        &play_url,
        &rights_binding,
    )
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "owned object task panicked",
            )
                .into_response()
        }
    };

    let session =
        ObjectSession::from_authority(OBJECT_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    put_object_session(session_id.clone(), session);

    let token = match issue_home_launch_token_with_context(
        &state.data_dir,
        OBJECT_VIEWER_CAPSULE,
        &context,
    ) {
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
    open_ok(
        OBJECT_VIEWER_CAPSULE,
        &session_id,
        &title,
        &view_url,
        &rights_binding,
    )
}

/// Recognize a minted dKMS `.ddrm` capsule that is quorum-openable: the capsule schema, a
/// 2-of-3 threshold escrow in `protections[0]` (with the producer verifying key needed to verify
/// the recovered shares), and the persisted sealed `ciphertext_b64`. Anything else (a media
/// sidecar with no ciphertext, a pre-keystone mint missing the producer key) returns `None` and
/// falls through to the existing object/media paths.
fn dkms_capsule(bytes: &[u8]) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if v.get("schema").and_then(|s| s.as_str()) != Some("elastos.ddrm.capsule/v1") {
        return None;
    }
    v.get("ciphertext_b64")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())?;
    let prot = v
        .get("protections")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())?;
    let scheme = prot.get("scheme").and_then(|s| s.as_str())?;
    if !(scheme.contains("threshold") || scheme.contains("shamir") || scheme.contains("quorum")) {
        return None;
    }
    prot.get("producer_verifying_key_b64")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())?;
    Some(v)
}

/// The dKMS consumer-open: recover the minted asset's REAL CEK from the 2-of-3 quorum named by
/// the `.ddrm` capsule and render the byte-identical cleartext. The gateway spawns the isolated
/// `ddrm-media-authority --quorum` helper (which drives `key-provider(dkms)` + a separate
/// `decrypt-provider` boundary); it never links the PQ crypto and never sees the CEK or shares.
/// The recovered bytes are served through the SAME principal-bound `ObjectSession` as `open_object`.
#[allow(clippy::too_many_arguments)]
async fn open_quorum(
    state: GatewayState,
    context: HomeLaunchTokenContext,
    capsule: serde_json::Value,
    object_cid: String,
    mime: String,
    helper_bin: String,
    decrypt_bin: String,
    session_id: String,
    rights_binding: String,
) -> Response {
    let principal_id = context.principal_id.clone();
    let title = capsule
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Protected asset")
        .to_string();

    let key_bin = resolve_key_bin();
    if !std::path::Path::new(&key_bin).is_file() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "key-provider not found; build capsules/key-provider or set ELASTOS_DDRM_KEY_PROVIDER_BIN",
        )
            .into_response();
    }
    // The OPEN descriptor must carry the live node endpoints (the dkms backend connects to them);
    // it falls back to the mint-time public descriptor when the operator points both at one file.
    let descriptor = std::env::var("ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR")
        .or_else(|_| std::env::var("ELASTOS_DKMS_QUORUM_DESCRIPTOR"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    let descriptor = match descriptor {
        Some(d) if std::path::Path::new(&d).is_file() => d,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "dKMS quorum open not configured (set ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR to a live node-set descriptor)",
            )
                .into_response()
        }
    };
    let caller_seed = match std::env::var("ELASTOS_DDRM_QUORUM_CALLER_SEED_B64")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "dKMS quorum open not configured (set ELASTOS_DDRM_QUORUM_CALLER_SEED_B64)",
            )
                .into_response()
        }
    };

    let capsule_bytes = match serde_json::to_vec(&capsule) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not serialize capsule",
            )
                .into_response()
        }
    };
    let object_cid_c = object_cid.clone();
    let mime_c = mime.clone();
    let built = tokio::task::spawn_blocking(move || {
        // The capsule (escrow + CIPHERTEXT, never plaintext) is handed to the helper as a 0600
        // temp file, unlinked as soon as the helper has read it at launch.
        let temp = PlaintextTemp::write(&capsule_bytes, "ddrm")?;
        let capsule_file = temp.path_str();
        ObjectAuthorityProc::launch_quorum(
            &helper_bin,
            &decrypt_bin,
            &key_bin,
            &principal_id,
            &capsule_file,
            &descriptor,
            &caller_seed,
            &object_cid_c,
            &mime_c,
        )
    })
    .await;
    let proc = match built {
        Ok(Ok(proc)) => Arc::new(proc),
        Ok(Err(err)) => {
            tracing::warn!("dKMS quorum open failed: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                "could not open owned asset from the dKMS quorum",
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "quorum open task panicked",
            )
                .into_response()
        }
    };

    let session =
        ObjectSession::from_authority(OBJECT_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    put_object_session(session_id.clone(), session);

    let token = match issue_home_launch_token_with_context(
        &state.data_dir,
        OBJECT_VIEWER_CAPSULE,
        &context,
    ) {
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
    open_ok(
        OBJECT_VIEWER_CAPSULE,
        &session_id,
        &title,
        &view_url,
        &rights_binding,
    )
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

/// Resolve the on-chain `subject` for the rights check: the principal's linked EVM
/// wallet address. `ELASTOS_DDRM_SUBJECT` overrides (operator-pinned wallet for testing);
/// otherwise the wallet-provider is asked for the principal's accounts and the first
/// `eip155:` address is used. Returns empty if none is linked (dev mode derives a
/// placeholder; chain mode fails closed).
async fn resolve_subject_address(state: &GatewayState, principal_id: &str) -> String {
    if let Ok(pinned) = std::env::var("ELASTOS_DDRM_SUBJECT") {
        if !pinned.trim().is_empty() {
            return pinned;
        }
    }
    let accounts = super::auth_gateway::wallet_provider_data(
        state,
        json!({ "op": "accounts", "principal_id": principal_id }),
    )
    .await;
    let Ok(data) = accounts else {
        return String::new();
    };
    data.get("accounts")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|acct| {
            acct.get("chain_namespace")
                .and_then(|v| v.as_str())
                .map(|ns| ns.starts_with("eip155:"))
                .unwrap_or(false)
        })
        .and_then(|acct| acct.get("address").and_then(|v| v.as_str()))
        .map(str::to_string)
        .unwrap_or_default()
}
