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
    consume_passkey_step_up_token, default_evm_connector_id, issue_home_launch_token_with_context,
    require_home_token_launch, require_owned_open_token_context, require_owned_open_token_launch,
    runtime_wallet_authority, system_wallet_accounts_summary, GatewayState, HomeLaunchTokenContext,
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

/// Privacy-preserving log tag for a sensitive identifier (wallet/subject, content id, principal).
/// Returns a short, NON-reversible fingerprint (`fp:` + first 8 hex of SHA-256) so an operator can
/// still correlate a single open across log lines WITHOUT the raw `(wallet, content_id, time)`
/// access pattern being persisted to the gateway log (pre-audit #5: metadata minimization). The node
/// itself still observes the raw values in memory while it serves the open — this only keeps them out
/// of the on-disk/shippable log; see docs/THREAT_MODEL.md for what the operator can and cannot see.
fn log_fp(value: &str) -> String {
    use sha2::Digest as _;
    let v = value.trim();
    if v.is_empty() {
        return "<none>".to_string();
    }
    let digest = sha2::Sha256::digest(v.as_bytes());
    format!("fp:{}", hex::encode(&digest[..4]))
}

/// The forensic-watermark grant anchor (hex) for a wallet-signed grant: `SHA-256(normalized
/// delegation signature)[..16]`. A NON-reversible commitment to the buyer's EIP-191 delegation
/// signature — the same value the invisible pixel-lock mark embeds — so a leaked frame can be
/// verified against the §4 custody record (`audit_log().content_open`). MUST stay byte-identical to
/// `ddrm_envelope::grant_watermark_digest16` (the embedder side); elastos-server deliberately does
/// NOT link the PQ `ddrm-envelope` crate, so this is the no-shared-dep twin guarded by the
/// `grant_watermark_digest_matches_envelope_golden` cross-check (the `is_pixel_lock` pattern).
/// Normalization (trim + lowercase) is pinned by that golden so the two sides cannot silently
/// disagree on input handling.
fn grant_watermark_digest16_hex(delegation_sig_hex: &str) -> String {
    use sha2::Digest as _;
    let normalized = delegation_sig_hex.trim().to_ascii_lowercase();
    let digest = sha2::Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..16])
}

#[derive(Debug, Deserialize)]
pub struct OpenOwnedRequest {
    /// The Library object URI to open (e.g. `localhost://Users/<principal>/…/clip.mp4`).
    /// Defaulted (not required) because the MARKETPLACE buy path identifies the asset by
    /// `content_id` instead — the object is not in the Library yet. The open/legacy paths
    /// still fail closed on an empty uri (the owned-root resolve refuses it).
    #[serde(default)]
    pub uri: String,
    /// TRUSTLESS open (phase 2): the handle returned by `/api/viewers/prepare-grant` for this
    /// open. Present only when the browser collected a wallet signature; absent => legacy
    /// enrolled-caller path (the node still authorizes, via the rights receipt).
    #[serde(default)]
    pub grant_handle: Option<String>,
    /// TRUSTLESS open (phase 2): the wallet's EIP-191 `personal_sign` over the canonical
    /// delegation `prepare-grant` returned. Paired with `grant_handle`.
    #[serde(default)]
    pub delegation_sig_hex: Option<String>,
    /// MARKETPLACE buy (storefront, not-yet-owned): the asset KID/content-id to buy. When present the
    /// buy skips the owned-root resolve (the object isn't in the Library yet — `acquire` pins it after).
    /// Absent => the legacy uri-based re-buy for an object already owned.
    #[serde(default)]
    pub content_id: Option<String>,
    /// MARKETPLACE buy: the asset's on-chain identity from the discovery listing / `/api/market/get`,
    /// used to source live terms (`sellersOf`/`listings` at id=1) with no `ELASTOS_DDRM_BUY_*` pins.
    #[serde(default)]
    pub operative: Option<String>,
    #[serde(default)]
    pub token_id: Option<String>,
    #[serde(default)]
    pub ledger: Option<String>,
    /// Accepts a JSON string OR number (the storefront sends what its stepper holds); normalized
    /// to the decimal string `buy_authority` parses.
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub quantity: Option<String>,
    /// MARKETPLACE buy: the chosen listing seller (the storefront shows the lowest-priced one). Omit to
    /// let the gateway pick the lowest active seller live.
    #[serde(default)]
    pub seller: Option<String>,
    /// MARKETPLACE buy: the price/pay-token the buyer saw in the UI. When present they arm abort-on-drift
    /// (the live re-read at buy time must match).
    #[serde(default)]
    pub expected_price: Option<String>,
    #[serde(default)]
    pub expected_pay_token: Option<String>,
}

/// Accept a JSON string or number and normalize to `Option<String>` (the storefront's qty
/// stepper naturally serializes a number; the buy core parses a decimal string).
fn de_opt_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "expected a string or number, got {other}"
            )))
        }
    })
}

/// Phase-1 request: which owned object the user wants to open (so the gateway can bind the
/// delegation to that asset's on-chain identity + this quorum's node-set).
#[derive(Debug, Deserialize)]
pub struct PrepareGrantRequest {
    pub uri: String,
}

/// True when the asset should play through the media viewer (MSE) rather than the
/// whole-object viewer. Covers video AND audio: both are packaged on the fly into DASH by the
/// media authority (the audio-only path skips the video filter) and played through the same
/// single-SourceBuffer MSE player. Audio routed here (not the object viewer, which has no
/// playback) so an owned track actually plays. Non-playable types still render as objects.
fn is_media_mime(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    m.starts_with("video/") || m.starts_with("audio/")
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

/// A 0700 temp DIRECTORY that is removed (recursively) on drop — the staging area into which the
/// gateway fetches a media asset's DASH directory (clear inits + ENCRYPTED segments + manifest)
/// before handing its path to the quorum-media helper. The helper reads the whole set into its
/// recovery material at launch, so the directory is deleted as soon as `launch_quorum` returns.
struct PlaintextTempDir {
    path: PathBuf,
}

impl PlaintextTempDir {
    fn create(tag: &str) -> Result<Self, String> {
        use rand::RngCore;
        let mut rnd = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut rnd);
        let name = format!(
            "elastos-{tag}-{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rnd)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).map_err(|e| format!("temp dir create: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self { path })
    }

    fn path_str(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

impl Drop for PlaintextTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// POST /api/viewers/open — open an owned Library object in the right protected viewer.
pub async fn open_owned_in_viewer(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<OpenOwnedRequest>,
) -> Response {
    let context = match require_owned_open_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();
    // Phase timing (TRACE-level via INFO so it shows in the dev gateway log): the open has felt
    // slow, so each phase logs its elapsed ms to pinpoint the cost (read / subject / rights / open).
    let t_open = std::time::Instant::now();

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
    // Content id the live-chain rights gate reads back. Shared with `buy_owned_access` so a re-buy
    // keys the SAME id this open later checks (see [`resolve_owned_content_id`]).
    let object_cid = resolve_owned_content_id(&owned);
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
    tracing::info!(
        "open timing: owned read + capsule parse {} ms",
        t_open.elapsed().as_millis()
    );
    let t_subject = std::time::Instant::now();
    let subject = resolve_subject_address(&state, &context.principal_id).await;
    tracing::info!(
        "open timing: subject resolve {} ms",
        t_subject.elapsed().as_millis()
    );
    let t_rights = std::time::Instant::now();
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
    // Surface the live decision (source + verdict) so the on-chain check is visible in the gateway
    // log — `dev-local-attestation` vs `chain-provider (live RPC: …)` tells the operator exactly
    // which gate ran. The subject (wallet) and content id are logged only as NON-reversible
    // fingerprints (pre-audit #5): the operator can correlate one open across lines, but the raw
    // `(wallet, content_id)` access pattern is not persisted to the log.
    tracing::info!(
        "rights decision: allowed={} source={} cid={} subject={}",
        rights.allowed,
        rights.source,
        log_fp(&object_cid),
        log_fp(&subject)
    );
    tracing::info!(
        "open timing: rights decision (chain RPC) {} ms",
        t_rights.elapsed().as_millis()
    );
    if !rights.allowed {
        tracing::info!(
            "owned open denied by rights for cid {}",
            log_fp(&object_cid)
        );
        // GAP-8 custody record for the REFUSAL (best-effort: the access is already denied, so a
        // failed append/log-open cannot loosen the decision — it only loses one trail entry, logged loudly).
        match state.audit_log() {
            Ok(audit) => {
                if let Err(e) = audit.content_open(
                    &session_id,
                    &context.principal_id,
                    &object_cid,
                    "open",
                    "denied",
                    &rights.source,
                    None,
                ) {
                    tracing::error!("AUDIT content_open(denied) append failed: {e}");
                }
            }
            Err(e) => tracing::error!("AUDIT log unavailable for denied-open record: {e}"),
        }
        return (
            StatusCode::FORBIDDEN,
            "no valid access token for this content (rights provider denied)",
        )
            .into_response();
    }
    let rights_binding = rights.receipt_hash_hex.clone();

    // Resolve the wallet-signed grant for a QUORUM (capsule) open BEFORE the custody write, so the
    // §4 record can carry the forensic grant anchor (`grant_digest`) bound to the EXACT grant
    // forwarded to the dKMS nodes (a non-reversible commitment to the buyer's delegation signature
    // that the invisible watermark also embeds — see docs/THREAT_MODEL.md §4). A malformed fresh
    // grant is rejected here, before any custody "opened" record is written. Media / no-grant opens
    // resolve to `None` (no anchor). `grant_digest` is set from the SAME signature each path uses.
    let mut grant_digest: Option<String> = None;
    let capsule_access_grant_b64: Option<String> = if capsule.is_some() {
        // TRUSTLESS open: if the browser collected a wallet signature (phase 2), assemble the
        // wallet-signed grant here and forward it so the dKMS nodes authorize themselves. Absent
        // (or malformed) => fall through to the legacy enrolled-caller path (None).
        match (
            req.grant_handle.as_deref(),
            req.delegation_sig_hex.as_deref(),
        ) {
            (Some(handle), Some(sig)) if !handle.trim().is_empty() && !sig.trim().is_empty() => {
                // Fresh wallet signature (first open of this asset in the window) — assemble + cache.
                // spawn_blocking (Sprint 46, council S42 guardian F8): `assemble` shells out to the
                // grant sidecar (deadline-bounded, but still up to ~31s of blocking) — that wait
                // must hold a blocking-pool thread, not an async worker. A panicked task maps to
                // the same fail-closed Err arm.
                let (handle_o, sig_o) = (handle.to_string(), sig.to_string());
                let assembled = tokio::task::spawn_blocking(move || {
                    super::access_grant::assemble(&handle_o, &sig_o)
                        .and_then(|g| super::access_grant::grant_to_b64(&g))
                })
                .await
                .unwrap_or_else(|e| Err(format!("assemble task failed: {e}")));
                match assembled {
                    Ok(b64) => {
                        grant_digest = Some(grant_watermark_digest16_hex(sig));
                        tracing::info!(
                            "trustless open: assembled wallet-signed grant for cid {}",
                            log_fp(&object_cid)
                        );
                        Some(b64)
                    }
                    Err(err) => {
                        tracing::warn!(
                            "trustless open: could not assemble grant ({err}); refusing"
                        );
                        return (
                            StatusCode::BAD_REQUEST,
                            "could not assemble access grant (re-prepare the open)",
                        )
                            .into_response();
                    }
                }
            }
            // No fresh signature: if a live wallet-signed delegation is cached for (subject, kid)
            // from an earlier open this window, assemble a fresh grant from it — popup-free (PC2
            // secure-view session parity). The live chain gate above already authorized this open.
            _ => {
                let kid_for_grant = normalize_kid_0x(&object_cid);
                // spawn_blocking (Sprint 46, council guardian F2): `assemble_cached` shells the
                // SAME grant sidecar as `assemble` — the popup-free re-open path must not hold an
                // async worker for the (deadline-bounded, ≤~31s) wait either. A panicked task maps
                // to the same fall-back-to-enrolled-path arm (fail-safe here, not fail-closed —
                // the live chain gate above already authorized this open).
                let cached = {
                    let (subj, kid) = (subject.clone(), kid_for_grant.clone());
                    tokio::task::spawn_blocking(move || {
                        super::access_grant::assemble_cached(&subj, &kid)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("assemble_cached task failed: {e}")))
                };
                match cached {
                    Ok(Some(grant)) => {
                        // Anchor from the cached delegation's own signature (the one being forwarded).
                        grant_digest = grant
                            .get("delegation_sig_hex")
                            .and_then(|v| v.as_str())
                            .map(grant_watermark_digest16_hex);
                        match super::access_grant::grant_to_b64(&grant) {
                            Ok(b64) => {
                                tracing::info!("trustless open: reused cached delegation (no wallet popup) for cid {}", log_fp(&object_cid));
                                Some(b64)
                            }
                            Err(err) => {
                                tracing::warn!("trustless open: cached grant serialize failed ({err}); falling back to enrolled path");
                                grant_digest = None;
                                None
                            }
                        }
                    }
                    Ok(None) => None,
                    Err(err) => {
                        tracing::warn!("trustless open: cached grant assemble failed ({err}); falling back to enrolled path");
                        None
                    }
                }
            }
        }
    } else {
        None
    };

    // GAP-8: the who-opened-what-when CUSTODY record. We are now AUTHORIZED and committed to opening
    // (mint session → decrypt). Write it to the tamper-evident log BEFORE the open proceeds, and
    // FAIL CLOSED if it cannot be durably committed: an open that cannot be recorded in the custody
    // trail does not happen (custody integrity over availability — fail closed, then explain). The
    // forensic grant anchor rides here (non-reversible; `None` for media/no-grant opens).
    // AUD-2: get the signed custody log FAIL-CLOSED (a memory-only fallback is no longer
    // possible), then write the record fail-closed. Either failure refuses the open — content
    // whose open cannot be durably, tamper-evidently recorded does not happen.
    let audit = match state.audit_log() {
        Ok(audit) => audit,
        Err(e) => {
            tracing::error!("AUDIT log unavailable; refusing the open: {e}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "audit log unavailable; refusing to open without a custody record",
            )
                .into_response();
        }
    };
    // `content_open` -> `emit` does a synchronous fsync; run it on a blocking
    // thread so the dDRM open path does not park a tokio worker on disk I/O.
    // The fail-closed contract is unchanged: the open proceeds ONLY on
    // Ok(Ok(())); an emit failure OR a join failure (Err(_)) refuses the open —
    // content whose open cannot be durably, tamper-evidently recorded does not
    // happen.
    let content_open_result = {
        let audit = audit.clone();
        let session_id = session_id.clone();
        let principal_id = context.principal_id.clone();
        let object_cid = object_cid.clone();
        let source = rights.source.clone();
        let grant_digest = grant_digest.clone();
        tokio::task::spawn_blocking(move || {
            audit.content_open(
                &session_id,
                &principal_id,
                &object_cid,
                "open",
                "opened",
                &source,
                grant_digest.as_deref(),
            )
        })
        .await
    };
    if !matches!(content_open_result, Ok(Ok(()))) {
        match content_open_result {
            Ok(Err(e)) => {
                tracing::error!("AUDIT content_open(opened) append failed; refusing the open: {e}")
            }
            Err(join_err) => {
                tracing::error!(
                    "AUDIT content_open(opened) task failed; refusing the open: {join_err}"
                )
            }
            Ok(Ok(())) => unreachable!(),
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "audit log unavailable; refusing to open without a custody record",
        )
            .into_response();
    }

    if let Some(capsule) = capsule {
        // Capture (owner, kid) for the grant BEFORE `object_cid` is moved into the call, so the
        // quorum retry loop can regenerate a fresh per-request grant (A7). Must match the (subject,
        // kid) the grant above was assembled for.
        let owner_for_quorum = subject.clone();
        let kid_for_quorum = normalize_kid_0x(&object_cid);
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
            capsule_access_grant_b64,
            owner_for_quorum,
            kid_for_quorum,
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

/// The Base chain id the on-chain access lives on (8453 mainnet by default; overridable for tests).
fn chain_id() -> u64 {
    std::env::var("ELASTOS_DDRM_CHAIN_ID")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(8453)
}

/// PC2's `normalizedKid`: the on-chain `bytes16` contentId is the KID, always `0x`-prefixed. The
/// capsule stores it bare 32-hex; prefix it so the grant binds to the same value the node reads.
fn normalize_kid_0x(kid: &str) -> String {
    let t = kid.trim();
    if t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("0x{t}")
    } else {
        t.to_string()
    }
}

/// POST /api/viewers/prepare-grant — phase 1 of the trustless open.
///
/// Resolve the owned `.ddrm` asset, bind a freshly-minted session key to (chain, the asset's
/// `bytes16` contentId, THIS quorum's node-set, the user's wallet), and return the canonical
/// delegation string the browser hands MetaMask for `personal_sign`. No authority is granted here;
/// the signed grant only becomes meaningful when a node verifies it + reads the chain.
pub async fn prepare_owned_grant(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<PrepareGrantRequest>,
) -> Response {
    // The FULL launch (not just the context `require_owned_open_token_context` returns) is needed
    // here: `runtime_wallet_authority` can only be minted from a whole `RequiredHomeLaunchToken`
    // (launch id + launch context), and that authority is what lets us list the principal's wallet
    // accounts to resolve their default EVM connector below. Identical actor allowlist to
    // `require_owned_open_token_context` — this does NOT admit any caller the context-only gate
    // would have rejected.
    let required_launch = match require_owned_open_token_launch(&state.data_dir, &headers) {
        Ok(required) => required,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let context = required_launch.context.clone();
    let principal_id = context.principal_id.clone();

    // The wallet-signed grant is only meaningful when the dKMS nodes do their OWN node-side
    // on-chain check (chain mode). In dev / chain-mock the local quorum authorizes the recover
    // via the enrolled caller seed, so a grant would only make the node fail closed (it requires
    // DKMS_CHAIN_RPC_POOL). Tell the browser no grant is needed and let it fall back to the plain
    // open path that already works in those modes.
    if super::rights_authority::rights_mode() != super::rights_authority::RightsMode::Chain {
        return (
            StatusCode::BAD_REQUEST,
            "wallet grant not required in this rights mode (use plain open)",
        )
            .into_response();
    }

    let data_dir = state.data_dir.clone();
    let uri = req.uri.clone();
    let owned = match tokio::task::spawn_blocking(move || {
        crate::library::read_owned_object_for_viewer(&data_dir, &principal_id, &uri)
    })
    .await
    {
        Ok(Ok(owned)) => owned,
        Ok(Err(err)) => {
            tracing::warn!("prepare-grant: owned object resolve refused: {err}");
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

    // Only a quorum-openable `.ddrm` capsule carries the on-chain identity + node-set a grant binds.
    let capsule = match dkms_capsule(&owned.bytes) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "this asset is not a dKMS quorum capsule (no wallet grant needed)",
            )
                .into_response()
        }
    };
    let prot = capsule
        .get("protections")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first());
    let node_set_id_b64 = prot
        .and_then(|p| p.get("node_set_id_b64"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let kid_hex = capsule
        .get("kid")
        .or_else(|| capsule.get("content_id"))
        .and_then(|v| v.as_str())
        .map(normalize_kid_0x)
        .unwrap_or_default();
    if node_set_id_b64.is_empty() || kid_hex.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "asset capsule is missing its node-set id or kid",
        )
            .into_response();
    }

    let subject = resolve_subject_address(&state, &context.principal_id).await;

    // PC2 secure-view session parity: if the wallet already signed a delegation for (subject, kid)
    // earlier in the window, tell the browser it is "already delegated" so it skips the MetaMask
    // popup — the open path will assemble a fresh grant from the cached delegation. The live
    // on-chain check still runs on every open + every node recover, so revocation is unaffected.
    if super::access_grant::has_live_session(&subject, &kid_hex) {
        return (
            StatusCode::OK,
            [("cache-control", "no-store")],
            Json(json!({
                "schema": "elastos.viewer.prepare-grant/v1",
                "already_delegated": true,
                "kid": kid_hex,
                "chain_id": chain_id(),
            })),
        )
            .into_response();
    }

    // spawn_blocking (Sprint 46, council S42 guardian F8): `prepare` shells out to the grant
    // sidecar (deadline-bounded, but still up to ~31s of blocking) — that wait must hold a
    // blocking-pool thread, not an async worker. A panicked task maps to the same fail-closed
    // Err arm.
    let prepare_out = {
        let (chain, kid, nsid, subj) = (
            chain_id(),
            kid_hex.clone(),
            node_set_id_b64.clone(),
            subject.clone(),
        );
        tokio::task::spawn_blocking(move || super::access_grant::prepare(chain, &kid, &nsid, &subj))
            .await
            .unwrap_or_else(|e| Err(format!("prepare task failed: {e}")))
    };
    let prepared = match prepare_out {
        Ok(p) => p,
        Err(err) => {
            if err.contains("link an EVM wallet") {
                return (StatusCode::FORBIDDEN, err).into_response();
            }
            tracing::warn!("prepare-grant failed: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not prepare access grant",
            )
                .into_response();
        }
    };

    // Best-effort: tell the shell which connector is the user's DEFAULT EVM signer, so it can
    // auto-route the `personal_sign` popup there instead of guessing. This is a UX hint, not an
    // authorization decision — the on-chain rights gate and dKMS quorum recover are what actually
    // gate the plaintext, so a summary-lookup failure here degrades to "no default" (null) rather
    // than failing the whole prepare-grant response the user is waiting on.
    let default_connector_id = match runtime_wallet_authority(&required_launch) {
        Ok(authority) => {
            let summary = system_wallet_accounts_summary(&state, &authority).await;
            default_evm_connector_id(&summary)
        }
        Err(err) => {
            tracing::warn!("prepare-grant: could not mint wallet authority for default connector lookup: {err}");
            None
        }
    };

    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(prepare_grant_response_json(
            &prepared.handle,
            &prepared.delegation_canonical,
            &prepared.owner_address,
            &prepared.kid_hex,
            prepared.chain_id,
            default_connector_id.as_deref(),
        )),
    )
        .into_response()
}

/// Pure response-shaping for the prepare-grant success path — unit-testable without a
/// `GatewayState` or a live launch token. `default_connector_id` is always present in the object,
/// `null` when the principal has no resolvable default EVM connector, so the shell can distinguish
/// "no default" from a malformed response.
fn prepare_grant_response_json(
    grant_handle: &str,
    delegation_canonical: &str,
    owner_address: &str,
    kid_hex: &str,
    chain_id: u64,
    default_connector_id: Option<&str>,
) -> serde_json::Value {
    json!({
        "schema": "elastos.viewer.prepare-grant/v1",
        "grant_handle": grant_handle,
        // The exact UTF-8 string the browser must EIP-191 personal_sign.
        "delegation_canonical": delegation_canonical,
        "owner_address": owner_address,
        "kid": kid_hex,
        "chain_id": chain_id,
        "default_connector_id": default_connector_id,
    })
}

/// How recent a passkey assertion must be for a money verb to honour it. Identical to the window
/// the Wallet/recovery step-ups use (`gateway_wallet_app`, `auth_gateway`) so "fresh" means ONE
/// thing across the runtime — a money verb is never the lenient surface.
pub(in crate::api) const MONEY_STEP_UP_MAX_AGE_SECS: u64 = 180;

/// Split a money-verb body into the INTENT the user confirmed and the step-up token proving they
/// confirmed it. The intent is the body VERBATIM minus `step_up_token`, so the assertion is bound
/// to every term the caller sent (asset, seller, quantity, price, pay token, mint destination):
/// alter any one of them after the ceremony and the binding check rejects the replay.
fn split_money_step_up(mut body: serde_json::Value) -> (serde_json::Value, Option<String>) {
    let token = body
        .as_object_mut()
        .and_then(|fields| fields.remove("step_up_token"))
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|token| !token.is_empty());
    (body, token)
}

/// The ONE gate every node-signed money verb passes through: a Home-hosted, proof-bound launch
/// PLUS a fresh, single-use passkey step-up bound to that launch and to this exact intent.
///
/// A standing Home session is authentication, not authorization to spend — main's recovery and
/// Wallet surfaces already demand a ceremony per effect, and the money verbs (which sign with a
/// managed key and broadcast) are held to the same bar. Returns the launch's principal context and
/// the intent JSON with the token stripped, so callers parse exactly what was signed over.
///
/// Every refusal is a 403 (`money_verb_refused`): an unauthorized spend is never a 400 the caller
/// can mistake for a malformed field, and the message names the step-up so operators can tell
/// "not signed in" from "not verified for THIS purchase" in one log line.
fn authorize_money_verb(
    state: &GatewayState,
    headers: &HeaderMap,
    operation: &str,
    body: serde_json::Value,
) -> anyhow::Result<(HomeLaunchTokenContext, serde_json::Value)> {
    let launch = require_home_token_launch(&state.data_dir, headers)?;
    let (intent, step_up_token) = split_money_step_up(body);
    let Some(step_up_token) = step_up_token else {
        anyhow::bail!(MONEY_VERB_STEP_UP_REQUIRED);
    };
    consume_passkey_step_up_token(
        &state.data_dir,
        &step_up_token,
        &launch,
        MONEY_STEP_UP_MAX_AGE_SECS,
        operation,
        &intent,
    )?;
    Ok((launch.context, intent))
}

/// The caller-facing reasons a money verb can be refused. Deliberately a CLOSED set.
///
/// The gate's internal errors are useful and specific — but they are specific about the runtime's
/// own state, not the caller's request: token decode failures, envelope schema mismatches, missing
/// gateway identity, unreadable or full step-up replay state, filesystem paths under the data dir.
/// Returning those verbatim narrates the node's persistence layer to an unauthenticated-to-spend
/// caller. Every one of them means the same thing on the wire — "this spend is not authorized" —
/// so they collapse to one line, and the detail goes to the operator's log instead.
///
/// The three distinctions kept are the ones a caller can act on: sign in, run the ceremony, or run
/// it again for THIS request.
pub(in crate::api) const MONEY_VERB_NOT_SIGNED_IN: &str =
    "this spend requires an active Home sign-in from the Home app itself";
pub(in crate::api) const MONEY_VERB_STEP_UP_REQUIRED: &str =
    "a fresh passkey step-up is required to spend: complete a passkey step-up ceremony for this \
     request and resend it with `step_up_token`";
pub(in crate::api) const MONEY_VERB_STEP_UP_REJECTED: &str =
    "the passkey step-up for this spend was rejected: it is expired, already used, or was not \
     issued for this exact request — run a new step-up ceremony for this request";

/// A money verb's refusal — always 403, never a status a caller can retry into a spend.
///
/// Maps to [`MONEY_VERB_NOT_SIGNED_IN`] / [`MONEY_VERB_STEP_UP_REQUIRED`] /
/// [`MONEY_VERB_STEP_UP_REJECTED`], and logs the underlying error at `warn` so operators keep the
/// one-line diagnosis they had before.
fn money_verb_refused(err: anyhow::Error) -> Response {
    let detail = err.to_string();
    let public = if detail == MONEY_VERB_STEP_UP_REQUIRED {
        MONEY_VERB_STEP_UP_REQUIRED
    } else if detail.contains("passkey step-up") {
        MONEY_VERB_STEP_UP_REJECTED
    } else {
        // Everything the launch check can produce, plus anything unclassified: fail closed onto
        // the least informative reason rather than leaking an unrecognized message.
        MONEY_VERB_NOT_SIGNED_IN
    };
    tracing::warn!("money verb refused: {detail}");
    (StatusCode::FORBIDDEN, public).into_response()
}

/// POST /api/market/buy — buy an access token for an owned Library object, then report
/// whether the rights gate will now allow the open.
///
/// This resolves the object EXACTLY as `/api/viewers/open` does: the legacy owned-root branch
/// derives `content_id` through the SAME shared [`resolve_owned_content_id`] the open path calls
/// (capsule `content_id`/`kid` first, then `content_cid`, then `owned:{name}`) and uses the same
/// wallet `subject`, so the purchase is keyed on the identifier the rights gate reads back — even
/// for a dKMS-capsule asset. The buy itself (assemble → sign → broadcast → await) lives in
/// `buy_authority`; here we only authenticate, resolve, and report.
pub async fn buy_owned_access(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let (context, intent) = match authorize_money_verb(&state, &headers, "market.buy", body) {
        Ok(authorized) => authorized,
        Err(err) => return money_verb_refused(err),
    };
    let req: OpenOwnedRequest = match serde_json::from_value(intent) {
        Ok(req) => req,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid buy request: {err}"),
            )
                .into_response()
        }
    };
    let principal_id = context.principal_id.clone();

    // MARKETPLACE buy (storefront): the asset isn't in the Library yet, so the request carries the KID
    // (`content_id`) directly — skip the owned-root resolve (that's the legacy re-buy path for an object
    // already owned). `acquire` pins it into the Library after the buy confirms.
    let content_id = if let Some(cid) = req.content_id.clone().filter(|s| !s.trim().is_empty()) {
        cid
    } else {
        // Legacy: resolve the object inside the principal's OWN root (same ownership gate the open uses).
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
        // SAME derivation the open path uses (capsule `content_id`/`kid` first, then `content_cid`,
        // then `owned:{name}`) — a dKMS-capsule re-buy MUST key on the capsule id the rights gate
        // reads back, not `content_cid`/`owned:{name}` (which it never reads).
        resolve_owned_content_id(&owned)
    };
    let subject = resolve_subject_address(&state, &principal_id).await;
    let now = now_unix();

    // The storefront supplies the asset's on-chain identity + buyer-agreed terms; the live `chain` path
    // sources seller/price/payToken from these with no `ELASTOS_DDRM_BUY_*` pins.
    let target = super::buy_authority::BuyTarget {
        operative: req.operative.clone(),
        token_id: req.token_id.clone(),
        ledger: req.ledger.clone(),
        quantity: req.quantity.clone(),
        seller: req.seller.clone(),
        expected_price: req.expected_price.clone(),
        expected_pay_token: req.expected_pay_token.clone(),
    };

    let bought = {
        let principal_id = principal_id.clone();
        let content_id = content_id.clone();
        let subject = subject.clone();
        tokio::task::spawn_blocking(move || {
            super::buy_authority::buy_access(&principal_id, &content_id, &subject, now, &target)
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
            // The HTTP surface only needs the message text to pick a status code; the money
            // classification (pre/post-broadcast) is the caller-side rail's concern, not this
            // read-open handler's.
            let msg = err.to_string();
            if msg.contains("wallet not linked") {
                return (StatusCode::FORBIDDEN, "link an EVM wallet to buy access").into_response();
            }
            // A live buy that needs an external signer is a precondition (409), not an
            // outage — surface the assembled tx so the caller can sign it. (Matches the exact
            // producer phrase in `buy_authority`; the old dead `"needs a signed transaction"`
            // alternative was removed — no producer emits it. Council S43 red-team INFO-2.)
            if msg.contains("needs a signature") {
                tracing::info!("buy requires external signature: {msg}");
                return (StatusCode::CONFLICT, msg).into_response();
            }
            tracing::warn!("buy failed: {msg}");
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    let (context, assembly) = match authorize_money_verb(&state, &headers, "create.mint", body) {
        Ok(authorized) => authorized,
        Err(err) => return money_verb_refused(err),
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
            // The error CARRIES its class (no prose matching): a malformed mint is the caller's
            // fault (400, reason surfaced); a missing capsule binary / signing / broadcast failure
            // is an upstream/outage condition (502, detail kept server-side).
            if err.is_client_error() {
                tracing::info!("mint rejected (bad request): {err}");
                (StatusCode::BAD_REQUEST, err.client_message()).into_response()
            } else {
                tracing::warn!("mint failed: {err}");
                (StatusCode::BAD_GATEWAY, err.client_message()).into_response()
            }
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
    let play_url =
        crate::api::viewer_route_with_launch_token(&media_play_route(&session_id), &token);
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
    let view_url =
        crate::api::viewer_route_with_launch_token(&object_view_route(&session_id), &token);
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
    // The recoverable payload is EITHER an embedded single-sample ciphertext (object) OR a media
    // DASH layout pointing at the published directory at `asset_cid` (media). One must be present
    // or the asset is unrecoverable (and not a quorum-openable item).
    let has_object_ciphertext = v
        .get("ciphertext_b64")
        .and_then(|c| c.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_media_layout = v.get("media").map(|m| !m.is_null()).unwrap_or(false)
        && v.get("asset_cid")
            .and_then(|c| c.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
    if !has_object_ciphertext && !has_media_layout {
        return None;
    }
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

/// Derive the dKMS node-set id (standard base64) from a live open-descriptor file, using the
/// canonical derivation (`ddrm_envelope::threshold_node_set_id_n`): SHA-256 over the domain tag,
/// the threshold `t` byte, then each node's verifying key (`u32`-BE length ‖ bytes) IN DESCRIPTOR
/// ORDER. It lets the gateway compare — host-side, up front — the node-set an asset was SEALED to
/// (its `protections[0].node_set_id_b64`) against the node-set the RUNNING quorum enforces. A
/// mismatch is EXACTLY the node-side `bad_node_set` refusal, surfaced before we spawn a recover so
/// the failure is self-evident from the gateway log alone (the helper reports it only on stderr).
fn descriptor_node_set_id_b64(descriptor_path: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let raw = std::fs::read(descriptor_path).ok()?;
    let d: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let th = d.get("threshold")?;
    let t = th.get("t").and_then(|v| v.as_u64())? as u8;
    let nodes = th.get("nodes").and_then(|n| n.as_array())?;
    let mut h = Sha256::new();
    h.update(b"elastos.dkms.threshold.node-set/v1");
    h.update([t]);
    for n in nodes {
        let vk_b64 = n.get("verifying_key_b64").and_then(|v| v.as_str())?;
        let vk = base64::engine::general_purpose::STANDARD
            .decode(vk_b64)
            .ok()?;
        h.update((vk.len() as u32).to_be_bytes());
        h.update(&vk);
    }
    Some(base64::engine::general_purpose::STANDARD.encode(h.finalize()))
}

/// Single source of truth for the on-chain content id of an owned dDRM asset, shared by BOTH
/// `/api/viewers/open` and the legacy owned-root re-buy branch of [`buy_owned_access`]. The open
/// path is authoritative: its precedence is the capsule `content_id` (else `kid`) FIRST, then
/// `owned.content_cid`, then `owned:{name}`. A minted dKMS asset is persisted AS its `.ddrm`
/// capsule, so the id the live-chain rights gate reads back lives INSIDE the capsule — deriving it
/// from `content_cid`/`owned:{name}` (as the old buy path did, never parsing the capsule) keyed a
/// re-buy on an id the gate never reads, so money was spent and the open still denied. Routing both
/// sites through here makes the buy/open derivations identical by construction.
fn resolve_owned_content_id(owned: &crate::library::OwnedObjectForViewer) -> String {
    dkms_capsule(&owned.bytes)
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
        })
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
    access_grant_b64: Option<String>,
    // (owner, kid) the grant was assembled for — used to regenerate a fresh per-request grant on
    // retries so the node's single-use replay guard doesn't reject a legitimate retry (A7).
    owner_address: String,
    kid_hex: String,
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

    // ── UNAMBIGUOUS node-set evidence (dKMS `bad_node_set` root-cause trace) ──────────────────
    // A node refuses a recover ("access grant rejected: bad_node_set") when the node-set an asset
    // was SEALED to differs from the one the running quorum enforces. That refusal is the single
    // most common reason an owned open fails, yet the helper reports it only on its (inherited)
    // stderr — the gateway otherwise sees a generic "capsule exited before answering" and mints an
    // opaque 502. Compute BOTH ids host-side and log the verdict so the cause is self-evident from
    // the gateway log alone, BEFORE we spawn the recover.
    let asset_node_set = capsule
        .get("protections")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())
        .and_then(|p| p.get("node_set_id_b64"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let quorum_node_set = descriptor_node_set_id_b64(&descriptor).unwrap_or_default();
    tracing::trace!(
        "dKMS open_quorum: kid={kid_hex} mime={mime} object_cid={} descriptor={descriptor}",
        log_fp(&object_cid)
    );
    // DEEP evidence: fingerprint the ORDERED node verifying-keys on BOTH sides. This tells a
    // genuinely-different quorum (DISJOINT vk sets — the asset's secret-holders no longer exist)
    // apart from a mere derivation/ordering mismatch (SAME vk set, different id). The mint seals
    // each share under its node's `verifying_key_b64` (encrypt-provider), and node_set_id is derived
    // from exactly those keys — so identical fp sets here would mean the shares ARE recoverable and
    // only the id derivation drifted. All values are PUBLIC keys, logged as non-reversible fps.
    let asset_vk_fps: Vec<String> = capsule
        .get("protections")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())
        .and_then(|p| p.get("shares"))
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|sh| sh.get("verifying_key_b64").and_then(|v| v.as_str()))
                .map(log_fp)
                .collect()
        })
        .unwrap_or_default();
    let quorum_vk_fps: Vec<String> = std::fs::read(&descriptor)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|d| {
            d.get("threshold")
                .and_then(|t| t.get("nodes"))
                .and_then(|n| n.as_array())
                .cloned()
        })
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("verifying_key_b64").and_then(|v| v.as_str()))
                .map(log_fp)
                .collect()
        })
        .unwrap_or_default();
    tracing::warn!(
        "dKMS node-set evidence: asset node-vk fps={asset_vk_fps:?} vs running quorum node-vk fps={quorum_vk_fps:?} (kid={kid_hex}) — DISJOINT sets ⇒ dead quorum (shares gone); SAME set ⇒ id-derivation drift"
    );
    if asset_node_set.is_empty() || quorum_node_set.is_empty() {
        tracing::warn!(
            "dKMS node-set precheck: could not derive both ids (asset='{asset_node_set}', quorum='{quorum_node_set}') — proceeding to recover"
        );
    } else if asset_node_set == quorum_node_set {
        tracing::info!(
            "dKMS node-set precheck: MATCH node_set={asset_node_set} kid={kid_hex} — asset is recoverable by the running quorum"
        );
    } else {
        // Fail FAST and HONESTLY (still fail-closed): the node-set is a permanent property of how the
        // asset was sealed. When it differs from the running quorum EVERY node rejects the recover with
        // bad_node_set — retrying cannot help. We know this host-side, before spawning, so surface a
        // clear, actionable message instead of three doomed attempts and an opaque 502. (Our derivation
        // is the canonical `threshold_node_set_id_n`, verified equal to the node's own pin, so a
        // MISMATCH here is exactly the refusal the quorum would return; we never block a recoverable
        // asset. If either id could not be derived we do NOT take this branch — we proceed to recover.)
        tracing::warn!(
            "dKMS node-set precheck: MISMATCH — asset sealed to node_set={asset_node_set} but the running quorum is node_set={quorum_node_set} (kid={kid_hex}). \
             Every share would be refused with bad_node_set; failing fast instead of spawning a doomed recover."
        );
        let short = |s: &str| s.chars().take(12).collect::<String>();
        return (
            StatusCode::CONFLICT,
            format!(
                "This protected asset was sealed to a different dKMS quorum (node-set {}…) than the one currently running (node-set {}…), so its keys cannot be recovered here. Re-mint it against the current quorum, or start the quorum whose nodes hold its shares.",
                short(&asset_node_set),
                short(&quorum_node_set)
            ),
        )
            .into_response();
    }

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

    // MEDIA (DASH): the capsule carries a `media` layout and the mime is audio/video. Recover the
    // CEK 2-of-3 and stream CENC-decrypted fMP4 fragments to the MSE player (the PC2 two-viewer
    // split: media routes to `elacity-player`, never the object viewer). Objects fall through.
    if is_media_mime(&mime) && capsule.get("media").map(|m| !m.is_null()).unwrap_or(false) {
        return open_quorum_media(
            state,
            context,
            capsule,
            capsule_bytes,
            object_cid,
            mime,
            helper_bin,
            decrypt_bin,
            key_bin,
            descriptor,
            caller_seed,
            session_id,
            rights_binding,
            access_grant_b64,
            owner_address,
            kid_hex,
        )
        .await;
    }

    let t_recover = std::time::Instant::now();
    // The LIVE remote quorum (Carrier/geo nodes) intermittently drops a node mid-recover ("node
    // closed its connection unexpectedly" → only 0/3 or 1/3 served → sub-quorum refused). That is
    // transient transport flakiness, not a real denial: the next attempt usually meets 2-of-3.
    // Retry a bounded number of times with a short backoff before surfacing a 502. EVERY attempt
    // re-runs the SAME on-chain authorization + full 2-of-3 recover, so this only masks flaky
    // transport — it never weakens the gate, never accepts a sub-quorum, and stays fail-closed.
    const MAX_QUORUM_ATTEMPTS: usize = 3;
    let mut launched = None;
    let mut last_err = String::new();
    for attempt in 1..=MAX_QUORUM_ATTEMPTS {
        tracing::trace!(
            "dKMS recover (object) attempt {attempt}/{MAX_QUORUM_ATTEMPTS}: spawning quorum helper (grant={}) kid={kid_hex}",
            if access_grant_b64.is_some() { "wallet-signed" } else { "enrolled" }
        );
        let helper_bin = helper_bin.clone();
        let decrypt_bin = decrypt_bin.clone();
        let key_bin = key_bin.clone();
        let principal_id = principal_id.clone();
        let descriptor = descriptor.clone();
        let caller_seed = caller_seed.clone();
        let object_cid_c = object_cid.clone();
        let mime_c = mime.clone();
        let orig_grant = access_grant_b64.clone();
        let owner_c = owner_address.clone();
        let kid_c = kid_hex.clone();
        let capsule_bytes = capsule_bytes.clone();
        let built = tokio::task::spawn_blocking(move || {
            // A7: on a RETRY (attempt > 1) send a FRESH grant — a new per-request nonce assembled
            // from the cached wallet-signed delegation (no popup, same delegation signature/anchor)
            // — so the node's single-use replay guard doesn't reject a legitimate retry after
            // transient transport loss. Regeneration shells the grant sidecar, hence it runs here in
            // the blocking task; it fails soft to the original grant.
            let grant_c = super::access_grant::grant_for_attempt(
                orig_grant.as_deref(),
                &owner_c,
                &kid_c,
                attempt,
            );
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
                grant_c.as_deref(),
            )
        })
        .await;
        match built {
            Ok(Ok(proc)) => {
                launched = Some(Arc::new(proc));
                break;
            }
            Ok(Err(err)) => {
                last_err = err.to_string();
                tracing::warn!(
                    "dKMS quorum open attempt {attempt}/{MAX_QUORUM_ATTEMPTS} failed: {last_err}"
                );
                if attempt < MAX_QUORUM_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(300 * attempt as u64))
                        .await;
                }
            }
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "quorum open task panicked",
                )
                    .into_response()
            }
        }
    }
    let proc = match launched {
        Some(proc) => proc,
        None => {
            tracing::warn!(
                "dKMS quorum open failed after {MAX_QUORUM_ATTEMPTS} attempts: {last_err}"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "could not open owned asset from the dKMS quorum",
            )
                .into_response();
        }
    };
    // This single span covers helper spawn + key-provider/decrypt-provider spawn + the 2-of-3
    // quorum recover over Carrier — the dominant cost of the open. Logged so we can see it.
    tracing::info!(
        "open timing: quorum recover (spawn + 2-of-3 over Carrier) {} ms",
        t_recover.elapsed().as_millis()
    );

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
    let view_url =
        crate::api::viewer_route_with_launch_token(&object_view_route(&session_id), &token);
    open_ok(
        OBJECT_VIEWER_CAPSULE,
        &session_id,
        &title,
        &view_url,
        &rights_binding,
    )
}

/// MEDIA dKMS consumer-open: fetch the published DASH directory by its asset CID, recover the
/// CEK 2-of-3 from the quorum named by the `.ddrm` capsule, and stream CENC-decrypted fMP4
/// fragments to `elacity-player` (MSE). The gateway never links the PQ crypto and never sees the
/// CEK; the recovered plaintext fragments are served per-segment through a principal-bound
/// `MediaSession`. This is the media half of PC2's two-viewer split (the object viewer is the
/// other half) — the analogue of the local-test-KMS `open_media`, but recovering the REAL CEK.
#[allow(clippy::too_many_arguments)]
async fn open_quorum_media(
    state: GatewayState,
    context: HomeLaunchTokenContext,
    capsule: serde_json::Value,
    capsule_bytes: Vec<u8>,
    object_cid: String,
    mime: String,
    helper_bin: String,
    decrypt_bin: String,
    key_bin: String,
    descriptor: String,
    caller_seed: String,
    session_id: String,
    rights_binding: String,
    access_grant_b64: Option<String>,
    // (owner, kid) the grant was assembled for — see open_quorum (A7 retry-nonce refresh).
    owner_address: String,
    kid_hex: String,
) -> Response {
    let principal_id = context.principal_id.clone();
    let title = capsule
        .get("title")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Protected media")
        .to_string();

    // The DASH directory CID (the asset CID): clear per-track inits + ENCRYPTED segments + MPD.
    let asset_cid = match capsule
        .get("asset_cid")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        Some(c) => c.to_string(),
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "media capsule missing asset_cid",
            )
                .into_response()
        }
    };

    let registry = match state.provider_registry.as_ref() {
        Some(r) => r.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "content provider unavailable for media fetch",
            )
                .into_response()
        }
    };
    let dash_dir = match PlaintextTempDir::create("ddrm-dash") {
        Ok(d) => d,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not stage media dir: {err}"),
            )
                .into_response()
        }
    };
    let dash_dir_path = dash_dir.path_str();

    // Fetch the whole DASH directory by CID into the 0700 temp dir (the helper reads inits +
    // encrypted segments from it at recovery; nothing decrypted is fetched here).
    let t_fetch = std::time::Instant::now();
    let dl = registry
        .send_raw(
            "ipfs",
            &json!({ "op": "download_directory", "cid": asset_cid, "dest": dash_dir_path }),
        )
        .await;
    match dl {
        Ok(resp) if resp.get("status").and_then(serde_json::Value::as_str) != Some("error") => {}
        Ok(resp) => {
            tracing::warn!("quorum media open: DASH fetch failed: {resp}");
            return (
                StatusCode::BAD_GATEWAY,
                "could not fetch media directory from IPFS",
            )
                .into_response();
        }
        Err(err) => {
            tracing::warn!("quorum media open: DASH fetch error: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                "could not fetch media directory from IPFS",
            )
                .into_response();
        }
    }
    tracing::info!(
        "open timing: DASH directory fetch {} ms",
        t_fetch.elapsed().as_millis()
    );

    let t_recover = std::time::Instant::now();
    // Same transient-quorum resilience as the object open: the LIVE remote nodes occasionally drop
    // a connection mid-recover, refusing the (real) sub-quorum. Retry a bounded number of times
    // before a 502. Every attempt re-runs the SAME on-chain authorization + full 2-of-3 recover, so
    // this only masks flaky Carrier transport — it never weakens the gate or accepts a sub-quorum.
    const MAX_QUORUM_ATTEMPTS: usize = 3;
    let mut launched = None;
    let mut last_err = String::new();
    for attempt in 1..=MAX_QUORUM_ATTEMPTS {
        tracing::trace!(
            "dKMS recover (media) attempt {attempt}/{MAX_QUORUM_ATTEMPTS}: spawning quorum helper (grant={}) kid={kid_hex}",
            if access_grant_b64.is_some() { "wallet-signed" } else { "enrolled" }
        );
        let helper_bin = helper_bin.clone();
        let decrypt_bin = decrypt_bin.clone();
        let key_bin = key_bin.clone();
        let principal_id = principal_id.clone();
        let descriptor = descriptor.clone();
        let caller_seed = caller_seed.clone();
        let object_cid_c = object_cid.clone();
        let mime_c = mime.clone();
        let orig_grant = access_grant_b64.clone();
        let owner_c = owner_address.clone();
        let kid_c = kid_hex.clone();
        let dash_dir_c = dash_dir_path.clone();
        let capsule_bytes = capsule_bytes.clone();
        let built = tokio::task::spawn_blocking(move || {
            // A7 (same as the object path): regenerate a fresh per-request grant on retries so the
            // node's single-use replay guard doesn't reject a legitimate retry. Fails soft.
            let grant_c = super::access_grant::grant_for_attempt(
                orig_grant.as_deref(),
                &owner_c,
                &kid_c,
                attempt,
            );
            // The capsule (escrow + identities, never plaintext) goes to the helper as a 0600 temp
            // file; the helper reads it + the DASH dir at launch, recovers the CEK, and warms the
            // boundary. `temp` is unlinked when this closure returns.
            let temp = PlaintextTemp::write(&capsule_bytes, "ddrm")?;
            let capsule_file = temp.path_str();
            MediaAuthorityProc::launch_quorum(
                &helper_bin,
                &decrypt_bin,
                &key_bin,
                &principal_id,
                &capsule_file,
                &descriptor,
                &caller_seed,
                &dash_dir_c,
                &object_cid_c,
                &mime_c,
                grant_c.as_deref(),
            )
        })
        .await;
        match built {
            Ok(Ok(proc)) => {
                launched = Some(Arc::new(proc));
                break;
            }
            Ok(Err(err)) => {
                last_err = err.to_string();
                tracing::warn!(
                    "dKMS quorum media open attempt {attempt}/{MAX_QUORUM_ATTEMPTS} failed: {last_err}"
                );
                if attempt < MAX_QUORUM_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(300 * attempt as u64))
                        .await;
                }
            }
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "quorum media open task panicked",
                )
                    .into_response()
            }
        }
    }
    let proc = match launched {
        Some(proc) => proc,
        None => {
            tracing::warn!(
                "dKMS quorum media open failed after {MAX_QUORUM_ATTEMPTS} attempts: {last_err}"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "could not open owned media from the dKMS quorum",
            )
                .into_response();
        }
    };
    tracing::info!(
        "open timing: quorum media recover (2-of-3 + CENC over the segment set) {} ms",
        t_recover.elapsed().as_millis()
    );
    // The helper read the whole DASH set into its recovery material at launch; drop the staging
    // directory now (removes the encrypted segments from disk for the rest of the session).
    drop(dash_dir);

    let mut session =
        MediaSession::from_authority(MEDIA_VIEWER_CAPSULE, context.principal_id.clone(), proc);
    // Surface the asset's PUBLIC cover art (the teaser pinned at mint, persisted in the capsule as
    // `thumbnail`) so the player can show it as the audio poster. Key-free, public imagery only.
    session.cover_uri = capsule
        .get("thumbnail")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Public asset title — lets the player label a generated cover placeholder with the asset name.
    session.title = title.clone();
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
    let play_url =
        crate::api::viewer_route_with_launch_token(&media_play_route(&session_id), &token);
    open_ok(
        MEDIA_VIEWER_CAPSULE,
        &session_id,
        &title,
        &play_url,
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
/// wallet address. Returns empty if none is linked (dev mode derives a placeholder; chain mode
/// fails closed).
///
/// `ELASTOS_DDRM_SUBJECT` pins the subject for local development ONLY. It is compiled out of a
/// release build (no `dev-modes`) because a pin makes every principal's rights check read one
/// operator-chosen wallet — point it at a wallet holding the access token and the whole node
/// unlocks the asset. `enforce_release_build_rights_safety` additionally refuses to boot a release
/// binary that was handed the variable, so the misconfiguration is loud rather than inert.
pub(crate) async fn resolve_subject_address(state: &GatewayState, principal_id: &str) -> String {
    #[cfg(feature = "dev-modes")]
    if let Ok(pinned) = std::env::var("ELASTOS_DDRM_SUBJECT") {
        if !pinned.trim().is_empty() {
            return pinned;
        }
    }
    // Resolve the signed-in principal's OWN verified EVM wallet from the auth store — the wallet the
    // user bound through the wallet app, persisted as an `EvmAccount` proof binding. This is the DRM
    // subject: resolved per-principal from already-verified state, no operator pin, and available
    // from the `principal_id` alone (no `RuntimeWalletAuthority` needed, because a proof binding is
    // persisted verified state, not a live wallet-account enumeration).
    //
    // A live wallet-provider `ListAccounts` path (with default-`browser_connect`-account precedence,
    // see the parked `select_subject_from_accounts` below) remains the richer future source for the
    // multi-wallet case; it needs the full `RequiredHomeLaunchToken` threaded through. Until then the
    // verified proof binding is the correct, sufficient subject. Empty => no bound wallet, which every
    // caller treats as "no rights" — never as "rights granted".
    let data_dir = state.data_dir.clone();
    let principal = principal_id.to_string();
    tokio::task::spawn_blocking(move || {
        crate::auth::linked_evm_subject_for_principal(&data_dir, &principal).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

// ── PARKED: subject-resolution rewire ─────────────────────────────────────────────────────────
//
// The four functions below decode a wallet-provider `accounts` response into the DRM subject
// wallet. They are INERT: `resolve_subject_address` above returns fail-closed empty because it is
// handed only a `principal_id`, and enumerating a principal's accounts requires a
// `RuntimeWalletAuthority`, which can only be minted from a whole `RequiredHomeLaunchToken`
// (`gateway_home_token::runtime_wallet_authority`).
//
// What is still missing to revive them (deliberately NOT done here — it is a cross-module change,
// not a viewer_open one):
//   1. `resolve_subject_address` must take `&RequiredHomeLaunchToken` instead of `principal_id`.
//      `buy_owned_access` and `mint_create_asset` now hold one (`authorize_money_verb`), but the
//      other six call sites do not: `open_owned_in_viewer` and five reads in `gateway_marketplace.rs`
//      all authenticate with `require_home_token_context`, which discards the launch. Each needs
//      `require_home_token_launch` instead.
//   2. `gateway_wallet_accounts::runtime_wallet_data` is `pub(in crate::api::gateway)`, so
//      `crate::api::viewer_open` cannot call it. Its visibility must widen to `pub(in crate::api)`
//      (or the account listing must move behind a helper that already lives under `gateway`).
//   3. The `ListAccounts` dispatch must stay off the production-route denylist pinned by
//      `gateway_tests::wallet::accounts::raw_wallet_account_dispatch_is_absent_from_production_routes`
//      — that test is what forced the original fail-closed gutting, and it must keep passing.
// Until all three land, keeping these decoders (and their `subject_*` tests) is cheaper than
// re-deriving the default-account precedence rules from scratch.

/// Pick the DRM subject wallet from a wallet-provider `accounts` response.
///
/// The subject is the EVM wallet the runtime binds the open delegation to and runs the on-chain
/// rights check against — it MUST match the wallet the user actually connects with in the browser.
/// So we prefer the principal's DEFAULT signable EVM account (the SAME `browser_connect` /
/// `transaction_intent` default the browser wallet bridge resolves), and only fall back to the
/// first linked EVM account when no usable default is recorded. Picking "first in the list" was a
/// latent bug once a principal has more than one linked EVM account (a second/test account could
/// sort ahead of the MetaMask default and hijack the subject) — see the `subject_*` tests.
#[allow(dead_code)] // parked: subject-resolution rewire
fn select_subject_from_accounts(data: &serde_json::Value) -> String {
    let accounts = data.get("accounts").and_then(|v| v.as_array());
    if let Some(addr) = default_evm_subject_address(data, accounts) {
        return addr;
    }
    // No usable default recorded: fall back to the first linked EVM account (legacy behavior).
    accounts
        .into_iter()
        .flatten()
        .find(|acct| is_eip155(acct))
        .and_then(|acct| acct.get("address").and_then(|v| v.as_str()))
        .map(str::to_string)
        .unwrap_or_default()
}

#[allow(dead_code)] // parked: subject-resolution rewire
fn is_eip155(acct: &serde_json::Value) -> bool {
    acct.get("chain_namespace")
        .and_then(|v| v.as_str())
        .map(|ns| ns.starts_with("eip155:"))
        .unwrap_or(false)
}

#[allow(dead_code)] // parked: subject-resolution rewire
fn is_signable_evm(acct: &serde_json::Value) -> bool {
    is_eip155(acct)
        && acct
            .get("signing_available")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// The principal's default signable EVM account address, resolved the SAME way the browser wallet
/// bridge resolves it: default intents in priority order, newest `set_at` wins, and the referenced
/// account must be a linked, signable EVM account. `None` when no such default exists (caller then
/// falls back to the first linked EVM account).
#[allow(dead_code)] // parked: subject-resolution rewire
fn default_evm_subject_address(
    data: &serde_json::Value,
    accounts: Option<&Vec<serde_json::Value>>,
) -> Option<String> {
    let accounts = accounts?;
    let defaults = data.get("default_accounts").and_then(|v| v.as_array())?;
    let signable_evm_account = |account_id: &str| {
        accounts.iter().find(|acct| {
            acct.get("account_id").and_then(|v| v.as_str()) == Some(account_id)
                && is_signable_evm(acct)
        })
    };
    for intent in ["browser_connect", "transaction_intent"] {
        let chosen = defaults
            .iter()
            .filter(|d| {
                d.get("intent").and_then(|v| v.as_str()) == Some(intent)
                    && d.get("chain_namespace")
                        .and_then(|v| v.as_str())
                        .map(|ns| ns.starts_with("eip155:"))
                        .unwrap_or(false)
                    && d.get("account_id")
                        .and_then(|v| v.as_str())
                        .map(|id| signable_evm_account(id).is_some())
                        .unwrap_or(false)
            })
            .max_by_key(|d| d.get("set_at").and_then(|v| v.as_u64()).unwrap_or(0));
        if let Some(def) = chosen {
            let account_id = def.get("account_id").and_then(|v| v.as_str())?;
            return signable_evm_account(account_id)
                .and_then(|acct| acct.get("address").and_then(|v| v.as_str()))
                .map(str::to_string);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GOLDEN cross-check: this no-shared-dep twin MUST equal `ddrm_envelope::grant_watermark_digest16`
    /// (the embedder). The same `(sig -> digest)` pair is asserted in `ddrm-envelope`'s test, so if
    /// either side changes its hashing or normalization, one of the two assertions fails. The pinned
    /// input exercises the normalization edge cases (mixed-case hex + surrounding whitespace).
    #[test]
    fn grant_watermark_digest_matches_envelope_golden() {
        const GOLDEN: &str = "a9e8be55b175d58849e16689d09a746f";
        // Canonical input.
        assert_eq!(grant_watermark_digest16_hex("0x1234abcd"), GOLDEN);
        // Mixed case + whitespace must normalize to the SAME digest (the only place A could drift).
        assert_eq!(grant_watermark_digest16_hex("  0x1234ABCD  "), GOLDEN);
        // 16 bytes => 32 hex chars.
        assert_eq!(grant_watermark_digest16_hex("anything").len(), 32);
    }

    // --- ESP-2: shared buy/open content-id resolver ------------------------------------------

    /// A minimal but VALID `.ddrm` capsule (passes every `dkms_capsule` gate) pinning `content_id`.
    fn capsule_bytes(content_id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "elastos.ddrm.capsule/v1",
            "content_id": content_id,
            "mime": "image/png",
            "ciphertext_b64": "QUJD",
            "protections": [{
                "scheme": "threshold-shamir-2of3",
                "producer_verifying_key_b64": "cGs=",
            }],
        }))
        .unwrap()
    }

    fn owned_for(
        bytes: Vec<u8>,
        content_cid: Option<&str>,
        name: &str,
    ) -> crate::library::OwnedObjectForViewer {
        crate::library::OwnedObjectForViewer {
            bytes,
            mime: "application/octet-stream".to_string(),
            content_cid: content_cid.map(str::to_string),
            name: name.to_string(),
        }
    }

    /// Byte-for-byte replica of the pre-refactor `/api/viewers/open` derivation (viewer_open.rs
    /// ~321-335) — the source of truth. The shared resolver MUST equal this for every asset.
    fn legacy_open_derivation(owned: &crate::library::OwnedObjectForViewer) -> String {
        let capsule = dkms_capsule(&owned.bytes);
        capsule
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
            })
    }

    /// REGRESSION (ESP-2): for a dKMS-capsule asset whose capsule `content_id` differs from its
    /// `content_cid` AND its name, the shared resolver — used by BOTH open and the legacy re-buy
    /// branch — MUST return the capsule `content_id` (the id the rights gate reads back), NOT the
    /// `content_cid`/`owned:{name}` the old buy path keyed on. Fails on the pre-fix buy derivation.
    #[test]
    fn resolver_prefers_capsule_content_id_over_cid_and_name() {
        let owned = owned_for(
            capsule_bytes("0xabc123thekid"),
            Some("ipfs://a-different-cid"),
            "a-different-name.png",
        );
        assert_eq!(resolve_owned_content_id(&owned), "0xabc123thekid");
        assert_ne!(resolve_owned_content_id(&owned), "ipfs://a-different-cid");
        assert_ne!(
            resolve_owned_content_id(&owned),
            "owned:a-different-name.png"
        );
    }

    /// The shared resolver is byte-identical to the open path's derivation for a capsule asset
    /// (open is unchanged by the refactor).
    #[test]
    fn resolver_matches_open_derivation_for_capsule() {
        let owned = owned_for(capsule_bytes("0xdeadbeef"), Some("ipfs://cid"), "n.png");
        assert_eq!(
            resolve_owned_content_id(&owned),
            legacy_open_derivation(&owned)
        );
        assert_eq!(resolve_owned_content_id(&owned), "0xdeadbeef");
    }

    /// When the capsule omits `content_id` but carries `kid`, both open and buy use the `kid`.
    #[test]
    fn resolver_uses_kid_when_content_id_absent() {
        let bytes = serde_json::to_vec(&json!({
            "schema": "elastos.ddrm.capsule/v1",
            "kid": "0xkidonly",
            "ciphertext_b64": "QUJD",
            "protections": [{ "scheme": "quorum", "producer_verifying_key_b64": "cGs=" }],
        }))
        .unwrap();
        let owned = owned_for(bytes, Some("ipfs://cid"), "n.png");
        assert_eq!(resolve_owned_content_id(&owned), "0xkidonly");
        assert_eq!(
            resolve_owned_content_id(&owned),
            legacy_open_derivation(&owned)
        );
    }

    /// Non-capsule / legacy assets are UNCHANGED: `content_cid` if present, else `owned:{name}`.
    #[test]
    fn resolver_non_capsule_falls_back_to_cid_then_name() {
        let with_cid = owned_for(
            b"not-a-capsule".to_vec(),
            Some("ipfs://legacy-cid"),
            "a.bin",
        );
        assert_eq!(resolve_owned_content_id(&with_cid), "ipfs://legacy-cid");
        assert_eq!(
            resolve_owned_content_id(&with_cid),
            legacy_open_derivation(&with_cid)
        );

        let no_cid = owned_for(b"not-a-capsule".to_vec(), None, "a.bin");
        assert_eq!(resolve_owned_content_id(&no_cid), "owned:a.bin");
        assert_eq!(
            resolve_owned_content_id(&no_cid),
            legacy_open_derivation(&no_cid)
        );
    }

    fn acct(id: &str, ns: &str, addr: &str, signable: bool) -> serde_json::Value {
        json!({ "account_id": id, "chain_namespace": ns, "address": addr, "signing_available": signable })
    }
    fn default_entry(intent: &str, ns: &str, account_id: &str, set_at: u64) -> serde_json::Value {
        json!({ "intent": intent, "chain_namespace": ns, "account_id": account_id, "set_at": set_at })
    }

    /// REGRESSION: with two linked EVM accounts, the subject MUST be the DEFAULT one, not the
    /// first in the list. Mirrors the real bug — a `Test EVM` account sorted ahead of the
    /// MetaMask default and hijacked the on-chain subject.
    #[test]
    fn subject_prefers_default_evm_account() {
        let data = json!({
            "accounts": [
                acct("acct-1", "eip155:8453", "0x4023", true),
                acct("acct-2", "eip155:8453", "0xab50", true),
            ],
            "default_accounts": [default_entry("browser_connect", "eip155:8453", "acct-2", 100)],
        });
        assert_eq!(select_subject_from_accounts(&data), "0xab50");
    }

    /// No default recorded → keep the legacy behavior (first linked EVM account).
    #[test]
    fn subject_falls_back_to_first_evm_when_no_default() {
        let data = json!({
            "accounts": [
                acct("acct-1", "eip155:8453", "0x4023", true),
                acct("acct-2", "eip155:8453", "0xab50", true),
            ],
            "default_accounts": [],
        });
        assert_eq!(select_subject_from_accounts(&data), "0x4023");
    }

    /// A default that points at a missing / non-signable account must NOT be honored; fall back.
    #[test]
    fn subject_ignores_default_pointing_at_unsignable_account() {
        let data = json!({
            "accounts": [
                acct("acct-1", "eip155:8453", "0x4023", true),
                acct("acct-3", "eip155:8453", "0xdead", false),
            ],
            "default_accounts": [default_entry("browser_connect", "eip155:8453", "acct-3", 100)],
        });
        assert_eq!(select_subject_from_accounts(&data), "0x4023");
    }

    /// Newest `set_at` wins among defaults for the same intent (matches the browser bridge).
    #[test]
    fn subject_newest_default_wins_for_same_intent() {
        let data = json!({
            "accounts": [
                acct("a", "eip155:8453", "0xaaa", true),
                acct("b", "eip155:8453", "0xbbb", true),
            ],
            "default_accounts": [
                default_entry("browser_connect", "eip155:8453", "a", 10),
                default_entry("browser_connect", "eip155:8453", "b", 20),
            ],
        });
        assert_eq!(select_subject_from_accounts(&data), "0xbbb");
    }

    /// No EVM accounts at all → empty subject (fail-closed; a chain rights check then denies).
    #[test]
    fn subject_empty_when_no_evm_accounts() {
        let data = json!({ "accounts": [], "default_accounts": [] });
        assert_eq!(select_subject_from_accounts(&data), "");
    }

    /// PRE-2 (log-redaction half): `log_fp` must turn a sensitive identifier into a short,
    /// NON-reversible fingerprint — the raw wallet/content-id must never appear — so the gateway log
    /// cannot persist the `(wallet, content_id)` access pattern. Pinned by the DDRM-verdict ratchet
    /// (`tests/ddrm_verdicts.rs`, PRE-2).
    #[test]
    fn log_fp_redacts_sensitive_identifiers() {
        let wallet = "0xABCDEF0123456789ABCDEF0123456789ABCDEF01";
        let fp = log_fp(wallet);
        assert!(fp.starts_with("fp:"), "must be a fingerprint tag, got {fp}");
        assert!(
            !fp.contains(wallet),
            "the raw value must not appear in the fingerprint"
        );
        assert!(
            fp.len() <= 16,
            "fingerprint must stay short (got {})",
            fp.len()
        );
        assert_eq!(log_fp("   "), "<none>", "blank input must redact to <none>");
        // Deterministic + discriminating: same input -> same fp, different inputs -> different fp.
        assert_eq!(log_fp(wallet), log_fp(wallet));
        assert_ne!(
            log_fp(wallet),
            log_fp("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        );
    }

    // --- Task 2 (ESP connector delegation signing): prepare-grant response carries the default
    // connector id, so the shell can auto-route the delegation signature to it. ---------------

    /// The prepare-grant response object MUST carry `default_connector_id` equal to the resolved
    /// default EVM connector when one exists.
    #[test]
    fn prepare_grant_response_carries_default_connector_id_when_present() {
        let body = prepare_grant_response_json(
            "handle-1",
            "delegation-canonical-text",
            "0xowner",
            "0xkid",
            8453,
            Some("metamask-1"),
        );
        assert_eq!(body["schema"], "elastos.viewer.prepare-grant/v1");
        assert_eq!(body["grant_handle"], "handle-1");
        assert_eq!(body["delegation_canonical"], "delegation-canonical-text");
        assert_eq!(body["owner_address"], "0xowner");
        assert_eq!(body["kid"], "0xkid");
        assert_eq!(body["chain_id"], 8453);
        assert_eq!(body["default_connector_id"], "metamask-1");
    }

    /// No default EVM connector on file -> the field is present but JSON `null`, never omitted (the
    /// shell must be able to distinguish "no default" from a malformed response).
    #[test]
    fn prepare_grant_response_default_connector_id_is_null_when_absent() {
        let body = prepare_grant_response_json(
            "handle-2",
            "delegation-canonical-text-2",
            "0xowner2",
            "0xkid2",
            8453,
            None,
        );
        assert!(body["default_connector_id"].is_null());
    }
}
