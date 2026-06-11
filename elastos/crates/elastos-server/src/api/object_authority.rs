//! Gateway-owned local key authority for owned NON-MEDIA assets (the dDRM viewer
//! seam, document/image/3D side), implemented as a SUBPROCESS relay — the exact
//! analogue of [`super::media_authority`] for whole-object (non-streamed) content.
//!
//! The gateway never links the PQ seal crypto. It spawns the isolated
//! `ddrm-media-authority` helper in `--object` mode (the "local test KMS" adapter),
//! which CENC-wraps the asset as a single-sample fragment under a fresh CEK, launches
//! a SEPARATE rail `decrypt-provider` boundary, seals the CEK to that boundary's
//! in-VM-minted session key, and answers a single `object` read over stdio with ONLY
//! the already-decrypted cleartext bytes. The CEK never reaches the gateway or browser.
//!
//!   Home (authenticated) --POST /api/viewers/ddrm-viewer/object/open-->
//!     gateway spawns ddrm-media-authority --object (-> decrypt-provider), reads a
//!     key-free object descriptor, registers a principal-bound ObjectSession, mints a
//!     ddrm-viewer launch token, and returns a view URL:
//!       /apps/ddrm-viewer/?session={id}&home_token={token}

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
use super::viewer_object::{
    object_view_route, put_object_session, ObjectSession, OBJECT_VIEWER_CAPSULE,
};

/// Compile-time dev-tree default for the isolated media-authority helper (it serves
/// both media and object modes), override with `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN`.
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

/// A live object decrypt session backed by a gateway-spawned local key-authority
/// subprocess. Holds the helper's process handle + the key-free object descriptor;
/// the single object read is relayed to the helper, which returns the already-
/// decrypted cleartext bytes. The gateway never sees the CEK.
pub struct ObjectAuthorityProc {
    child: Child,
    io: Mutex<ProcIo>,
    /// The asset's real content type (from the descriptor).
    pub mime: String,
    /// The cleartext object length in bytes (from the descriptor).
    pub byte_length: usize,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
}

struct ProcIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ObjectAuthorityProc {
    /// Spawn the helper in object mode, read its one-line descriptor, return a handle.
    /// `mime`/`object_cid`, when set, pin the asset's real content type and bind the
    /// decrypt transcript to its content identity (Library-bound open).
    pub fn launch(
        helper_bin: &str,
        decrypt_bin: &str,
        principal_id: &str,
        object_file: Option<&str>,
        mime: Option<&str>,
        object_cid: Option<&str>,
        rights_binding: Option<&str>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(helper_bin);
        cmd.args([
            "--object",
            "--principal",
            principal_id,
            "--decrypt-bin",
            decrypt_bin,
        ]);
        if let Some(object_file) = object_file {
            cmd.args(["--object-file", object_file]);
        }
        if let Some(mime) = mime {
            cmd.args(["--mime", mime]);
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
            .map_err(|e| format!("spawn object-authority ({helper_bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);

        let mut line = String::new();
        let n = stdout
            .read_line(&mut line)
            .map_err(|e| format!("read descriptor: {e}"))?;
        if n == 0 {
            return Err("object-authority exited before publishing a session".to_string());
        }
        let descriptor: Value = serde_json::from_str(line.trim())
            .map_err(|e| format!("parse descriptor: {e} ({line})"))?;
        let mime = descriptor
            .get("mime")
            .and_then(Value::as_str)
            .ok_or("descriptor missing mime")?
            .to_string();
        let byte_length = descriptor
            .get("byte_length")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing byte_length")? as usize;
        let expires_at = descriptor
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing expires_at")?;

        Ok(Self {
            child,
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            byte_length,
            expires_at,
        })
    }

    /// Relay the object read; returns the already-decrypted cleartext bytes.
    pub fn object(&self) -> Result<Vec<u8>, String> {
        let mut io = self.io.lock().map_err(|_| "object-authority mutex poisoned")?;
        writeln!(io.stdin, "{}", json!({ "op": "object" }))
            .map_err(|e| format!("write object request: {e}"))?;
        io.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let mut line = String::new();
        let n = io
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("read object response: {e}"))?;
        if n == 0 {
            return Err("object-authority closed its stdout".to_string());
        }
        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("parse response: {e}"))?;
        if resp.get("status").and_then(Value::as_str) != Some("ok") {
            let message = resp
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("object-authority error");
            return Err(message.to_string());
        }
        let b64 = resp
            .get("object_b64")
            .and_then(Value::as_str)
            .ok_or("response missing object_b64")?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode object_b64: {e}"))
    }
}

impl Drop for ObjectAuthorityProc {
    fn drop(&mut self) {
        if let Ok(mut io) = self.io.lock() {
            let _ = writeln!(io.stdin, "{}", json!({ "op": "shutdown" }));
            let _ = io.stdin.flush();
        }
        let _ = self.child.wait();
    }
}

/// POST /api/viewers/ddrm-viewer/object/open — stand up an owned non-media view
/// session through a gateway-spawned local key-authority and return a view URL.
pub async fn open_owned_object(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let principal_id = context.principal_id.clone();

    let helper_bin = std::env::var("ELASTOS_DDRM_MEDIA_AUTHORITY_BIN")
        .unwrap_or_else(|_| DEV_MEDIA_AUTHORITY_BIN.to_string());
    let decrypt_bin =
        std::env::var("ELASTOS_DDRM_DECRYPT_BIN").unwrap_or_else(|_| DEV_DECRYPT_BIN.to_string());
    let object_file = std::env::var("ELASTOS_DDRM_SAMPLE_OBJECT").ok();
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

    // Spawning runs the seal handshake — keep it off the async runtime.
    let built = tokio::task::spawn_blocking(move || {
        ObjectAuthorityProc::launch(
            &helper_bin,
            &decrypt_bin,
            &principal_id,
            object_file.as_deref(),
            None,
            None,
            None,
        )
    })
    .await;
    let proc = match built {
        Ok(Ok(proc)) => Arc::new(proc),
        Ok(Err(err)) => {
            tracing::warn!("object open failed: {err}");
            return (StatusCode::BAD_GATEWAY, format!("could not open owned asset: {err}"))
                .into_response();
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "object open task panicked").into_response()
        }
    };

    let session_id = random_session_id();
    let mime = proc.mime.clone();
    let byte_length = proc.byte_length;
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

    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(json!({
            "schema": "elastos.viewer.object.open/v1",
            "viewer": OBJECT_VIEWER_CAPSULE,
            "session": session_id,
            "mime": mime,
            "byte_length": byte_length,
            "play_url": view_url,
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
