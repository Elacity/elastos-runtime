//! Carrier bridge for capsule stdio ↔ provider dispatch.
//!
//! Two bridge modes:
//!
//! 1. **MicroVM bridge**: Reads JSON-line requests from a Unix socket
//!    (connected to crosvm `virtio-console`), dispatches to providers, writes responses back.
//!    Guest uses `elastos-guest::RuntimeClient` with `ELASTOS_CARRIER_PATH=/dev/hvc0`.
//!
//! 2. **WASM bridge**: Reads JSON-line requests from an OS pipe (the capsule's stdout),
//!    dispatches to providers, writes responses to another pipe (the capsule's stdin).
//!    Guest uses `elastos-guest::RuntimeClient` with `CarrierChannel::Stdio`.
//!
//! Wire format: newline-delimited JSON matching `RequestEnvelope` / `ResponseEnvelope`
//! from `elastos-guest::runtime`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::local_http::LoopbackHttpBaseUrl;
use crate::provider_resource::{build_capability_resource, required_action_for};
use anyhow::{Context, Result};
use elastos_common::localhost::{
    is_supported_resource_scheme, is_system_only_backend_resource, rooted_localhost_fs_path,
    rooted_localhost_uri,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use elastos_compute::providers::BridgePipes;
use elastos_runtime::capability::CapabilityManager;
use elastos_runtime::provider::ProviderRegistry;

const CAPABILITY_APPROVAL_POLL_MS: u64 = 100;
const CAPABILITY_APPROVAL_MAX_POLLS: usize = 300;

/// Resources needed by the bridge to handle requests.
#[derive(Clone)]
pub struct BridgeContext {
    pub provider_registry: Arc<ProviderRegistry>,
    pub capability_manager: Arc<CapabilityManager>,
    pub pending_store: Arc<elastos_runtime::capability::pending::PendingRequestStore>,
    /// Capsule identity for token minting (session ID or capsule name)
    pub capsule_id: String,
    /// Runtime principal used to resolve capsule-facing `Users/self` aliases.
    pub principal_id: Option<String>,
    /// Runtime data directory used by protected principal-root storage helpers.
    pub data_dir: Option<PathBuf>,
}

/// Spawn a Carrier bridge handler for a microVM capsule.
///
/// Listens on a Unix socket that crosvm serial port 2 connects to.
/// Must be called BEFORE starting the VM so the socket exists when crosvm launches.
/// Reads `RequestEnvelope` JSON lines, dispatches to providers,
/// writes `ResponseEnvelope` JSON lines back.
pub async fn spawn_carrier_bridge(
    socket_path: &Path,
    _provider_registry: Arc<ProviderRegistry>,
    _session_token: String,
    bridge_ctx: Option<BridgeContext>,
) -> Result<()> {
    // Remove stale socket and create a listener BEFORE crosvm starts.
    // crosvm --serial type=unix-stream connects to this socket on launch.
    let _ = tokio::fs::remove_file(socket_path).await;
    let listener = tokio::net::UnixListener::bind(socket_path)
        .context("Failed to bind microVM Carrier bridge socket")?;

    let socket_display = socket_path.display().to_string();
    let ctx = bridge_ctx;

    // Accept one bidirectional connection in background — crosvm connects when
    // the VM boots. The supported contract is a single `unix-stream` socket
    // with `input-unix-stream` enabled on the crosvm side.
    tokio::spawn(async move {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Carrier bridge accept failed: {}", e);
                return;
            }
        };
        tracing::info!(
            "Carrier microVM bridge: bidirectional connection accepted for {}",
            socket_display
        );
        let (reader, mut writer) = stream.into_split();
        const MAX_LINE_BYTES: usize = 1_048_576; // 1 MB — prevent OOM from malicious guest
        let mut reader = BufReader::new(reader);

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF — guest shut down
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("Carrier bridge read error: {}", e);
                    break;
                }
            }

            if line.len() > MAX_LINE_BYTES {
                tracing::warn!(
                    "Carrier bridge: oversized line ({} bytes), dropping",
                    line.len()
                );
                let error = serde_json::json!({
                    "id": 0,
                    "type": "error",
                    "error": "request_too_large"
                });
                let _ = writer.write_all(error.to_string().as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                let _ = writer.flush().await;
                continue;
            }

            tracing::debug!("[serial-bridge] → {}", line.trim());
            let response = match handle_request(&line, &ctx).await {
                Ok(resp) => {
                    tracing::debug!("[serial-bridge] ← {}", resp);
                    resp
                }
                Err(e) => {
                    tracing::warn!("[serial-bridge] error: {}", e);
                    serde_json::json!({
                        "id": 0,
                        "response": {"type": "error", "code": "bridge_error", "message": e.to_string()}
                    })
                }
            };

            let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
            bytes.push(b'\n');
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
        tracing::info!("Carrier bridge closed for {}", socket_display);
    });

    Ok(())
}

/// Spawn a Carrier bridge for a WASM capsule.
///
/// Reads SDK requests from the capsule's stdout pipe, dispatches to providers,
/// writes responses to the capsule's stdin pipe. Runs in a dedicated OS thread
/// since the pipe I/O is blocking (the WASM capsule runs in `spawn_blocking`).
///
/// The bridge exits when the capsule closes its stdout (EOF on the pipe).
pub fn spawn_wasm_carrier_bridge(pipes: BridgePipes, ctx: BridgeContext) {
    let tokio_handle = tokio::runtime::Handle::current();

    if let Err(e) = std::thread::Builder::new()
        .name("wasm-carrier-bridge".into())
        .spawn(move || {
            use std::io::{BufRead, Write};

            let reader = std::io::BufReader::new(pipes.capsule_stdout);
            let mut writer = pipes.capsule_stdin;
            let ctx = Some(ctx);

            const MAX_LINE_BYTES: usize = 1_048_576; // 1 MB

            for line_result in reader.lines() {
                let line = match line_result {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::debug!("WASM bridge read error: {}", e);
                        break;
                    }
                };

                if line.trim().is_empty() {
                    continue;
                }

                if line.len() > MAX_LINE_BYTES {
                    tracing::warn!("WASM bridge: oversized line ({} bytes), dropping", line.len());
                    let error = serde_json::json!({"id":0,"type":"error","error":"request_too_large"});
                    let _ = writeln!(writer, "{}", error);
                    let _ = writer.flush();
                    continue;
                }

                let response = tokio_handle.block_on(async {
                    let result = handle_request(&line, &ctx).await;
                    let resp = match &result {
                        Ok(resp) => {
                            tracing::info!("[wasm-bridge] → {}", line.trim());
                            tracing::info!("[wasm-bridge] ← {}", resp);
                            resp.clone()
                        }
                        Err(e) => {
                            tracing::warn!("[wasm-bridge] error: {}", e);
                            serde_json::json!({
                                "id": 0,
                                "response": {"type": "error", "code": "bridge_error", "message": e.to_string()}
                            })
                        }
                    };
                    resp
                });

                let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
                bytes.push(b'\n');
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
            tracing::info!("WASM Carrier bridge closed");
        })
    {
        tracing::error!("Failed to spawn WASM bridge thread: {}", e);
    }
}

/// Spawn a Carrier bridge for a WASM capsule that proxies requests to a
/// running runtime API. The capsule still talks only over the fd bridge; the
/// host-side bridge performs the HTTP calls.
pub fn spawn_wasm_api_bridge(pipes: BridgePipes, api_url: String, client_token: String) {
    let tokio_handle = tokio::runtime::Handle::current();
    let principal_id = pipes.principal_id.clone();

    if let Err(e) = std::thread::Builder::new()
        .name("wasm-api-bridge".into())
        .spawn(move || {
            use std::io::{BufRead, Write};

            let reader = std::io::BufReader::new(pipes.capsule_stdout);
            let mut writer = pipes.capsule_stdin;

            for line_result in reader.lines() {
                let line = match line_result {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::debug!("WASM API bridge read error: {}", e);
                        break;
                    }
                };

                if line.trim().is_empty() {
                    continue;
                }

                let response = tokio_handle.block_on(async {
                    match handle_remote_request(&line, &api_url, &client_token, principal_id.as_deref()).await {
                        Ok(resp) => {
                            tracing::debug!("[wasm-api-bridge] → {}", line.trim());
                            tracing::debug!("[wasm-api-bridge] ← {}", resp);
                            resp
                        }
                        Err(e) => {
                            tracing::warn!("[wasm-api-bridge] error: {}", e);
                            serde_json::json!({
                                "id": 0,
                                "response": {"type": "error", "code": "bridge_error", "message": e.to_string()}
                            })
                        }
                    }
                });

                let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
                bytes.push(b'\n');
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
            tracing::debug!("WASM API bridge closed");
        })
    {
        tracing::error!("Failed to spawn WASM API bridge thread: {}", e);
    }
}

/// Parse an action string into a capability Action.
/// Returns None for unrecognized actions instead of silently defaulting.
fn parse_action(s: &str) -> Option<elastos_runtime::capability::Action> {
    use elastos_runtime::capability::Action;
    Some(match s.to_lowercase().as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" => Action::Execute,
        "message" => Action::Message,
        "delete" => Action::Delete,
        "admin" => Action::Admin,
        _ => return None,
    })
}

fn is_runtime_control_request(request_type: &str) -> bool {
    matches!(
        request_type,
        "list_capsules"
            | "launch_capsule"
            | "stop_capsule"
            | "grant_capability"
            | "revoke_capability"
            | "send_message"
            | "receive_messages"
            | "fetch_content"
            | "storage_read"
            | "storage_write"
            | "provider_call"
    )
}

struct CarrierInvokeDispatch {
    scheme: String,
    operation: String,
    request: serde_json::Value,
    resource: String,
}

fn carrier_invoke_dispatch(
    request: &serde_json::Value,
    principal_id: Option<&str>,
) -> Result<CarrierInvokeDispatch, String> {
    let uri = request["uri"]
        .as_str()
        .ok_or_else(|| "carrier_invoke missing uri".to_string())?;
    if !is_supported_resource_scheme(uri) {
        return Err("carrier URI must use elastos:// or localhost://".to_string());
    }
    if is_system_only_backend_resource(uri) {
        return Err("system backends are not app capabilities; use elastos://content".to_string());
    }
    let uri = scope_current_user_alias(uri, principal_id)?;

    let operation = request["operation"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "carrier_invoke missing operation".to_string())?
        .to_string();

    let scheme = provider_scheme_for_carrier_uri(&uri)?;
    let mut body = request
        .get("body")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if scheme == "localhost" {
        let path = match body.get("path").and_then(|value| value.as_str()) {
            Some(path) => scope_current_user_alias(path, principal_id)?,
            None => uri.to_string(),
        };
        body["path"] = serde_json::Value::String(path);
    }
    if scheme == "chain" && body.get("network").is_none() {
        if let Some(network) = uri
            .strip_prefix("elastos://chain/")
            .and_then(|rest| rest.split('/').next())
            .filter(|network| !network.is_empty() && *network != "meta")
        {
            body["network"] = serde_json::Value::String(network.to_string());
        }
    }
    if scheme == "wallet" && operation == "request_signature" {
        if let Some((chain_namespace, intent)) = wallet_signature_parts_from_uri(&uri) {
            if body.get("chain_namespace").is_none() {
                body["chain_namespace"] = serde_json::Value::String(chain_namespace);
            }
            if body.get("intent").is_none() {
                body["intent"] = serde_json::Value::String(intent);
            }
        }
    }

    let resource = build_capability_resource(&scheme, &operation, &body)?;
    body["op"] = serde_json::Value::String(operation.clone());

    Ok(CarrierInvokeDispatch {
        scheme,
        operation,
        request: body,
        resource,
    })
}

fn protected_principal_root_carrier_response(
    bridge_ctx: &BridgeContext,
    operation: &str,
    request: &serde_json::Value,
) -> Option<serde_json::Value> {
    let rooted = principal_root_read_write_uri(operation, request)?;

    let Some(principal_id) = bridge_ctx.principal_id.as_deref() else {
        return Some(carrier_error_response(
            "principal_context_required",
            "localhost://Users requires a principal-scoped launch context",
        ));
    };
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    if rooted != localhost_root && !rooted.starts_with(&format!("{localhost_root}/")) {
        return Some(carrier_error_response(
            "principal_context_required",
            "localhost://Users roots must use Users/self or the active principal root",
        ));
    }
    let Some(data_dir) = bridge_ctx.data_dir.as_deref() else {
        return Some(carrier_error_response(
            "principal_context_required",
            "principal-root storage requires a local runtime data directory",
        ));
    };

    match crate::auth::load_principal_root_protection(data_dir, principal_id, &localhost_root) {
        Ok(Some(_)) => {}
        Ok(None) => return None,
        Err(err) => {
            return Some(carrier_error_response(
                "principal_root_protection_invalid",
                &err.to_string(),
            ));
        }
    }

    let Some(path) = rooted_localhost_fs_path(data_dir, &rooted) else {
        return Some(carrier_error_response(
            "invalid_localhost_path",
            "invalid principal-root object path",
        ));
    };

    match operation {
        "read" => {
            let bytes = match crate::auth::read_principal_root_object(
                data_dir,
                principal_id,
                &localhost_root,
                &rooted,
                &path,
            ) {
                Ok(bytes) => bytes,
                Err(err) => return Some(provider_error_result("read_failed", &err.to_string())),
            };
            let bytes = apply_read_window(
                bytes,
                request.get("offset").and_then(|value| value.as_u64()),
                request.get("length").and_then(|value| value.as_u64()),
            );
            Some(provider_ok_result(serde_json::json!({
                "content": bytes,
                "size": bytes.len(),
            })))
        }
        "write" => {
            let content = match request_content_bytes(request) {
                Ok(content) => content,
                Err(message) => return Some(carrier_error_response("invalid_content", &message)),
            };
            let append = request
                .get("append")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let bytes = if append && path.is_file() {
                match crate::auth::read_principal_root_object(
                    data_dir,
                    principal_id,
                    &localhost_root,
                    &rooted,
                    &path,
                ) {
                    Ok(mut existing) => {
                        existing.extend_from_slice(&content);
                        existing
                    }
                    Err(err) => {
                        return Some(provider_error_result("read_failed", &err.to_string()))
                    }
                }
            } else {
                content.clone()
            };
            match crate::auth::write_principal_root_object(
                data_dir,
                principal_id,
                &localhost_root,
                &rooted,
                &path,
                &bytes,
            ) {
                Ok(()) => Some(provider_ok_result(serde_json::json!({
                    "bytes_written": content.len(),
                }))),
                Err(err) => Some(provider_error_result("write_failed", &err.to_string())),
            }
        }
        _ => None,
    }
}

fn principal_root_read_write_uri(operation: &str, request: &serde_json::Value) -> Option<String> {
    if !matches!(operation, "read" | "write") {
        return None;
    }
    let object_uri = request
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let rooted = rooted_localhost_uri(object_uri)?;
    rooted.starts_with("localhost://Users/").then_some(rooted)
}

fn request_content_bytes(request: &serde_json::Value) -> Result<Vec<u8>, String> {
    let Some(value) = request.get("content") else {
        return Err("write request missing content".to_string());
    };
    if let Some(text) = value.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    serde_json::from_value::<Vec<u8>>(value.clone())
        .map_err(|err| format!("write content must be bytes or string: {err}"))
}

fn apply_read_window(bytes: Vec<u8>, offset: Option<u64>, length: Option<u64>) -> Vec<u8> {
    let start = offset.unwrap_or(0) as usize;
    if start >= bytes.len() {
        return Vec::new();
    }
    let end = match length {
        Some(length) => start.saturating_add(length as usize).min(bytes.len()),
        None => bytes.len(),
    };
    bytes[start..end].to_vec()
}

fn provider_ok_result(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "carrier_result",
        "result": {
            "status": "ok",
            "data": data,
        },
    })
}

fn provider_error_result(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "carrier_result",
        "result": {
            "status": "error",
            "code": code,
            "message": message,
        },
    })
}

fn carrier_error_response(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "error",
        "code": code,
        "message": message,
    })
}

fn scope_current_user_alias(
    uri_or_resource: &str,
    principal_id: Option<&str>,
) -> Result<String, String> {
    let Some(rooted) = rooted_localhost_uri(uri_or_resource) else {
        return Ok(uri_or_resource.to_string());
    };

    if is_unscoped_current_user_alias(&rooted) {
        let Some(principal_id) = principal_id else {
            return Err(
                "localhost://Users/self requires a principal-scoped launch context".to_string(),
            );
        };
        let principal_root = crate::auth::principal_localhost_root(principal_id);
        if rooted == "localhost://Users/self" {
            return Ok(principal_root);
        }
        let rest = rooted
            .strip_prefix("localhost://Users/self/")
            .ok_or_else(|| format!("Invalid current-user alias: {uri_or_resource}"))?;
        return Ok(format!("{principal_root}/{rest}"));
    }

    if rooted.starts_with("localhost://Users/") {
        let Some(principal_id) = principal_id else {
            return Err("localhost://Users requires a principal-scoped launch context".to_string());
        };
        let principal_root = crate::auth::principal_localhost_root(principal_id);
        if rooted == principal_root || rooted.starts_with(&format!("{principal_root}/")) {
            return Ok(rooted);
        }
        return Err(
            "localhost://Users roots must use Users/self or the active principal root".to_string(),
        );
    }

    Ok(rooted)
}

fn is_unscoped_current_user_alias(uri_or_resource: &str) -> bool {
    let Some(rooted) = rooted_localhost_uri(uri_or_resource) else {
        return false;
    };
    rooted == "localhost://Users/self" || rooted.starts_with("localhost://Users/self/")
}

fn provider_scheme_for_carrier_uri(uri: &str) -> Result<String, String> {
    if uri.starts_with("localhost://") {
        if rooted_localhost_uri(uri).is_none() {
            return Err(format!("Invalid rooted localhost URI: {}", uri));
        }
        return Ok("localhost".to_string());
    }

    let rest = uri
        .strip_prefix("elastos://")
        .ok_or_else(|| "carrier URI must use elastos:// or localhost://".to_string())?;
    let scheme = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "elastos URI missing provider".to_string())?;
    Ok(scheme.to_string())
}

fn wallet_signature_parts_from_uri(uri: &str) -> Option<(String, String)> {
    let mut segments = uri.strip_prefix("elastos://wallet/")?.split('/');
    let chain_namespace = segments.next()?.trim();
    let sign_segment = segments.next()?.trim();
    let intent = segments.next()?.trim();
    if chain_namespace.is_empty()
        || sign_segment != "sign"
        || intent.is_empty()
        || segments.next().is_some()
    {
        return None;
    }
    Some((chain_namespace.to_string(), intent.to_string()))
}

/// Handle a single request from the guest capsule.
async fn handle_request(line: &str, ctx: &Option<BridgeContext>) -> Result<serde_json::Value> {
    let envelope: serde_json::Value =
        serde_json::from_str(line.trim()).context("Invalid JSON from guest")?;

    let id = envelope["id"].as_u64().unwrap_or(0);
    let request = &envelope["request"];
    let request_type = request["type"].as_str().unwrap_or("");

    let response = match request_type {
        "carrier_invoke" => {
            let bridge_ctx = ctx
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no bridge context"))?;

            let token_b64 = request["token"].as_str().unwrap_or("");
            let dispatch =
                match carrier_invoke_dispatch(request, bridge_ctx.principal_id.as_deref()) {
                    Ok(dispatch) => dispatch,
                    Err(message) => {
                        return Ok(serde_json::json!({
                            "id": id,
                            "response": {
                                "type": "error",
                                "code": "invalid_carrier_invoke",
                                "message": message,
                            }
                        }));
                    }
                };

            // Validate capability token before dispatching to provider.
            // The guest SDK sends the token it received from request_capability.
            if !token_b64.is_empty() {
                use elastos_runtime::capability::token::{CapabilityToken, ResourceId};
                match CapabilityToken::from_base64(token_b64) {
                    Ok(token) => {
                        let resource_id = ResourceId::new(&dispatch.resource);
                        // PRE-AUDIT #3: enforce the action the OPERATION requires, not the token's
                        // own action. A Read-granted token invoking a write/delete op now fails
                        // closed here (WrongAction) instead of being waved through.
                        let required = required_action_for(&dispatch.operation);
                        if bridge_ctx
                            .capability_manager
                            .validate(&token, &bridge_ctx.capsule_id, required, &resource_id, None)
                            .await
                            .is_err()
                        {
                            return Ok(serde_json::json!({
                                "id": id,
                                "response": {
                                    "type": "error",
                                    "code": "capability_denied",
                                    "message": "Capability validation failed",
                                }
                            }));
                        }
                    }
                    Err(_) => {
                        return Ok(serde_json::json!({
                            "id": id,
                            "response": {
                                "type": "error",
                                "code": "invalid_token",
                                "message": "Invalid capability token",
                            }
                        }));
                    }
                }
            } else {
                // No token provided — reject the call.
                return Ok(serde_json::json!({
                    "id": id,
                    "response": {
                        "type": "error",
                        "code": "missing_token",
                        "message": "carrier_invoke requires a capability token",
                    }
                }));
            }

            if let Some(response) = protected_principal_root_carrier_response(
                bridge_ctx,
                &dispatch.operation,
                &dispatch.request,
            ) {
                return Ok(serde_json::json!({
                    "id": id,
                    "response": response,
                }));
            }

            match bridge_ctx
                .provider_registry
                .send_raw(&dispatch.scheme, &dispatch.request)
                .await
            {
                Ok(result) => serde_json::json!({
                    "type": "carrier_result",
                    "result": result,
                }),
                Err(e) => {
                    tracing::warn!(
                        "Bridge carrier_invoke failed for {}/{}: {}",
                        dispatch.scheme,
                        dispatch.operation,
                        e
                    );
                    serde_json::json!({
                        "type": "error",
                        "code": "provider_error",
                        "message": "Provider operation failed",
                    })
                }
            }
        }

        "request_capability" => {
            let resource = request["resource"].as_str().unwrap_or("");
            let action_str = request["action"].as_str().unwrap_or("execute");

            if let Some(ctx) = ctx {
                if !is_supported_resource_scheme(resource) {
                    return Ok(serde_json::json!({
                        "id": id,
                        "response": {
                            "type": "error",
                            "code": "unsupported_resource",
                            "message": "capability resources must use elastos:// or localhost://",
                        },
                    }));
                }
                if is_system_only_backend_resource(resource) {
                    return Ok(serde_json::json!({
                        "id": id,
                        "response": {
                            "type": "error",
                            "code": "system_backend_denied",
                            "message": "system backends are not app capabilities; use elastos://content",
                        },
                    }));
                }
                let scoped_resource =
                    match scope_current_user_alias(resource, ctx.principal_id.as_deref()) {
                        Ok(resource) => resource,
                        Err(message) => {
                            return Ok(serde_json::json!({
                                "id": id,
                                "response": {
                                    "type": "error",
                                    "code": "principal_context_required",
                                    "message": message,
                                },
                            }));
                        }
                    };
                let resource = scoped_resource.as_str();
                let action = match parse_action(action_str) {
                    Some(a) => a,
                    None => {
                        return Ok(serde_json::json!({
                            "id": id,
                            "response": {
                                "type": "error",
                                "code": "invalid_action",
                                "message": format!("Unknown action: {}", action_str),
                            }
                        }));
                    }
                };
                let resource_id = elastos_runtime::capability::ResourceId::new(resource);

                // Create a pending request — the shell decides whether to grant.
                let pending = ctx
                    .pending_store
                    .create_request(
                        elastos_runtime::session::SessionId(ctx.capsule_id.clone()),
                        resource_id.clone(),
                        action,
                    )
                    .await;
                let request_id = pending.id.to_string();

                if pending.is_denied() {
                    tracing::info!(
                        "bridge: denied {} {} for capsule '{}' (capacity)",
                        action,
                        resource,
                        ctx.capsule_id,
                    );
                    serde_json::json!({
                        "type": "error",
                        "code": "denied",
                        "message": "capability request denied (too many pending)",
                    })
                } else {
                    // Poll for the shell's decision (AutoGrantEngine or manual).
                    // The shell polls /api/capability/pending and grants/denies.
                    let mut granted_token = None;
                    for _ in 0..CAPABILITY_APPROVAL_MAX_POLLS {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            CAPABILITY_APPROVAL_POLL_MS,
                        ))
                        .await;
                        if let Some(req) = ctx.pending_store.get_request(&request_id).await {
                            match &req.status {
                                elastos_runtime::capability::pending::RequestStatus::Granted {
                                    token,
                                    ..
                                } => {
                                    granted_token = Some(token.clone());
                                    break;
                                }
                                elastos_runtime::capability::pending::RequestStatus::Denied {
                                    reason,
                                } => {
                                    tracing::info!(
                                        "bridge: denied {} {} for capsule '{}': {}",
                                        action,
                                        resource,
                                        ctx.capsule_id,
                                        reason,
                                    );
                                    return Ok(serde_json::json!({
                                        "id": id,
                                        "response": {
                                            "type": "error",
                                            "code": "denied",
                                            "message": reason,
                                        },
                                    }));
                                }
                                elastos_runtime::capability::pending::RequestStatus::Expired => {
                                    return Ok(serde_json::json!({
                                        "id": id,
                                        "response": {
                                            "type": "error",
                                            "code": "expired",
                                            "message": "capability request expired",
                                        },
                                    }));
                                }
                                _ => {} // still pending
                            }
                        }
                    }

                    if let Some(token) = granted_token {
                        let token_b64 = encode_bridge_capability_token(&token);
                        tracing::info!(
                            "bridge: shell granted {} {} to capsule '{}'",
                            action,
                            resource,
                            ctx.capsule_id,
                        );
                        serde_json::json!({
                            "type": "capability_token",
                            "token": token_b64,
                        })
                    } else {
                        tracing::warn!(
                            "bridge: capability request timed out {} {} for capsule '{}'",
                            action,
                            resource,
                            ctx.capsule_id,
                        );
                        serde_json::json!({
                            "type": "error",
                            "code": "timeout",
                            "message": "capability request not approved within 30s",
                        })
                    }
                }
            } else {
                // Infrastructure trust domain: this capsule was launched without
                // a capability context (e.g. gateway service-plane capsules).
                // Capability requests are denied — infrastructure capsules should
                // not need user-facing capabilities.
                tracing::warn!(
                    "bridge: infrastructure capsule requested capability {} {} (denied)",
                    resource,
                    action_str,
                );
                serde_json::json!({
                    "type": "error",
                    "code": "infrastructure_capsule",
                    "message": "infrastructure capsules do not participate in user capability approval",
                })
            }
        }

        "ping" => serde_json::json!({"type": "pong"}),

        "get_runtime_info" => serde_json::json!({
            "type": "runtime_info",
            "version": env!("CARGO_PKG_VERSION"),
            "capsule_count": 0,
        }),

        request_type if is_runtime_control_request(request_type) => serde_json::json!({
            "type": "error",
            "code": "not_capsule_kernel_abi",
            "message": format!("{} is not exposed through the capsule kernel ABI", request_type),
        }),

        _ => serde_json::json!({
            "type": "error",
            "code": "unknown_request",
            "message": format!("Unknown request type: {}", request_type),
        }),
    };

    Ok(serde_json::json!({
        "id": id,
        "response": response,
    }))
}

fn encode_bridge_capability_token(
    token: &elastos_runtime::capability::token::CapabilityToken,
) -> String {
    token.to_base64().unwrap_or_default()
}

async fn handle_remote_request(
    line: &str,
    api_url: &str,
    client_token: &str,
    principal_id: Option<&str>,
) -> Result<serde_json::Value> {
    let api_base = LoopbackHttpBaseUrl::parse(api_url).map_err(|e| {
        anyhow::anyhow!(
            "attached WASM bridge requires a local runtime API URL; rejecting remote transport: {}",
            e
        )
    })?;

    let envelope: serde_json::Value =
        serde_json::from_str(line.trim()).context("Invalid JSON from guest")?;

    let id = envelope["id"].as_u64().unwrap_or(0);
    let request = &envelope["request"];
    let request_type = request["type"].as_str().unwrap_or("");
    let client = reqwest::Client::new();

    let response = match request_type {
        "carrier_invoke" => {
            let dispatch = match carrier_invoke_dispatch(request, principal_id) {
                Ok(dispatch) => dispatch,
                Err(message) => {
                    return Ok(serde_json::json!({
                        "id": id,
                        "response": {
                            "type": "error",
                            "code": "invalid_carrier_invoke",
                            "message": message,
                        }
                    }));
                }
            };
            let cap_token = request["token"].as_str().unwrap_or("");

            if principal_root_read_write_uri(&dispatch.operation, &dispatch.request).is_some() {
                return Ok(serde_json::json!({
                    "id": id,
                    "response": carrier_error_response(
                        "principal_context_required",
                        "principal-root storage requires an in-runtime protected storage bridge",
                    ),
                }));
            }

            tracing::debug!(
                "[wasm-api-bridge] carrier_invoke {}/{} token={} body={}",
                dispatch.scheme,
                dispatch.operation,
                !cap_token.is_empty(),
                &dispatch
                    .request
                    .to_string()
                    .chars()
                    .take(150)
                    .collect::<String>()
            );

            let mut req = client
                .post(api_base.join(&format!(
                    "/api/provider/{}/{}",
                    dispatch.scheme, dispatch.operation
                ))?)
                .header("Authorization", format!("Bearer {}", client_token))
                .json(&dispatch.request);

            if !cap_token.is_empty() {
                req = req.header("X-Capability-Token", cap_token);
            }

            let resp = req.send().await?;
            let status = resp.status();
            let body: serde_json::Value = resp.json().await?;
            tracing::debug!(
                "[wasm-api-bridge] {}/{} → {} {}",
                dispatch.scheme,
                dispatch.operation,
                status,
                &body.to_string().chars().take(200).collect::<String>()
            );
            serde_json::json!({
                "type": "carrier_result",
                "result": body,
            })
        }
        "request_capability" => {
            let resource = request["resource"].as_str().unwrap_or("");
            let action = request["action"].as_str().unwrap_or("execute");

            let scoped_resource = match scope_current_user_alias(resource, principal_id) {
                Ok(resource) => resource,
                Err(message) => {
                    return Ok(serde_json::json!({
                        "id": id,
                        "response": {
                            "type": "error",
                            "code": "principal_context_required",
                            "message": message,
                        }
                    }));
                }
            };
            let resource = scoped_resource.as_str();

            let resp = client
                .post(api_base.join("/api/capability/request")?)
                .header("Authorization", format!("Bearer {}", client_token))
                .json(&serde_json::json!({
                    "resource": resource,
                    "action": action,
                }))
                .send()
                .await?;
            let body: serde_json::Value = resp.json().await?;

            if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
                serde_json::json!({
                    "type": "capability_token",
                    "token": token,
                })
            } else {
                match body.get("status").and_then(|s| s.as_str()) {
                    Some("denied") | Some("auto_denied") | Some("expired") => {
                        return Ok(serde_json::json!({
                            "id": id,
                            "response": {
                                "type": "error",
                                "code": body.get("status").and_then(|s| s.as_str()).unwrap_or("denied"),
                                "message": body.get("reason").and_then(|r| r.as_str()).unwrap_or("capability request denied"),
                            }
                        }));
                    }
                    _ => {}
                }
                let request_id = body
                    .get("request_id")
                    .and_then(|r| r.as_str())
                    .ok_or_else(|| anyhow::anyhow!("capability response missing request_id"))?;

                let mut token = None;
                for _ in 0..CAPABILITY_APPROVAL_MAX_POLLS {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        CAPABILITY_APPROVAL_POLL_MS,
                    ))
                    .await;
                    let resp = client
                        .get(api_base.join(&format!("/api/capability/request/{}", request_id))?)
                        .header("Authorization", format!("Bearer {}", client_token))
                        .send()
                        .await?;
                    let status: serde_json::Value = resp.json().await?;
                    if let Some(granted) = status.get("token").and_then(|t| t.as_str()) {
                        token = Some(granted.to_string());
                        break;
                    }
                    match status.get("status").and_then(|s| s.as_str()) {
                        Some("denied") | Some("expired") => {
                            return Ok(serde_json::json!({
                                "id": id,
                                "response": {
                                    "type": "error",
                                    "code": status.get("status").and_then(|s| s.as_str()).unwrap_or("error"),
                                    "message": status.get("reason").and_then(|r| r.as_str()).unwrap_or("capability request failed"),
                                }
                            }));
                        }
                        _ => {}
                    }
                }

                let token = token
                    .ok_or_else(|| anyhow::anyhow!("capability request still pending after 30s"))?;
                serde_json::json!({
                    "type": "capability_token",
                    "token": token,
                })
            }
        }
        "ping" => serde_json::json!({"type": "pong"}),
        "get_runtime_info" => serde_json::json!({
            "type": "runtime_info",
            "version": env!("CARGO_PKG_VERSION"),
            "capsule_count": 0,
        }),

        request_type if is_runtime_control_request(request_type) => serde_json::json!({
            "type": "error",
            "code": "not_capsule_kernel_abi",
            "message": format!("{} is not exposed through the capsule kernel ABI", request_type),
        }),

        _ => serde_json::json!({
            "type": "error",
            "code": "unknown_request",
            "message": format!("Unknown request type: {}", request_type),
        }),
    };

    Ok(serde_json::json!({
        "id": id,
        "response": response,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use elastos_runtime::{
        capability::token::{Action, CapabilityToken, ResourceId, TokenConstraints},
        primitives::time::SecureTimestamp,
    };
    use std::sync::Arc;

    fn bridge_context() -> BridgeContext {
        let audit_log = Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics = Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager = Arc::new(elastos_runtime::capability::CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics,
        ));

        BridgeContext {
            provider_registry: Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
            capability_manager,
            pending_store: Arc::new(
                elastos_runtime::capability::pending::PendingRequestStore::new(audit_log),
            ),
            capsule_id: "test-capsule".to_string(),
            principal_id: None,
            data_dir: None,
        }
    }

    fn bridge_token(ctx: &BridgeContext, resource: &str, action: Action) -> String {
        let token = ctx.capability_manager.grant(
            &ctx.capsule_id,
            ResourceId::new(resource),
            action,
            TokenConstraints::default(),
            None,
        );
        encode_bridge_capability_token(&token)
    }

    #[test]
    fn test_parse_action_known() {
        assert!(parse_action("read").is_some());
        assert!(parse_action("write").is_some());
        assert!(parse_action("execute").is_some());
        assert!(parse_action("message").is_some());
        assert!(parse_action("delete").is_some());
        assert!(parse_action("admin").is_some());
    }

    #[test]
    fn test_parse_action_unknown_rejected() {
        assert!(parse_action("INVALID").is_none());
        assert!(parse_action("").is_none());
        assert!(parse_action("drop_table").is_none());
    }

    #[test]
    fn test_parse_action_case_insensitive() {
        assert!(parse_action("READ").is_some());
        assert!(parse_action("Write").is_some());
        assert!(parse_action("EXECUTE").is_some());
    }

    #[test]
    fn test_runtime_control_request_classification() {
        assert!(is_runtime_control_request("launch_capsule"));
        assert!(is_runtime_control_request("storage_read"));
        assert!(is_runtime_control_request("provider_call"));
        assert!(!is_runtime_control_request("carrier_invoke"));
        assert!(!is_runtime_control_request("request_capability"));
    }

    // End-to-end over the Carrier bridge: a capsule's capability-gated
    // carrier_invoke("elastos://inspect/capsules") must validate the inspect
    // capability and reach the inspect provider; without a token it is rejected.
    #[tokio::test]
    async fn carrier_invoke_reaches_inspect_provider_with_capability() {
        use crate::inspect_provider::{CatalogInspectSource, InspectProvider, InspectSource};

        let tmp = tempfile::tempdir().unwrap();
        let capsule_dir = tmp.path().join("capsules").join("probe");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "probe",
                "role": "app", "type": "wasm", "entrypoint": "probe.wasm"
            }))
            .unwrap(),
        )
        .unwrap();

        let audit_log = Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics = Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager = Arc::new(elastos_runtime::capability::CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics,
        ));
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        let source: Arc<dyn InspectSource> = Arc::new(CatalogInspectSource::new(
            tmp.path().join("capsules"),
            Arc::downgrade(&registry),
        ));
        registry
            .register(Arc::new(InspectProvider::new(source)))
            .await;

        // System inspect capability granted to the calling capsule.
        let token = encode_bridge_capability_token(&capability_manager.grant(
            "test-capsule",
            ResourceId::new("elastos://inspect/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        ));

        let ctx = Some(BridgeContext {
            provider_registry: registry,
            capability_manager: capability_manager.clone(),
            pending_store: Arc::new(
                elastos_runtime::capability::pending::PendingRequestStore::new(audit_log),
            ),
            capsule_id: "test-capsule".to_string(),
            principal_id: None,
            data_dir: None,
        });

        // With a valid capability: reaches the provider.
        let line = serde_json::json!({
            "id": 1,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://inspect/capsules",
                "operation": "capsules",
                "token": token,
            }
        })
        .to_string();
        let resp = handle_request(&line, &ctx).await.unwrap();
        assert_eq!(resp["response"]["type"], "carrier_result");
        assert_eq!(resp["response"]["result"]["status"], "ok");
        assert!(
            resp["response"]["result"].to_string().contains("probe"),
            "carrier inspect did not reach the provider: {resp}"
        );

        // Without a token: rejected before dispatch.
        let line_no_token = serde_json::json!({
            "id": 2,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://inspect/capsules",
                "operation": "capsules",
            }
        })
        .to_string();
        let denied = handle_request(&line_no_token, &ctx).await.unwrap();
        assert_eq!(denied["response"]["code"], "missing_token");
    }

    // MERGE TRIPWIRE. For every inspect op the product provider serves, a token
    // minted at the *canonical* action (provider_resource::inspect_op_required_action)
    // must pass the carrier capability gate and reach the provider. Today our gate
    // validates token.action(), so this passes by construction — but when the DDRM
    // branch lands, the gate becomes validate(.., required_action_for(op), ..). If
    // that map omits an inspect op it fails closed to Action::Admin and a Read
    // token is denied → this test fails LOUDLY at merge, instead of the break
    // slipping through git's clean auto-merge. This converts the documented
    // reconciliation note into an enforced invariant.
    #[tokio::test]
    async fn carrier_inspect_ops_match_canonical_action_contract() {
        use crate::inspect_provider::{CatalogInspectSource, InspectProvider, InspectSource};
        use crate::provider_resource::inspect_op_required_action;

        let tmp = tempfile::tempdir().unwrap();
        let capsule_dir = tmp.path().join("capsules").join("probe");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "probe",
                "role": "app", "type": "wasm", "entrypoint": "probe.wasm"
            }))
            .unwrap(),
        )
        .unwrap();

        let audit_log = Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics = Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager = Arc::new(elastos_runtime::capability::CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics,
        ));
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        let source: Arc<dyn InspectSource> = Arc::new(CatalogInspectSource::new(
            tmp.path().join("capsules"),
            Arc::downgrade(&registry),
        ));
        registry
            .register(Arc::new(InspectProvider::new(source)))
            .await;

        let ctx = Some(BridgeContext {
            provider_registry: registry,
            capability_manager: capability_manager.clone(),
            pending_store: Arc::new(
                elastos_runtime::capability::pending::PendingRequestStore::new(audit_log),
            ),
            capsule_id: "test-capsule".to_string(),
            principal_id: None,
            data_dir: None,
        });

        // Read-side ops the product provider serves. (self/revoke live on the
        // embedded-handler contract, not this provider — not gate-tested here.)
        let cases = [
            ("capsules", serde_json::json!({})),
            ("capsule", serde_json::json!({ "id": "probe" })),
            (
                "plan",
                serde_json::json!({ "id": "probe", "operation": "x" }),
            ),
        ];

        for (op, mut payload) in cases {
            let action = inspect_op_required_action(op)
                .unwrap_or_else(|| panic!("no canonical action for inspect op {op}"));
            let token = encode_bridge_capability_token(&capability_manager.grant(
                "test-capsule",
                ResourceId::new(format!("elastos://inspect/{op}")),
                action,
                TokenConstraints::default(),
                None,
            ));
            let obj = payload.as_object_mut().unwrap();
            obj.insert("type".into(), serde_json::json!("carrier_invoke"));
            obj.insert(
                "uri".into(),
                serde_json::json!(format!("elastos://inspect/{op}")),
            );
            obj.insert("operation".into(), serde_json::json!(op));
            obj.insert("token".into(), serde_json::json!(token));
            let line = serde_json::json!({ "id": 1, "request": payload }).to_string();

            let resp = handle_request(&line, &ctx).await.unwrap();
            // A canonical-action token must clear the gate and reach the provider
            // (business outcome may be ok/not_found/invalid_request — all are
            // carrier_result; only a gate failure yields type "error").
            assert_eq!(
                resp["response"]["type"], "carrier_result",
                "inspect op {op} did not clear the capability gate at action {action:?}: {resp}"
            );
        }
    }

    #[test]
    fn carrier_invoke_dispatch_uses_uri_resource_contract() {
        let dispatch = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": "localhost://Local/SharedByLocalUsersAndBots/Home/a.md",
                "operation": "read",
                "body": {}
            }),
            None,
        )
        .expect("localhost carrier invoke should dispatch");

        assert_eq!(dispatch.scheme, "localhost");
        assert_eq!(dispatch.operation, "read");
        assert_eq!(
            dispatch.resource,
            "localhost://Local/SharedByLocalUsersAndBots/Home/a.md"
        );
        assert_eq!(
            dispatch
                .request
                .get("path")
                .and_then(|value| value.as_str()),
            Some("localhost://Local/SharedByLocalUsersAndBots/Home/a.md")
        );
    }

    #[test]
    fn carrier_invoke_dispatch_rejects_unscoped_current_user_alias() {
        let result = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/Documents/a.md",
                "operation": "read",
                "body": {}
            }),
            None,
        );
        assert!(
            result.is_err(),
            "capsule-kernel Users/self requires a principal context"
        );
        let error = result.err().unwrap();

        assert_eq!(
            error,
            "localhost://Users/self requires a principal-scoped launch context"
        );
    }

    #[test]
    fn carrier_invoke_dispatch_scopes_current_user_alias_with_principal() {
        let principal_id = "person:local:test-principal";
        let expected_root = crate::auth::principal_localhost_root(principal_id);
        let dispatch = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/Documents/a.md",
                "operation": "read",
                "body": {}
            }),
            Some(principal_id),
        )
        .expect("principal-scoped current-user alias should dispatch");

        let expected_path = format!("{expected_root}/Documents/a.md");
        assert_eq!(dispatch.resource, expected_path);
        assert_eq!(
            dispatch
                .request
                .get("path")
                .and_then(|value| value.as_str()),
            Some(expected_path.as_str())
        );
    }

    #[test]
    fn carrier_invoke_dispatch_allows_active_explicit_principal_root() {
        let principal_id = "person:local:test-principal";
        let principal_root = crate::auth::principal_localhost_root(principal_id);
        let path = format!("{principal_root}/Documents/a.md");
        let dispatch = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": path,
                "operation": "read",
                "body": {}
            }),
            Some(principal_id),
        )
        .expect("active explicit principal root should dispatch");

        assert_eq!(dispatch.resource, path);
    }

    #[test]
    fn carrier_invoke_dispatch_rejects_foreign_principal_root() {
        let active_principal_id = "person:local:active";
        let foreign_root = crate::auth::principal_localhost_root("person:local:foreign");
        let result = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": format!("{foreign_root}/Documents/a.md"),
                "operation": "read",
                "body": {}
            }),
            Some(active_principal_id),
        );

        assert_eq!(
            result.err().as_deref(),
            Some("localhost://Users roots must use Users/self or the active principal root")
        );
    }

    #[test]
    fn carrier_invoke_dispatch_derives_chain_network() {
        let dispatch = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": "elastos://chain/esc-mainnet/block_number",
                "operation": "block_number",
                "body": {}
            }),
            None,
        )
        .expect("chain carrier invoke should dispatch");

        assert_eq!(dispatch.scheme, "chain");
        assert_eq!(
            dispatch
                .request
                .get("network")
                .and_then(|value| value.as_str()),
            Some("esc-mainnet")
        );
        assert_eq!(
            dispatch.resource,
            "elastos://chain/esc-mainnet/block_number"
        );
    }

    #[test]
    fn carrier_invoke_dispatch_derives_wallet_chain_and_intent() {
        let dispatch = carrier_invoke_dispatch(
            &serde_json::json!({
                "type": "carrier_invoke",
                "uri": "elastos://wallet/eip155:20/sign/transaction_intent",
                "operation": "request_signature",
                "body": {
                    "capsule_id": "market",
                    "resource": "elastos://wallet/eip155:20/sign/transaction_intent",
                    "reason": "Approve transaction",
                    "payload": {"schema": "elastos.wallet.test/v1"}
                }
            }),
            None,
        )
        .expect("wallet carrier invoke should dispatch");

        assert_eq!(dispatch.scheme, "wallet");
        assert_eq!(dispatch.operation, "request_signature");
        assert_eq!(
            dispatch
                .request
                .get("chain_namespace")
                .and_then(|value| value.as_str()),
            Some("eip155:20")
        );
        assert_eq!(
            dispatch
                .request
                .get("intent")
                .and_then(|value| value.as_str()),
            Some("transaction_intent")
        );
        assert_eq!(
            dispatch.resource,
            "elastos://wallet/eip155:20/sign/transaction_intent"
        );
    }

    #[test]
    fn test_bridge_capability_token_encoding_matches_runtime_transport() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();

        let mut token = CapabilityToken::new(
            "test-capsule".to_string(),
            verifying_key.to_bytes(),
            ResourceId::new("elastos://peer/*"),
            Action::Execute,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );
        token.sign(&signing_key);

        let encoded = encode_bridge_capability_token(&token);
        assert!(!encoded.starts_with('{'));

        let decoded =
            CapabilityToken::from_base64(&encoded).expect("bridge token should decode as base64");
        assert_eq!(token.id(), decoded.id());
        assert_eq!(token.capsule(), decoded.capsule());
        assert_eq!(token.action(), decoded.action());
    }

    #[tokio::test]
    async fn handle_remote_request_rejects_non_loopback_api_url() {
        let err = handle_remote_request(
            r#"{"id":1,"request":{"type":"ping"}}"#,
            "https://example.com",
            "client-token",
            None,
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("attached WASM bridge requires a local runtime API URL"));
    }

    #[tokio::test]
    async fn handle_remote_request_rejects_raw_runtime_control_api() {
        let response = handle_remote_request(
            r#"{"id":8,"request":{"type":"launch_capsule","cid":"QmExample","config":{}}}"#,
            "http://127.0.0.1:12345",
            "client-token",
            None,
        )
        .await
        .expect("browser host adapter should reject runtime control before HTTP dispatch");

        assert_eq!(response["id"], 8);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "not_capsule_kernel_abi");
    }

    #[tokio::test]
    async fn handle_remote_request_denies_users_self_before_runtime_prompt() {
        let response = handle_remote_request(
            r#"{"id":12,"request":{"type":"request_capability","resource":"localhost://Users/self/Documents/*","action":"read"}}"#,
            "http://127.0.0.1:12345",
            "client-token",
            None,
        )
        .await
        .expect("attached WASM bridge should reject before runtime dispatch");

        assert_eq!(response["id"], 12);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "principal_context_required");
    }

    #[tokio::test]
    async fn handle_remote_request_rejects_users_root_storage_without_protected_bridge() {
        let principal_id = "person:local:active";
        let response = handle_remote_request(
            r#"{"id":13,"request":{"type":"carrier_invoke","uri":"localhost://Users/self/Documents/a.md","operation":"read","token":"tok","body":{}}}"#,
            "http://127.0.0.1:12345",
            "client-token",
            Some(principal_id),
        )
        .await
        .expect("attached bridge should reject before provider dispatch");

        assert_eq!(response["id"], 13);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "principal_context_required");
        assert!(response["response"]["message"]
            .as_str()
            .unwrap()
            .contains("protected storage bridge"));
    }

    #[tokio::test]
    async fn handle_request_rejects_raw_runtime_control_api() {
        let response = handle_request(
            r#"{"id":7,"request":{"type":"launch_capsule","cid":"QmExample","config":{}}}"#,
            &None,
        )
        .await
        .expect("bridge should produce a fail-closed response");

        assert_eq!(response["id"], 7);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "not_capsule_kernel_abi");
    }

    #[tokio::test]
    async fn handle_request_rejects_old_provider_call_shape() {
        let response = handle_request(
            r#"{"id":10,"request":{"type":"provider_call","scheme":"did","op":"get_did","body":{},"token":"tok"}}"#,
            &None,
        )
        .await
        .expect("bridge should reject old provider-call ABI");

        assert_eq!(response["id"], 10);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "not_capsule_kernel_abi");
    }

    #[tokio::test]
    async fn handle_request_denies_system_backend_capability_before_pending() {
        let ctx = bridge_context();
        let pending_store = ctx.pending_store.clone();
        let response = handle_request(
            r#"{"id":8,"request":{"type":"request_capability","resource":"elastos://ipfs-provider/add","action":"write"}}"#,
            &Some(ctx),
        )
        .await
        .expect("bridge should fail closed before creating a pending request");

        assert_eq!(response["id"], 8);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "system_backend_denied");
        assert!(
            pending_store.list_pending().await.is_empty(),
            "system backend denials must not create approval prompts"
        );
    }

    #[tokio::test]
    async fn handle_request_denies_unsupported_capability_scheme_before_pending() {
        let ctx = bridge_context();
        let pending_store = ctx.pending_store.clone();
        let response = handle_request(
            r#"{"id":9,"request":{"type":"request_capability","resource":"https://example.com/raw","action":"read"}}"#,
            &Some(ctx),
        )
        .await
        .expect("bridge should fail closed before creating a pending request");

        assert_eq!(response["id"], 9);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "unsupported_resource");
        assert!(
            pending_store.list_pending().await.is_empty(),
            "unsupported resource denials must not create approval prompts"
        );
    }

    #[tokio::test]
    async fn handle_request_denies_users_self_without_principal_before_pending() {
        let ctx = bridge_context();
        let pending_store = ctx.pending_store.clone();
        let response = handle_request(
            r#"{"id":11,"request":{"type":"request_capability","resource":"localhost://Users/self/Documents/*","action":"read"}}"#,
            &Some(ctx),
        )
        .await
        .expect("bridge should require principal context before creating a pending request");

        assert_eq!(response["id"], 11);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "principal_context_required");
        assert!(
            pending_store.list_pending().await.is_empty(),
            "principal-context denials must not create approval prompts"
        );
    }

    #[tokio::test]
    async fn handle_request_denies_foreign_user_root_before_pending() {
        let mut ctx = bridge_context();
        ctx.principal_id = Some("person:local:active".to_string());
        let pending_store = ctx.pending_store.clone();
        let foreign_root = crate::auth::principal_localhost_root("person:local:foreign");
        let response = handle_request(
            &format!(
                r#"{{"id":12,"request":{{"type":"request_capability","resource":"{foreign_root}/Documents/*","action":"read"}}}}"#
            ),
            &Some(ctx),
        )
        .await
        .expect("bridge should reject foreign principal roots before creating a pending request");

        assert_eq!(response["id"], 12);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "principal_context_required");
        assert_eq!(
            response["response"]["message"],
            "localhost://Users roots must use Users/self or the active principal root"
        );
        assert!(
            pending_store.list_pending().await.is_empty(),
            "foreign-root denials must not create approval prompts"
        );
    }

    #[tokio::test]
    async fn handle_request_uses_protected_principal_root_object_for_users_self_writes() {
        let temp = tempfile::tempdir().unwrap();
        let principal_id = "person:local:active";
        let protection =
            crate::auth::store_test_principal_root_protection(temp.path(), principal_id);
        let mut ctx = bridge_context();
        ctx.principal_id = Some(principal_id.to_string());
        ctx.data_dir = Some(temp.path().to_path_buf());

        let object_uri = format!(
            "{}/.AppData/LocalHost/Chat/state.json",
            protection.localhost_root
        );
        let write_token = bridge_token(&ctx, &object_uri, Action::Write);
        let write_line = serde_json::json!({
            "id": 21,
            "request": {
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/.AppData/LocalHost/Chat/state.json",
                "operation": "write",
                "token": write_token,
                "body": {
                    "content": b"secret-chat-state".to_vec(),
                    "append": false
                }
            }
        })
        .to_string();
        let ctx_opt = Some(ctx.clone());
        let write_response = handle_request(&write_line, &ctx_opt)
            .await
            .expect("protected write should produce a bridge response");

        assert_eq!(write_response["id"], 21);
        assert_eq!(write_response["response"]["type"], "carrier_result");
        assert_eq!(write_response["response"]["result"]["status"], "ok");

        let path = rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        let stored = std::fs::read_to_string(path).unwrap();
        assert!(stored.contains("elastos.principal-root.object/v1"));
        assert!(stored.contains(&protection.data_key_id));
        assert!(!stored.contains("secret-chat-state"));

        let read_token = bridge_token(&ctx, &object_uri, Action::Read);
        let read_line = serde_json::json!({
            "id": 22,
            "request": {
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/.AppData/LocalHost/Chat/state.json",
                "operation": "read",
                "token": read_token,
                "body": {}
            }
        })
        .to_string();
        let read_response = handle_request(&read_line, &ctx_opt)
            .await
            .expect("protected read should produce a bridge response");
        let content: Vec<u8> =
            serde_json::from_value(read_response["response"]["result"]["data"]["content"].clone())
                .unwrap();
        assert_eq!(content, b"secret-chat-state");
    }

    #[tokio::test]
    async fn carrier_invoke_denies_read_token_on_write_operation_preaudit3() {
        // PRE-AUDIT #3: localhost read/write/delete share ONE resource string, so before this fix a
        // token granted for `read` could drive a `write` — the bridge only checked the token against
        // its OWN action. Now the bridge enforces the action the OPERATION requires.
        let temp = tempfile::tempdir().unwrap();
        let principal_id = "person:local:active";
        let protection =
            crate::auth::store_test_principal_root_protection(temp.path(), principal_id);
        let mut ctx = bridge_context();
        ctx.principal_id = Some(principal_id.to_string());
        ctx.data_dir = Some(temp.path().to_path_buf());

        let object_uri = format!(
            "{}/.AppData/LocalHost/Chat/state.json",
            protection.localhost_root
        );
        // A token granted ONLY for read on this exact resource.
        let read_token = bridge_token(&ctx, &object_uri, Action::Read);
        // Attempt to WRITE with it.
        let write_with_read = serde_json::json!({
            "id": 31,
            "request": {
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/.AppData/LocalHost/Chat/state.json",
                "operation": "write",
                "token": read_token,
                "body": { "content": b"escalated-write".to_vec(), "append": false }
            }
        })
        .to_string();
        let ctx_opt = Some(ctx.clone());
        let denied = handle_request(&write_with_read, &ctx_opt)
            .await
            .expect("bridge should return a response");
        assert_eq!(denied["id"], 31);
        assert_eq!(denied["response"]["type"], "error");
        assert_eq!(
            denied["response"]["code"], "capability_denied",
            "a read-granted token must NOT authorize a write op: {denied}"
        );
        // And nothing was written.
        let path = rooted_localhost_fs_path(temp.path(), &object_uri).unwrap();
        assert!(
            !path.exists(),
            "the escalated write must not have touched disk"
        );

        // The matching action (a write-granted token) is still accepted.
        let write_token = bridge_token(&ctx, &object_uri, Action::Write);
        let write_ok = serde_json::json!({
            "id": 32,
            "request": {
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/.AppData/LocalHost/Chat/state.json",
                "operation": "write",
                "token": write_token,
                "body": { "content": b"authorized-write".to_vec(), "append": false }
            }
        })
        .to_string();
        let ok = handle_request(&write_ok, &ctx_opt)
            .await
            .expect("bridge should return a response");
        assert_eq!(
            ok["response"]["type"], "carrier_result",
            "matching action opens: {ok}"
        );
    }

    #[tokio::test]
    async fn handle_request_rejects_users_root_carrier_invoke_without_data_dir() {
        let principal_id = "person:local:active";
        let mut ctx = bridge_context();
        ctx.principal_id = Some(principal_id.to_string());
        let object_uri = format!(
            "{}/Documents/a.md",
            crate::auth::principal_localhost_root(principal_id)
        );
        let read_token = bridge_token(&ctx, &object_uri, Action::Read);
        let line = serde_json::json!({
            "id": 23,
            "request": {
                "type": "carrier_invoke",
                "uri": "localhost://Users/self/Documents/a.md",
                "operation": "read",
                "token": read_token,
                "body": {}
            }
        })
        .to_string();

        let response = handle_request(&line, &Some(ctx))
            .await
            .expect("missing data dir should produce a fail-closed response");

        assert_eq!(response["id"], 23);
        assert_eq!(response["response"]["type"], "error");
        assert_eq!(response["response"]["code"], "principal_context_required");
        assert!(response["response"]["message"]
            .as_str()
            .unwrap()
            .contains("local runtime data directory"));
    }
}
