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

use std::path::Path;
use std::sync::Arc;

use crate::local_http::LoopbackHttpBaseUrl;
use anyhow::{Context, Result};
use elastos_common::localhost::rooted_localhost_uri;
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
                    match handle_remote_request(&line, &api_url, &client_token).await {
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

/// Build the capability resource string from scheme, op, and request body.
///
/// For `localhost`: uses `body.path` which may be a full URI or a rooted local
/// path like `Users/self/.AppData/LocalHost/Chat/channels.json`.
/// Rootless bare paths are rejected by returning an invalid localhost resource,
/// which makes capability validation fail closed.
/// For `did`/`peer`: uses `elastos://scheme/*` (wildcard, matching how tokens are granted).
/// For `ai`: uses backend-specific path matching the HTTP handler's logic.
fn build_capability_resource(scheme: &str, op: &str, body: &serde_json::Value) -> String {
    match scheme {
        "localhost" => {
            match body
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|p| !p.is_empty())
            {
                Some(p) => {
                    rooted_localhost_uri(p).unwrap_or_else(|| "localhost://INVALID".to_string())
                }
                None => "localhost://INVALID".to_string(),
            }
        }
        "ai" => {
            let backend = body.get("backend").and_then(|v| v.as_str());
            match backend {
                Some(b)
                    if b.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
                {
                    format!("elastos://ai/{}/{}", b, op)
                }
                _ => format!("elastos://ai/meta/{}", op),
            }
        }
        "did" | "peer" => format!("elastos://{}/*", scheme),
        _ => format!("{}://*", scheme),
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

/// Handle a single request from the guest capsule.
async fn handle_request(line: &str, ctx: &Option<BridgeContext>) -> Result<serde_json::Value> {
    let envelope: serde_json::Value =
        serde_json::from_str(line.trim()).context("Invalid JSON from guest")?;

    let id = envelope["id"].as_u64().unwrap_or(0);
    let request = &envelope["request"];
    let request_type = request["type"].as_str().unwrap_or("");

    let response = match request_type {
        "provider_call" => {
            let bridge_ctx = ctx
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no bridge context"))?;

            let scheme = request["scheme"].as_str().unwrap_or("");
            let op = request["op"].as_str().unwrap_or("");
            let token_b64 = request["token"].as_str().unwrap_or("");

            // Validate capability token before dispatching to provider.
            // The guest SDK sends the token it received from request_capability.
            // Resource is built from scheme+op matching the HTTP handler's logic.
            if !token_b64.is_empty() {
                use elastos_runtime::capability::token::{CapabilityToken, ResourceId};
                match CapabilityToken::from_base64(token_b64) {
                    Ok(token) => {
                        let body = request
                            .get("body")
                            .cloned()
                            .unwrap_or(serde_json::json!({}));
                        let resource = build_capability_resource(scheme, op, &body);
                        let resource_id = ResourceId::new(&resource);
                        if bridge_ctx
                            .capability_manager
                            .validate(
                                &token,
                                &bridge_ctx.capsule_id,
                                token.action(),
                                &resource_id,
                                None,
                            )
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
                        "message": "provider_call requires a capability token",
                    }
                }));
            }

            let body = request
                .get("body")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let mut req = body;
            req["op"] = serde_json::Value::String(op.to_string());

            match bridge_ctx.provider_registry.send_raw(scheme, &req).await {
                Ok(result) => serde_json::json!({
                    "type": "provider_result",
                    "result": result,
                }),
                Err(e) => {
                    tracing::warn!("Bridge provider_call failed for {}/{}: {}", scheme, op, e);
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
        "provider_call" => {
            let scheme = request["scheme"].as_str().unwrap_or("");
            let op = request["op"].as_str().unwrap_or("");
            let body = request
                .get("body")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let cap_token = request["token"].as_str().unwrap_or("");

            tracing::debug!(
                "[wasm-api-bridge] provider_call {}/{} token={} body={}",
                scheme,
                op,
                !cap_token.is_empty(),
                &body.to_string().chars().take(150).collect::<String>()
            );

            let mut req = client
                .post(api_base.join(&format!("/api/provider/{}/{}", scheme, op))?)
                .header("Authorization", format!("Bearer {}", client_token))
                .json(&body);

            if !cap_token.is_empty() {
                req = req.header("X-Capability-Token", cap_token);
            }

            let resp = req.send().await?;
            let status = resp.status();
            let body: serde_json::Value = resp.json().await?;
            tracing::debug!(
                "[wasm-api-bridge] {}/{} → {} {}",
                scheme,
                op,
                status,
                &body.to_string().chars().take(200).collect::<String>()
            );
            serde_json::json!({
                "type": "provider_result",
                "result": body,
            })
        }
        "request_capability" => {
            let resource = request["resource"].as_str().unwrap_or("");
            let action = request["action"].as_str().unwrap_or("execute");

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

    #[test]
    fn test_build_capability_resource_localhost_full_uri() {
        let body = serde_json::json!({"path": "localhost://Users/self/.AppData/LocalHost/Chat/channels.json"});
        let resource = build_capability_resource("localhost", "read", &body);
        assert_eq!(
            resource,
            "localhost://Users/self/.AppData/LocalHost/Chat/channels.json"
        );
    }

    #[test]
    fn test_build_capability_resource_localhost_bare_path() {
        let body = serde_json::json!({"path": "Users/self/.AppData/LocalHost/Chat/channels.json"});
        let resource = build_capability_resource("localhost", "read", &body);
        assert_eq!(
            resource,
            "localhost://Users/self/.AppData/LocalHost/Chat/channels.json"
        );
    }

    #[test]
    fn test_build_capability_resource_localhost_bare_history() {
        let body =
            serde_json::json!({"path": "Users/self/.AppData/LocalHost/Chat/history/general.json"});
        let resource = build_capability_resource("localhost", "write", &body);
        assert_eq!(
            resource,
            "localhost://Users/self/.AppData/LocalHost/Chat/history/general.json"
        );
    }

    #[test]
    fn test_build_capability_resource_localhost_no_path() {
        let body = serde_json::json!({});
        let resource = build_capability_resource("localhost", "read", &body);
        assert_eq!(resource, "localhost://INVALID");
    }

    #[test]
    fn test_build_capability_resource_peer() {
        let body = serde_json::json!({});
        let resource = build_capability_resource("peer", "gossip_join", &body);
        assert_eq!(resource, "elastos://peer/*");
    }

    #[test]
    fn test_build_capability_resource_did() {
        let body = serde_json::json!({});
        let resource = build_capability_resource("did", "get_did", &body);
        assert_eq!(resource, "elastos://did/*");
    }

    #[test]
    fn test_build_capability_resource_ai_with_backend() {
        let body = serde_json::json!({"backend": "local"});
        let resource = build_capability_resource("ai", "chat_completions", &body);
        assert_eq!(resource, "elastos://ai/local/chat_completions");
    }

    #[test]
    fn test_build_capability_resource_ai_no_backend() {
        let body = serde_json::json!({});
        let resource = build_capability_resource("ai", "chat_completions", &body);
        assert_eq!(resource, "elastos://ai/meta/chat_completions");
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
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("attached WASM bridge requires a local runtime API URL"));
    }
}
