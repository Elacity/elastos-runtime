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
//!       /apps/ddrm-viewer/?session={id}#home_token={token}   (token in the FRAGMENT, so it
//!       is never sent to a server: not in Referer, access logs, or shared history)

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
    /// Pixel-lock: the asset is served as flattened, watermarked page images (the raw bytes
    /// never leave the decrypt boundary). When true, the object read is refused and the page
    /// route is the only way to view it.
    pub pixel_locked: bool,
    /// For pixel-lock assets, the document's page count (from the descriptor).
    pub total_pages: u32,
    /// For pixel-lock assets, the content type of each rendered page (e.g. `image/jpeg`).
    pub page_content_type: String,
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
        // Guard the raw child so EVERY early return below reaps it (council S42 red-team F1): a
        // hostile sidecar can force a bail via a well-formed-but-incomplete descriptor, and a bare
        // `?` there would otherwise leak a live process. `disarm()` hands the child to the session
        // only on the success path.
        let mut guard = super::capsule_watchdog::ReapGuard::new(
            super::capsule_watchdog::spawn_grouped(
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit()),
            )
            .map_err(|e| format!("spawn object-authority ({helper_bin}): {e}"))?,
        );
        let stdin = guard.child_mut().stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(guard.child_mut().stdout.take().ok_or("no stdout")?);

        // Bound the OPEN read (Sprint 42) with the content-open deadline (the sidecar transcodes +
        // seals before answering): a hung object-authority is group-killed and a fire DENIES
        // (fail-closed). On any Err, `guard` reaps the (possibly-killed) child on drop — no zombie.
        let descriptor = super::capsule_watchdog::read_open_deadlined(
            guard.pid(),
            &mut stdout,
            "object-authority",
        )?;
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
            child: guard.disarm(),
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            byte_length,
            expires_at,
            pixel_locked: false,
            total_pages: 0,
            page_content_type: String::new(),
        })
    }

    /// Spawn the helper in `--quorum` mode (the dKMS consumer-open): it recovers the asset's
    /// REAL CEK from the 2-of-3 quorum named by the `.ddrm` capsule, decrypts, and serves the
    /// byte-identical cleartext over the SAME descriptor + `object` protocol as `launch`. The
    /// gateway never links the PQ crypto and never sees the CEK; this is the production analogue
    /// of the local-test-KMS `launch`.
    #[allow(clippy::too_many_arguments)]
    pub fn launch_quorum(
        helper_bin: &str,
        decrypt_bin: &str,
        key_bin: &str,
        principal_id: &str,
        capsule_file: &str,
        descriptor_file: &str,
        caller_seed_b64: &str,
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
        // Guard the raw child so every early return reaps it (council S42 red-team F1).
        let mut guard = super::capsule_watchdog::ReapGuard::new(
            super::capsule_watchdog::spawn_grouped(
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit()),
            )
            .map_err(|e| format!("spawn quorum-authority ({helper_bin}): {e}"))?,
        );
        let stdin = guard.child_mut().stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(guard.child_mut().stdout.take().ok_or("no stdout")?);

        // Bound the OPEN read (Sprint 42) with the content-open deadline; a fire DENIES fail-closed.
        // On any Err below, `guard` reaps the (possibly-killed) child on drop — no zombie.
        let descriptor = super::capsule_watchdog::read_open_deadlined(
            guard.pid(),
            &mut stdout,
            "quorum-authority",
        )?;
        let mime = descriptor
            .get("mime")
            .and_then(Value::as_str)
            .unwrap_or(mime)
            .to_string();
        let byte_length = descriptor
            .get("byte_length")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing byte_length")? as usize;
        let expires_at = descriptor
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or("descriptor missing expires_at")?;
        // Pixel-lock metadata: present (true) when the helper renders this asset to page images
        // in-boundary (e.g. PDFs). Absent ⇒ a raw object read, as before.
        let pixel_locked = descriptor
            .get("pixel_locked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let total_pages = descriptor
            .get("total_pages")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let page_content_type = descriptor
            .get("page_content_type")
            .and_then(Value::as_str)
            .unwrap_or("image/jpeg")
            .to_string();

        Ok(Self {
            child: guard.disarm(),
            io: Mutex::new(ProcIo { stdin, stdout }),
            mime,
            byte_length,
            expires_at,
            pixel_locked,
            total_pages,
            page_content_type,
        })
    }

    /// Relay a pixel-lock page read; returns that page's rendered (watermarked) image bytes and
    /// the document's page count. The raw object never crosses this boundary — only the image.
    pub fn object_page(&self, n: u32) -> Result<(Vec<u8>, u32), String> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| "object-authority mutex poisoned")?;
        writeln!(io.stdin, "{}", json!({ "op": "page", "n": n }))
            .map_err(|e| format!("write page request: {e}"))?;
        io.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        // Bound the VIEW read (Sprint 42): a hung page relay is group-killed at the deadline so no
        // request thread parks; a fire DENIES fail-closed. The child is reaped at Drop.
        let resp = super::capsule_watchdog::read_line_deadlined(
            self.child.id(),
            &mut io.stdout,
            "object-authority",
        )?;
        if resp.get("status").and_then(Value::as_str) != Some("ok") {
            let message = resp
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("object-authority error");
            return Err(message.to_string());
        }
        let b64 = resp
            .get("page_b64")
            .and_then(Value::as_str)
            .ok_or("response missing page_b64")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode page_b64: {e}"))?;
        let total_pages = resp
            .get("total_pages")
            .and_then(Value::as_u64)
            .unwrap_or(self.total_pages as u64) as u32;
        Ok((bytes, total_pages))
    }

    /// Relay the object read; returns the already-decrypted cleartext bytes.
    pub fn object(&self) -> Result<Vec<u8>, String> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| "object-authority mutex poisoned")?;
        writeln!(io.stdin, "{}", json!({ "op": "object" }))
            .map_err(|e| format!("write object request: {e}"))?;
        io.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        // Bound the VIEW read (Sprint 42): a hung object relay is group-killed at the deadline; a
        // fire DENIES fail-closed. The child is reaped at Drop.
        let resp = super::capsule_watchdog::read_line_deadlined(
            self.child.id(),
            &mut io.stdout,
            "object-authority",
        )?;
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
        // BOUNDED reap (Sprint 42): an authority that ignores shutdown/EOF must not park the
        // dropping thread on an unbounded `wait()` — group-kill it after a short grace.
        super::capsule_watchdog::reap_grouped(&mut self.child);
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
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not open owned asset: {err}"),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "object open task panicked",
            )
                .into_response()
        }
    };

    let session_id = random_session_id();
    let mime = proc.mime.clone();
    let byte_length = proc.byte_length;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 42 ratchet: a HUNG object-authority (the VIEW-path open) is killed at the deadline —
    /// `launch` returns BOUNDED (never parks the open thread for the child's lifetime), the killed
    /// child is reaped (no zombie), and the error carries the access marker + says access is
    /// DENIED (fail-closed, the mirror of the rights-decide rule).
    #[test]
    #[cfg(unix)]
    fn a_hung_object_authority_is_killed_and_access_is_denied() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        let prior_open = std::env::var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");
        // The OPEN descriptor read uses the content-open deadline (not the chain knob).
        std::env::set_var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hung-object-authority.sh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let err = match ObjectAuthorityProc::launch(
            stub.to_str().unwrap(),
            "unused-decrypt-bin",
            "did:elastos:test-principal",
            None,
            None,
            None,
            None,
        ) {
            Ok(_) => panic!("expected the hung object-authority to be killed, not a live open"),
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

    /// Sprint 42 ratchet (the VIEW leg, distinct from the open): a sidecar that answers the
    /// descriptor fast (so `launch` succeeds and the open watchdog is disarmed) but then HANGS on
    /// the per-op `object` read is STILL bounded — the view read arms its own watchdog, the hung
    /// relay is killed, and the read DENIES (fail-closed). Pins the "every later op is separately
    /// bounded" property the persistent-session doc asserts.
    #[test]
    #[cfg(unix)]
    fn a_hung_object_view_read_is_bounded_and_denied() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("answers-then-hangs.sh");
        // Emit a valid one-line descriptor, THEN hang on the first op read.
        std::fs::write(
            &stub,
            "#!/bin/sh\nprintf '{\"mime\":\"application/octet-stream\",\
             \"byte_length\":3,\"expires_at\":9999999999}\\n'\nsleep 300\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let proc = ObjectAuthorityProc::launch(
            stub.to_str().unwrap(),
            "unused-decrypt-bin",
            "did:elastos:test-principal",
            None,
            None,
            None,
            None,
        )
        .expect("the descriptor is answered, so the open itself succeeds");

        let started = std::time::Instant::now();
        let err = proc.object().unwrap_err();
        match prior {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the VIEW read is BOUNDED by the deadline, not the child's 300s sleep"
        );
        assert!(
            err.contains(crate::api::capsule_watchdog::ACCESS_DEADLINE_MARKER)
                && err.contains("DENIED"),
            "a hung per-op view read DENIES access (fail-closed): {err}"
        );
    }
}
