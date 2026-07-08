//! Gateway-owned local key authority for owned-media playback (the dDRM viewer
//! seam folded into the product), implemented as a SUBPROCESS relay.
//!
//! The gateway never links the PQ seal crypto (it would feature-clash with the
//! existing ed25519-dalek 3.0-pre / iroh stack). Instead it spawns an isolated
//! `ddrm-media-authority` helper — the "local test KMS" adapter — which CENC-packs
//! the asset, launches a SEPARATE rail `decrypt-provider` boundary, seals the CEK
//! to that boundary's in-VM-minted session key, and answers per-segment reads over
//! a tiny stdio protocol with ONLY already-decrypted, `senc`-stripped bytes.
//!
//!   Home (authenticated) --POST /api/viewers/elacity-player/media/open-->
//!     gateway spawns ddrm-media-authority (-> decrypt-provider), reads a key-free
//!     session descriptor, registers a principal-bound MediaSession, mints an
//!     elacity-player launch token, and returns a play URL:
//!       /apps/elacity-player/?session={id}&home_token={token}
//!
//! Per Anders: key-authority and decrypt boundary stay SEPARATE processes; the
//! runtime relays only sealed material + scoped output; the CEK never reaches the
//! gateway or the browser; the viewer gets decrypted segment bytes only.

use std::io::{BufReader, Write};
use std::path::Path as FsPath;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde_json::{json, Value};

use super::gateway::{
    issue_home_launch_token_with_context, require_home_token_context, GatewayState,
};
use super::viewer_media::{
    media_play_route, put_media_session, MediaSession, MEDIA_VIEWER_CAPSULE,
};

/// Compile-time dev-tree default for the isolated media-authority helper, override
/// with `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN`.
const DEV_MEDIA_AUTHORITY_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../scripts/dev/ddrm-media-authority/target/debug/ddrm-media-authority"
);
/// Compile-time dev-tree default for the rail decrypt-provider boundary, override
/// with `ELASTOS_DDRM_DECRYPT_BIN`.
const DEV_DECRYPT_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/decrypt-provider/target/debug/decrypt-provider"
);

/// Resolve the local key-authority helper binary (env override or dev-tree default).
pub(crate) fn resolve_helper_bin() -> String {
    std::env::var("ELASTOS_DDRM_MEDIA_AUTHORITY_BIN")
        .unwrap_or_else(|_| DEV_MEDIA_AUTHORITY_BIN.to_string())
}

/// Resolve the rail decrypt-provider binary (env override or dev-tree default).
pub(crate) fn resolve_decrypt_bin() -> String {
    std::env::var("ELASTOS_DDRM_DECRYPT_BIN").unwrap_or_else(|_| DEV_DECRYPT_BIN.to_string())
}

/// Compile-time dev-tree default for the key-provider (dkms backend) the quorum consumer-open
/// drives, override with `ELASTOS_DDRM_KEY_PROVIDER_BIN`.
const DEV_KEY_PROVIDER_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/key-provider/target/debug/key-provider"
);

/// Resolve the key-provider binary (env override or dev-tree default) for `--quorum` opens.
pub(crate) fn resolve_key_bin() -> String {
    std::env::var("ELASTOS_DDRM_KEY_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_KEY_PROVIDER_BIN.to_string())
}

/// A live media decrypt session backed by a gateway-spawned local key-authority
/// subprocess. Holds the helper's process handle + the key-free session descriptor;
/// each segment read is relayed to the helper, which returns already-decrypted,
/// `senc`-stripped bytes. The gateway never sees the CEK.
pub struct MediaAuthorityProc {
    child: Child,
    io: Mutex<ProcIo>,
    /// MSE `addSourceBuffer` mime/codecs string (from the descriptor). For multi-track sessions
    /// this is track 0's mime; per-track mimes live in `tracks`.
    pub mime: String,
    /// Number of addressable media segments (track 0's count for multi-track sessions).
    pub segment_count: usize,
    /// The clear init segment bytes (track 0's init for multi-track sessions).
    pub init_bytes: Vec<u8>,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
    /// Per-track playback model (video + audio for split DASH; one entry for muxed/audio). Empty
    /// for the legacy local-muxed `launch` path, which uses the single fields above + `/segment`.
    pub tracks: Vec<AuthorityTrack>,
}

/// One playable track relayed by the helper: its MSE mime, clear init, and segment count.
/// The player attaches one `SourceBuffer` per entry (PC2's two-viewer model).
pub struct AuthorityTrack {
    pub kind: String,
    pub mime: String,
    pub segment_count: usize,
    pub init_bytes: Vec<u8>,
}

struct ProcIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl MediaAuthorityProc {
    /// Spawn the helper, read its one-line session descriptor, and return a handle.
    /// `object_cid`, when set, binds the decrypt transcript to the real owned object's
    /// content identity (Library-bound open); `None` uses the demo default.
    pub fn launch(
        helper_bin: &str,
        decrypt_bin: &str,
        principal_id: &str,
        video: Option<&str>,
        object_cid: Option<&str>,
        rights_binding: Option<&str>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(helper_bin);
        cmd.args(["--principal", principal_id, "--decrypt-bin", decrypt_bin]);
        if let Some(video) = video {
            cmd.args(["--video", video]);
        }
        if let Some(object_cid) = object_cid {
            cmd.args(["--object-cid", object_cid]);
        }
        if let Some(binding) = rights_binding {
            cmd.args(["--rights-binding", binding]);
        }
        // Guard the raw child so EVERY early return below reaps it (the shared `ReapGuard`): a
        // hostile sidecar can force a bail via a malformed/incomplete descriptor, and a bare `?`
        // there would otherwise leak a live process (the ELACITY-2282 Defect B class).
        let mut guard = super::capsule_watchdog::ReapGuard::new(
            super::capsule_watchdog::spawn_grouped(
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit()),
            )
            .map_err(|e| format!("spawn media-authority ({helper_bin}): {e}"))?,
        );
        let stdin = guard.child_mut().stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(guard.child_mut().stdout.take().ok_or("no stdout")?);

        // Bound the OPEN read (Sprint 42) with the content-open deadline (the helper runs ffmpeg +
        // the seal handshake before answering): a hung media-authority is group-killed and a fire
        // DENIES (fail-closed). On any Err, `guard` reaps the (possibly-killed) child — no zombie.
        let descriptor = super::capsule_watchdog::read_open_deadlined(
            guard.pid(),
            &mut stdout,
            "media-authority",
        )?;
        let mime = descriptor
            .get("mime")
            .and_then(Value::as_str)
            .ok_or("descriptor missing mime")?
            .to_string();
        let segment_count = descriptor
            .get("segment_count")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing segment_count")? as usize;
        let expires_at = descriptor
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing expires_at")?;
        let init_bytes = descriptor
            .get("init_b64")
            .and_then(Value::as_str)
            .map(|b64| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| format!("decode init_b64: {e}"))
            })
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            child: guard.disarm(),
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            segment_count,
            init_bytes,
            expires_at,
            tracks: Vec::new(),
        })
    }

    /// Spawn the helper in `--quorum` media mode (the dKMS consumer-open for DASH): it recovers
    /// the asset's REAL CEK from the 2-of-3 quorum named by the `.ddrm` capsule, reads the
    /// pre-fetched DASH directory (`dash_dir`), and serves CENC-decrypted fMP4 fragments over the
    /// SAME descriptor + `segment` protocol as `launch`. The gateway never links the PQ crypto and
    /// never sees the CEK; this is the production analogue of the local-test-KMS `launch` for media.
    #[allow(clippy::too_many_arguments)]
    pub fn launch_quorum(
        helper_bin: &str,
        decrypt_bin: &str,
        key_bin: &str,
        principal_id: &str,
        capsule_file: &str,
        descriptor_file: &str,
        caller_seed_b64: &str,
        dash_dir: &str,
        object_cid: &str,
        mime: &str,
        access_grant_b64: Option<&str>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(helper_bin);
        cmd.args([
            "--quorum",
            "--principal",
            principal_id,
            "--decrypt-bin",
            decrypt_bin,
            "--key-bin",
            key_bin,
            "--capsule",
            capsule_file,
            "--descriptor",
            descriptor_file,
            "--caller-seed",
            caller_seed_b64,
            "--dash-dir",
            dash_dir,
            "--object-cid",
            object_cid,
            "--mime",
            mime,
        ]);
        // TRUSTLESS AUTHORIZATION: forward the wallet-signed grant (base64 JSON) so the nodes
        // verify the wallet + read the chain themselves. Absent => legacy enrolled-caller path.
        if let Some(grant) = access_grant_b64.filter(|s| !s.trim().is_empty()) {
            cmd.args(["--access-grant", grant]);
        }
        // Guard the raw child so every early return reaps it (the shared `ReapGuard`).
        let mut guard = super::capsule_watchdog::ReapGuard::new(
            super::capsule_watchdog::spawn_grouped(
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit()),
            )
            .map_err(|e| format!("spawn quorum media-authority ({helper_bin}): {e}"))?,
        );
        let stdin = guard.child_mut().stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(guard.child_mut().stdout.take().ok_or("no stdout")?);

        // Bound the OPEN read (Sprint 42) with the content-open deadline; a fire DENIES fail-closed.
        // On any Err below, `guard` reaps the (possibly-killed) child — no zombie.
        let descriptor = super::capsule_watchdog::read_open_deadlined(
            guard.pid(),
            &mut stdout,
            "quorum media-authority",
        )?;
        let expires_at = descriptor
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing expires_at")?;

        // Multi-track descriptor (`tracks[]`): each AdaptationSet carries its own mime + clear init
        // + segment count. The legacy single fields are populated from track 0 so existing readers
        // keep working; the player drives per-track `/track/{t}/...` routes from `tracks`.
        let decode_init = |d: &Value| -> Result<Vec<u8>, String> {
            d.get("init_b64")
                .and_then(Value::as_str)
                .map(|b64| {
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| format!("decode init_b64: {e}"))
                })
                .transpose()
                .map(Option::unwrap_or_default)
        };
        let mut tracks: Vec<AuthorityTrack> = Vec::new();
        if let Some(arr) = descriptor.get("tracks").and_then(Value::as_array) {
            for t in arr {
                tracks.push(AuthorityTrack {
                    kind: t
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    mime: t
                        .get("mime")
                        .and_then(Value::as_str)
                        .unwrap_or(mime)
                        .to_string(),
                    segment_count: t
                        .get("segment_count")
                        .and_then(Value::as_u64)
                        .ok_or("track descriptor missing segment_count")?
                        as usize,
                    init_bytes: decode_init(t)?,
                });
            }
        }
        let (mime, segment_count, init_bytes) = if let Some(t0) = tracks.first() {
            (t0.mime.clone(), t0.segment_count, t0.init_bytes.clone())
        } else {
            // Legacy single-track descriptor (top-level mime/segment_count/init_b64).
            let mime = descriptor
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or(mime)
                .to_string();
            let segment_count = descriptor
                .get("segment_count")
                .and_then(Value::as_u64)
                .ok_or("descriptor missing segment_count")?
                as usize;
            (mime, segment_count, decode_init(&descriptor)?)
        };

        Ok(Self {
            child: guard.disarm(),
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            segment_count,
            init_bytes,
            expires_at,
            tracks,
        })
    }

    /// Relay one segment read; returns already-decrypted, `senc`-stripped bytes.
    pub fn segment(&self, index: usize) -> Result<Vec<u8>, String> {
        self.segment_op(json!({ "op": "segment", "index": index }))
    }

    /// Relay one PER-TRACK segment read (`{track, index}`): the helper maps the track-local index
    /// to the global flat CENC-counter position before decrypt. Used by multi-track playback.
    pub fn segment_track(&self, track: usize, index: usize) -> Result<Vec<u8>, String> {
        self.segment_op(json!({ "op": "segment", "track": track, "index": index }))
    }

    fn segment_op(&self, request: Value) -> Result<Vec<u8>, String> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| "media-authority mutex poisoned")?;
        writeln!(io.stdin, "{request}").map_err(|e| format!("write segment request: {e}"))?;
        io.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        // Bound the VIEW read (Sprint 42): a hung segment relay is group-killed at the deadline so
        // no playback thread parks; a fire DENIES fail-closed. The child is reaped at Drop.
        let resp = super::capsule_watchdog::read_line_deadlined(
            self.child.id(),
            &mut io.stdout,
            "media-authority",
        )?;
        if resp.get("status").and_then(Value::as_str) != Some("ok") {
            let message = resp
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("media-authority error");
            return Err(message.to_string());
        }
        let b64 = resp
            .get("segment_b64")
            .and_then(Value::as_str)
            .ok_or("response missing segment_b64")?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode segment_b64: {e}"))
    }
}

impl Drop for MediaAuthorityProc {
    fn drop(&mut self) {
        if let Ok(mut io) = self.io.lock() {
            let _ = writeln!(io.stdin, "{}", json!({ "op": "shutdown" }));
            let _ = io.stdin.flush();
        }
        // BOUNDED reap (Sprint 42): an authority that ignores shutdown/EOF must not park the
        // dropping thread on an unbounded `wait()` — group-kill it after a short grace.
        super::capsule_watchdog::reap_grouped(&mut self.child);
    }
}

/// POST /api/viewers/elacity-player/media/open — stand up an owned-media play
/// session through a gateway-spawned local key-authority and return a play URL.
pub async fn open_demo_media(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();

    let helper_bin = std::env::var("ELASTOS_DDRM_MEDIA_AUTHORITY_BIN")
        .unwrap_or_else(|_| DEV_MEDIA_AUTHORITY_BIN.to_string());
    let decrypt_bin =
        std::env::var("ELASTOS_DDRM_DECRYPT_BIN").unwrap_or_else(|_| DEV_DECRYPT_BIN.to_string());
    let video = std::env::var("ELASTOS_DDRM_SAMPLE_VIDEO").ok();
    if !FsPath::new(&helper_bin).is_file() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "media-authority helper not found at {helper_bin}; build it with \
                 `cargo build` in scripts/dev/ddrm-media-authority or set \
                 ELASTOS_DDRM_MEDIA_AUTHORITY_BIN"
            ),
        )
            .into_response();
    }

    // Spawning runs ffmpeg + the seal handshake — keep it off the async runtime.
    let built = tokio::task::spawn_blocking(move || {
        MediaAuthorityProc::launch(
            &helper_bin,
            &decrypt_bin,
            &principal_id,
            video.as_deref(),
            None,
            None,
        )
    })
    .await;
    let proc = match built {
        Ok(Ok(proc)) => Arc::new(proc),
        Ok(Err(err)) => {
            tracing::warn!("media open failed: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not open owned media: {err}"),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "media open task panicked",
            )
                .into_response()
        }
    };

    let session_id = random_session_id();
    let segment_count = proc.segment_count;
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

    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(json!({
            "schema": "elastos.viewer.media.open/v1",
            "viewer": MEDIA_VIEWER_CAPSULE,
            "session": session_id,
            "segment_count": segment_count,
            "play_url": play_url,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 42 ratchet: a HUNG media-authority (the streamed OPEN) is killed at the deadline —
    /// `launch` returns BOUNDED (never parks the open thread for the child's lifetime), the killed
    /// child is reaped by the `ChildReaper` guard (no zombie), and the error carries the access
    /// marker + says access is DENIED (fail-closed).
    #[test]
    #[cfg(unix)]
    fn a_hung_media_authority_is_killed_and_access_is_denied() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        let prior_open = std::env::var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");
        // The OPEN descriptor read uses the content-open deadline (not the chain knob).
        std::env::set_var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hung-media-authority.sh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let err = match MediaAuthorityProc::launch(
            stub.to_str().unwrap(),
            "unused-decrypt-bin",
            "did:elastos:test-principal",
            None,
            None,
            None,
        ) {
            Ok(_) => panic!("expected the hung media-authority to be killed, not a live open"),
            Err(e) => e,
        };
        match prior {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }
        match prior_open {
            Some(v) => std::env::set_var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS"),
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the OPEN is BOUNDED by the deadline, not the child's 300s sleep"
        );
        assert!(
            err.contains(crate::api::capsule_watchdog::ACCESS_DEADLINE_MARKER),
            "the error carries the access-deadline marker: {err}"
        );
        assert!(
            err.contains("DENIED"),
            "a content-open timeout DENIES access (fail-closed): {err}"
        );
    }
}
