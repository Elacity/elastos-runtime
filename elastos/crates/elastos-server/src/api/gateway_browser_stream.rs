//! Browser Net/Exit stream gateway helpers.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;
#[cfg(unix)]
use tokio::io::{copy_bidirectional, AsyncWriteExt as _};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

const BROWSER_RUNTIME_STREAM_TMP_DIR: &str = "elastos-browser-streams";

pub(in crate::api::gateway) async fn gateway_browser_net_http(
    registry: &ProviderRegistry,
    request: &serde_json::Value,
) -> Response {
    let validation = match registry.send_raw("net", request).await {
        Ok(value) => value,
        Err(err) => {
            return gateway_provider_error_response(
                "net",
                anyhow::anyhow!("net provider unavailable: {}", err),
            )
        }
    };
    let exit_handoff_message = validation
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("Net provider requested internal Exit handoff")
        .to_string();
    match validation.get("status").and_then(|value| value.as_str()) {
        Some("ok") => return Json(validation).into_response(),
        Some("error")
            if validation.get("code").and_then(|value| value.as_str())
                == Some("exit_unavailable") =>
        {
            // Net validated the browser request and refused ambient networking.
            // Runtime owns the handoff to the internal Exit provider.
        }
        Some("error") => {
            let message = validation
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("net provider rejected Browser request");
            return gateway_provider_error_response("net", anyhow::anyhow!(message.to_string()));
        }
        _ => {
            return gateway_provider_error_response(
                "net",
                anyhow::anyhow!("net provider returned an invalid response"),
            )
        }
    }

    let exit_request = serde_json::json!({
        "op": "http_fetch",
        "url": request.get("url").cloned().unwrap_or(serde_json::Value::Null),
        "method": request.get("method").cloned().unwrap_or_else(|| serde_json::json!("GET")),
        "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
        "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
    });
    let response = match registry.send_raw("exit", &exit_request).await {
        Ok(value) => value,
        Err(err) => {
            return gateway_provider_error_response(
                "exit",
                anyhow::anyhow!(
                    "exit provider unavailable: {}; {}",
                    exit_handoff_message,
                    err
                ),
            )
        }
    };
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let code = response
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("provider_error");
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("exit provider rejected Browser request");
        if matches!(code, "exit_unavailable" | "backend_error") {
            return gateway_provider_error_response(
                "exit",
                anyhow::anyhow!("exit provider unavailable: {}", message),
            );
        }
        return gateway_provider_error_response("exit", anyhow::anyhow!(message.to_string()));
    }
    Json(response).into_response()
}

pub(in crate::api::gateway) async fn gateway_browser_net_stream(
    registry: &ProviderRegistry,
    request: &serde_json::Value,
) -> Response {
    let validation = match registry.send_raw("net", request).await {
        Ok(value) => value,
        Err(err) => {
            return gateway_provider_error_response(
                "net",
                anyhow::anyhow!("net provider unavailable: {}", err),
            )
        }
    };
    let exit_handoff_message = validation
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("Net provider requested internal Exit handoff")
        .to_string();
    match validation.get("status").and_then(|value| value.as_str()) {
        Some("ok") => return Json(validation).into_response(),
        Some("error")
            if validation.get("code").and_then(|value| value.as_str())
                == Some("exit_unavailable") => {}
        Some("error") => {
            let message = validation
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("net provider rejected Browser stream request");
            return gateway_provider_error_response("net", anyhow::anyhow!(message.to_string()));
        }
        _ => {
            return gateway_provider_error_response(
                "net",
                anyhow::anyhow!("net provider returned an invalid response"),
            )
        }
    }

    let exit_request = serde_json::json!({
        "op": "open_stream",
        "target": request.get("target").cloned().unwrap_or(serde_json::Value::Null),
        "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
        "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
    });
    let response = match registry.send_raw("exit", &exit_request).await {
        Ok(value) => value,
        Err(err) => {
            return gateway_provider_error_response(
                "exit",
                anyhow::anyhow!(
                    "exit provider unavailable: {}; {}",
                    exit_handoff_message,
                    err
                ),
            )
        }
    };
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let code = response
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("provider_error");
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("exit provider rejected Browser stream request");
        if matches!(code, "exit_unavailable" | "backend_error") {
            return gateway_provider_error_response(
                "exit",
                anyhow::anyhow!("exit provider unavailable: {}", message),
            );
        }
        return gateway_provider_error_response("exit", anyhow::anyhow!(message.to_string()));
    }
    Json(response).into_response()
}

pub(in crate::api::gateway) async fn browser_reserve_stream_session(
    registry: &ProviderRegistry,
    request: &serde_json::Value,
) -> Result<serde_json::Value, (&'static str, anyhow::Error)> {
    let net_call = browser_provider_resource_call(
        "net",
        "stream",
        "elastos://net/stream".to_string(),
        request.clone(),
    )
    .map_err(|(_status, message)| ("net", anyhow::anyhow!(message)))?;
    let validation = registry
        .send_raw(net_call.scheme, &net_call.request)
        .await
        .map_err(|err| ("net", anyhow::anyhow!("net provider unavailable: {}", err)))?;
    let exit_handoff_message = validation
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("Net provider requested internal Exit handoff")
        .to_string();
    match validation.get("status").and_then(|value| value.as_str()) {
        Some("ok") => {
            let receipt = provider_response_data(&validation).ok_or_else(|| {
                (
                    "net",
                    anyhow::anyhow!("net provider returned an invalid stream response"),
                )
            })?;
            return validate_browser_stream_receipt(receipt).map_err(|err| ("net", err));
        }
        Some("error")
            if validation.get("code").and_then(|value| value.as_str())
                == Some("exit_unavailable") => {}
        Some("error") => {
            let message = validation
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("net provider rejected Browser stream request");
            return Err(("net", anyhow::anyhow!(message.to_string())));
        }
        _ => {
            return Err((
                "net",
                anyhow::anyhow!("net provider returned an invalid response"),
            ))
        }
    }

    let exit_call = browser_provider_resource_call(
        "exit",
        "open_stream",
        "elastos://exit/open_stream".to_string(),
        serde_json::json!({
            "target": request.get("target").cloned().unwrap_or(serde_json::Value::Null),
            "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
            "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
        }),
    )
    .map_err(|(_status, message)| ("exit", anyhow::anyhow!(message)))?;
    let response = registry
        .send_raw(exit_call.scheme, &exit_call.request)
        .await
        .map_err(|err| {
            (
                "exit",
                anyhow::anyhow!(
                    "exit provider unavailable: {}; {}",
                    exit_handoff_message,
                    err
                ),
            )
        })?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("exit provider rejected Browser stream request");
        return Err(("exit", anyhow::anyhow!(message.to_string())));
    }
    let receipt = provider_response_data(&response).ok_or_else(|| {
        (
            "exit",
            anyhow::anyhow!("exit provider returned an invalid stream response"),
        )
    })?;
    validate_browser_stream_receipt(receipt).map_err(|err| ("exit", err))
}

pub(in crate::api::gateway) async fn browser_attach_runtime_stream_path(
    data_dir: &FsPath,
    mut receipt: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if receipt
        .get("byte_transport")
        .and_then(|value| value.as_str())
        != Some("adapter_ipc")
    {
        return Ok(receipt);
    }
    let stream_id = receipt
        .get("stream_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("adapter_ipc stream session missing stream_id"))?;
    let runtime_stream_path = browser_runtime_stream_socket_path(data_dir, stream_id)?;
    let relay = browser_stream_relay(&receipt)?;
    spawn_browser_runtime_stream_listener(&runtime_stream_path, relay).await?;
    let adapter_ipc = receipt
        .get_mut("adapter_ipc")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("adapter_ipc stream session missing descriptor"))?;
    adapter_ipc.insert(
        "runtime_stream_path".to_string(),
        serde_json::json!(runtime_stream_path.to_string_lossy().to_string()),
    );
    Ok(receipt)
}

#[derive(Debug, Clone)]
struct BrowserExitRelay {
    path: PathBuf,
    open_request: Vec<u8>,
}

fn browser_stream_relay(receipt: &serde_json::Value) -> anyhow::Result<Option<BrowserExitRelay>> {
    let Some(relay_ipc) = receipt.get("relay_ipc") else {
        return Ok(None);
    };
    if relay_ipc.is_null() {
        return Ok(None);
    }
    let relay_ipc = relay_ipc
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("relay_ipc descriptor must be an object"))?;
    if relay_ipc.get("schema").and_then(|value| value.as_str()) != Some("elastos.exit.relay-ipc/v1")
    {
        anyhow::bail!("relay_ipc descriptor must use elastos.exit.relay-ipc/v1");
    }
    if relay_ipc.get("kind").and_then(|value| value.as_str()) != Some("unix_socket") {
        anyhow::bail!("relay_ipc descriptor must use unix_socket kind");
    }
    let path = relay_ipc
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay_ipc descriptor missing path"))?;
    if path.is_empty() || !path.starts_with('/') {
        anyhow::bail!("relay_ipc path must be absolute");
    }
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        anyhow::bail!("relay_ipc path must not contain whitespace or NUL");
    }
    let stream_id = receipt
        .get("stream_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay_ipc stream session missing stream_id"))?;
    let target = receipt
        .get("target")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay_ipc stream session missing target"))?;
    let open_request = serde_json::json!({
        "schema": "elastos.exit.relay-open/v1",
        "stream_id": stream_id,
        "target": target,
        "scheme": receipt.get("scheme").cloned().unwrap_or(serde_json::Value::Null),
        "host": receipt.get("host").cloned().unwrap_or(serde_json::Value::Null),
        "principal_id": receipt.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
        "reason": receipt.get("reason").cloned().unwrap_or(serde_json::Value::Null),
    });
    let mut open_request = serde_json::to_vec(&open_request)?;
    open_request.push(b'\n');
    Ok(Some(BrowserExitRelay {
        path: PathBuf::from(path),
        open_request,
    }))
}

#[cfg(unix)]
async fn spawn_browser_runtime_stream_listener(
    path: &FsPath,
    relay: Option<BrowserExitRelay>,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() {
            std::fs::remove_file(path)?;
        } else {
            anyhow::bail!("Runtime browser stream path exists and is not a Unix socket");
        }
    }
    let listener = UnixListener::bind(path)?;
    let cleanup_path = path.to_path_buf();
    tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(30), listener.accept()).await {
            Ok(Ok((mut stream, _addr))) => {
                if let Some(relay) = relay {
                    match UnixStream::connect(&relay.path).await {
                        Ok(mut relay_stream) => {
                            if let Err(err) = relay_stream.write_all(&relay.open_request).await {
                                tracing::warn!(
                                    path = %cleanup_path.display(),
                                    relay = %relay.path.display(),
                                    error = %err,
                                    "browser runtime stream relay handshake failed"
                                );
                                let _ = std::fs::remove_file(cleanup_path);
                                return;
                            }
                            match copy_bidirectional(&mut stream, &mut relay_stream).await {
                                Ok((to_relay, to_engine)) => {
                                    tracing::info!(
                                        path = %cleanup_path.display(),
                                        relay = %relay.path.display(),
                                        to_relay,
                                        to_engine,
                                        "browser runtime stream relay closed"
                                    );
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        path = %cleanup_path.display(),
                                        relay = %relay.path.display(),
                                        error = %err,
                                        "browser runtime stream relay failed"
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                path = %cleanup_path.display(),
                                relay = %relay.path.display(),
                                error = %err,
                                "browser runtime stream could not connect to exit relay"
                            );
                        }
                    }
                } else {
                    tracing::info!(
                        path = %cleanup_path.display(),
                        "browser runtime stream accepted and closed fail-closed"
                    );
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    path = %cleanup_path.display(),
                    error = %err,
                    "browser runtime stream listener failed"
                );
            }
            Err(_) => {
                tracing::debug!(
                    path = %cleanup_path.display(),
                    "browser runtime stream expired without an engine bridge attach"
                );
            }
        }
        let _ = std::fs::remove_file(cleanup_path);
    });
    Ok(())
}

#[cfg(not(unix))]
async fn spawn_browser_runtime_stream_listener(
    _path: &FsPath,
    _relay: Option<BrowserExitRelay>,
) -> anyhow::Result<()> {
    anyhow::bail!("Browser runtime stream sockets require a Unix host adapter");
}

pub(in crate::api::gateway) fn browser_runtime_stream_socket_path(
    _data_dir: &FsPath,
    stream_id: &str,
) -> anyhow::Result<PathBuf> {
    if !stream_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
    {
        anyhow::bail!("stream_id must be a safe identifier");
    }
    let digest = Sha256::digest(stream_id.as_bytes());
    let socket_name = format!("{}.sock", hex::encode(&digest[..16]));
    // Unix socket paths have a small platform limit (`sun_path` is 104 bytes on macOS,
    // 108 on Linux). Keep Browser stream sockets in a short runtime temp directory and
    // expose only the opaque descriptor to the internal Browser Engine Adapter. macOS's
    // `$TMPDIR` (`/var/folders/.../T/`) is long enough that the default temp base can
    // overflow `sun_path` (bind → "AF_UNIX path too long"), so fall back to a short base
    // (`/tmp`) when the computed path would not fit. Conservative bound; macOS hard limit is 104.
    const SUN_PATH_SAFE_MAX: usize = 100;
    // SECURITY: on Linux `temp_dir()` is the SHARED `/tmp`, and the overflow fallback is ALWAYS the
    // world-writable `/tmp`. A fixed directory name there could be pre-created and owned by any local
    // user, who could then unlink our deterministically-named socket and re-bind their own to MITM or
    // DoS the decrypted media stream. Scope the base directory to our euid, and (in
    // `ensure_private_stream_dir`) create it 0700-owned-by-us and REFUSE any pre-existing directory we
    // do not own or that is group/other-writable.
    let dir_name = stream_dir_name();
    let mut stream_dir = std::env::temp_dir().join(&dir_name);
    if stream_dir.join(&socket_name).as_os_str().len() > SUN_PATH_SAFE_MAX {
        stream_dir = PathBuf::from("/tmp").join(&dir_name);
    }
    ensure_private_stream_dir(&stream_dir)?;
    Ok(stream_dir.join(socket_name))
}

/// The base directory name for Browser stream sockets. On unix it is scoped to the euid so the
/// (shared `/tmp`) base is not a fixed name any local user could pre-create and own.
#[cfg(unix)]
fn stream_dir_name() -> String {
    let euid = unsafe { libc::geteuid() };
    format!("{BROWSER_RUNTIME_STREAM_TMP_DIR}-{euid}")
}
#[cfg(not(unix))]
fn stream_dir_name() -> String {
    BROWSER_RUNTIME_STREAM_TMP_DIR.to_string()
}

/// Create — or validate the reuse of — a PRIVATE per-user directory for Browser stream sockets.
/// Because the fallback base is the world-writable `/tmp`, this is the gate that stops a local
/// attacker from squatting the directory and hijacking the (deterministically-named) socket: the
/// directory is created `0700` and, if it already exists, must be a real directory (not a planted
/// symlink), owned by this process's euid, and NOT group/other-writable — otherwise we fail closed
/// rather than bind a socket carrying decrypted media into a directory someone else controls.
#[cfg(unix)]
fn ensure_private_stream_dir(dir: &FsPath) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            // symlink_metadata: do NOT follow a symlink an attacker may have planted in its place.
            let meta = std::fs::symlink_metadata(dir)
                .map_err(|e| anyhow::anyhow!("stat browser stream dir {}: {e}", dir.display()))?;
            if !meta.file_type().is_dir() {
                anyhow::bail!(
                    "browser stream dir {} is not a directory (possible squatting) — refusing",
                    dir.display()
                );
            }
            let euid = unsafe { libc::geteuid() };
            if meta.uid() != euid {
                anyhow::bail!(
                    "browser stream dir {} is owned by uid {}, not this process ({}) — refusing to reuse a foreign directory",
                    dir.display(),
                    meta.uid(),
                    euid
                );
            }
            if meta.mode() & 0o022 != 0 {
                anyhow::bail!(
                    "browser stream dir {} is group/other-writable (mode {:o}) — refusing an unsafe directory",
                    dir.display(),
                    meta.mode() & 0o777
                );
            }
            Ok(())
        }
        Err(err) => Err(anyhow::anyhow!(
            "create browser stream dir {}: {err}",
            dir.display()
        )),
    }
}
#[cfg(not(unix))]
fn ensure_private_stream_dir(dir: &FsPath) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("create browser stream dir {}: {e}", dir.display()))
}

pub(in crate::api::gateway) fn validate_browser_stream_receipt(
    receipt: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if receipt.get("schema").and_then(|value| value.as_str())
        != Some("elastos.exit.stream-session/v1")
    {
        anyhow::bail!("stream provider did not return an elastos.exit.stream-session/v1 receipt");
    }
    Ok(receipt)
}

pub(in crate::api::gateway) fn browser_visible_stream_session(
    receipt: &serde_json::Value,
) -> serde_json::Value {
    let mut visible = receipt.clone();
    if let Some(object) = visible.as_object_mut() {
        object.remove("adapter_ipc");
        object.remove("relay_ipc");
    }
    visible
}

pub(in crate::api::gateway) fn browser_engine_stream_session(
    receipt: &serde_json::Value,
) -> serde_json::Value {
    let mut session = serde_json::json!({
        "schema": receipt.get("schema").cloned().unwrap_or(serde_json::Value::Null),
        "stream_id": receipt.get("stream_id").cloned().unwrap_or(serde_json::Value::Null),
        "target": receipt.get("target").cloned().unwrap_or(serde_json::Value::Null),
        "byte_transport": receipt.get("byte_transport").cloned().unwrap_or(serde_json::Value::Null),
        "adapter_ipc": receipt.get("adapter_ipc").cloned().unwrap_or(serde_json::Value::Null),
    });
    if let Some(relay_ipc) = receipt.get("relay_ipc").filter(|value| !value.is_null()) {
        if let Some(object) = session.as_object_mut() {
            object.insert("relay_ipc".to_string(), relay_ipc.clone());
        }
    }
    session
}

#[cfg(all(test, unix))]
mod private_dir_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn unique_tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "elastos-priv-dir-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// The happy path: the directory is created private (not group/other-accessible) and reusing
    /// our own directory on a subsequent call is idempotent.
    #[test]
    fn ensure_private_stream_dir_creates_0700_and_reuses_our_own() {
        let dir = unique_tmp("ok");
        let _ = std::fs::remove_dir_all(&dir);
        ensure_private_stream_dir(&dir).expect("create private dir");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "a freshly created stream dir must not be group/other-accessible, got {mode:o}"
        );
        // Reusing the same (still ours, still 0700) directory succeeds.
        ensure_private_stream_dir(&dir).expect("reuse our own private dir");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression (security): a pre-existing directory that is group/other-writable — the shape a
    /// local attacker would create to squat the shared `/tmp` base and hijack the deterministically
    /// named socket — must be REFUSED, not silently reused.
    #[test]
    fn ensure_private_stream_dir_refuses_a_world_writable_dir() {
        let dir = unique_tmp("hostile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = ensure_private_stream_dir(&dir)
            .expect_err("a world-writable stream dir must be refused");
        assert!(
            format!("{err:#}").contains("writable"),
            "expected a group/other-writable refusal, got: {err:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
