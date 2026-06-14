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

use std::io::{BufRead, BufReader, Write};
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
    std::env::var("ELASTOS_DDRM_KEY_PROVIDER_BIN").unwrap_or_else(|_| DEV_KEY_PROVIDER_BIN.to_string())
}

/// A live media decrypt session backed by a gateway-spawned local key-authority
/// subprocess. Holds the helper's process handle + the key-free session descriptor;
/// each segment read is relayed to the helper, which returns already-decrypted,
/// `senc`-stripped bytes. The gateway never sees the CEK.
pub struct MediaAuthorityProc {
    child: Child,
    io: Mutex<ProcIo>,
    /// MSE `addSourceBuffer` mime/codecs string (from the descriptor).
    pub mime: String,
    /// Number of addressable media segments.
    pub segment_count: usize,
    /// The clear init segment bytes (CENC init is unencrypted).
    pub init_bytes: Vec<u8>,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
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
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn media-authority ({helper_bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);

        let mut line = String::new();
        let n = stdout
            .read_line(&mut line)
            .map_err(|e| format!("read descriptor: {e}"))?;
        if n == 0 {
            return Err("media-authority exited before publishing a session".to_string());
        }
        let descriptor: Value = serde_json::from_str(line.trim())
            .map_err(|e| format!("parse descriptor: {e} ({line})"))?;
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
            child,
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            segment_count,
            init_bytes,
            expires_at,
        })
    }

    /// Relay one segment read; returns already-decrypted, `senc`-stripped bytes.
    pub fn segment(&self, index: usize) -> Result<Vec<u8>, String> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| "media-authority mutex poisoned")?;
        writeln!(io.stdin, "{}", json!({ "op": "segment", "index": index }))
            .map_err(|e| format!("write segment request: {e}"))?;
        io.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let mut line = String::new();
        let n = io
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("read segment response: {e}"))?;
        if n == 0 {
            return Err("media-authority closed its stdout".to_string());
        }
        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("parse response: {e}"))?;
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
        let _ = self.child.wait();
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
