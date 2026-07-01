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
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use elastos_compute::providers::BridgePipes;
use elastos_runtime::capability::pending::{PendingRequestStore, RequestStatus};
use elastos_runtime::capability::token::CapabilityToken;
use elastos_runtime::capability::CapabilityManager;
use elastos_runtime::provider::ProviderRegistry;

const CAPABILITY_APPROVAL_POLL_MS: u64 = 100;
const CAPABILITY_APPROVAL_MAX_POLLS: usize = 300;

/// Hard cap on one newline-delimited request from an (untrusted) guest, enforced
/// DURING the read. The previous code checked the length AFTER `read_line` /
/// `.lines()` had already buffered the whole line, so a line with no newline
/// could OOM the host before the check ran (BUG-6). The bounded readers below
/// cap the allocation while reading, then drain to the next newline so the
/// stream realigns to the following request.
const MAX_LINE_BYTES: usize = 1_048_576; // 1 MB
/// Chunk size for draining an oversized line back to stream alignment — bounded
/// so the drain itself never reintroduces the OOM it is preventing.
const DRAIN_CHUNK_BYTES: u64 = 64 * 1024;

/// Outcome of reading one bounded, newline-delimited request line.
#[derive(Debug)]
enum BoundedLine {
    /// A complete request line (trailing `\n`/`\r\n` stripped). May be empty.
    Line(String),
    /// Clean EOF — the peer closed with no pending bytes.
    Eof,
    /// The line exceeded `MAX_LINE_BYTES` before a newline arrived; the overflow
    /// has been drained up to (and including) the next newline so the stream is
    /// realigned to the next request. The caller should reply `request_too_large`.
    TooLarge,
}

/// The single canonical `request_too_large` reply, shared by all three bridges so
/// the wire shape can never drift between them.
fn oversized_request_error() -> serde_json::Value {
    serde_json::json!({ "id": 0, "type": "error", "error": "request_too_large" })
}

/// Read one line from an async reader without ever buffering more than
/// `MAX_LINE_BYTES` (+1) bytes — the fail-closed inverse of an unbounded
/// `read_line`. On overflow it drains to the next newline and reports
/// [`BoundedLine::TooLarge`] rather than allocating the whole oversized line.
async fn read_bounded_line<R>(reader: &mut R) -> std::io::Result<BoundedLine>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf = Vec::new();
    // `take` makes the read itself stop at the cap, so a newline-less flood
    // cannot grow `buf` past the bound regardless of how much the guest sends.
    let n = (&mut *reader)
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(BoundedLine::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        // Lossy is safe: a ≤1 MB buffer, and a non-UTF8 body just fails JSON
        // parsing downstream (bridge_error) instead of killing the connection.
        return Ok(BoundedLine::Line(
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    }
    // Hit the cap with no newline → oversized. Realign, then report.
    drain_to_newline_async(reader).await?;
    Ok(BoundedLine::TooLarge)
}

/// Discard bytes (in bounded chunks) up to and including the next newline, so the
/// reader is positioned at the start of the next request after an overflow.
async fn drain_to_newline_async<R>(reader: &mut R) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let mut sink = Vec::new();
        let n = (&mut *reader)
            .take(DRAIN_CHUNK_BYTES)
            .read_until(b'\n', &mut sink)
            .await?;
        if n == 0 || sink.last() == Some(&b'\n') {
            return Ok(()); // EOF, or realigned at the newline
        }
        // Still draining a huge line; `sink` is dropped each pass so memory
        // stays bounded by DRAIN_CHUNK_BYTES.
    }
}

/// Synchronous twin of [`read_bounded_line`] for the pipe-backed WASM bridges
/// (`std::io::BufRead`), with the same fail-closed bound + realign semantics.
fn read_bounded_line_sync<R: std::io::BufRead>(reader: &mut R) -> std::io::Result<BoundedLine> {
    use std::io::{BufRead, Read};
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(BoundedLine::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(BoundedLine::Line(
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    }
    drain_to_newline_sync(reader)?;
    Ok(BoundedLine::TooLarge)
}

fn drain_to_newline_sync<R: std::io::BufRead>(reader: &mut R) -> std::io::Result<()> {
    use std::io::{BufRead, Read};
    loop {
        let mut sink = Vec::new();
        let n = reader
            .by_ref()
            .take(DRAIN_CHUNK_BYTES)
            .read_until(b'\n', &mut sink)?;
        if n == 0 || sink.last() == Some(&b'\n') {
            return Ok(());
        }
    }
}

/// Terminal outcome of awaiting the consent-broker's decision on a pending
/// capability request.
enum CapabilityDecision {
    Granted(Box<CapabilityToken>),
    Denied(String),
    Expired,
    /// No decision within the poll budget.
    TimedOut,
}

/// Classify a pending request's current status into a terminal decision, or
/// `None` while it is still pending / absent.
async fn poll_capability_decision(
    store: &PendingRequestStore,
    request_id: &str,
) -> Option<CapabilityDecision> {
    let req = store.get_request(request_id).await?;
    match req.status {
        RequestStatus::Granted { token, .. } => Some(CapabilityDecision::Granted(token)),
        RequestStatus::Denied { reason } => Some(CapabilityDecision::Denied(reason)),
        RequestStatus::Expired => Some(CapabilityDecision::Expired),
        _ => None,
    }
}

/// Await the consent-broker's grant/deny decision: poll the store on an interval,
/// then do ONE final read after the loop. The loop sleeps BEFORE each check, so a
/// decision landing between the last in-loop poll and loop exit would otherwise be
/// dropped to a spurious timeout (BUG-5); the trailing read closes that window.
async fn await_capability_decision(
    store: &PendingRequestStore,
    request_id: &str,
    max_polls: usize,
    poll_ms: u64,
) -> CapabilityDecision {
    for _ in 0..max_polls {
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
        if let Some(decision) = poll_capability_decision(store, request_id).await {
            return decision;
        }
    }
    // Final read — catch a decision that landed after the last in-loop poll.
    poll_capability_decision(store, request_id)
        .await
        .unwrap_or(CapabilityDecision::TimedOut)
}

/// Poll `poll` up to `max_polls` times on an interval, then do ONE final read —
/// the transport-agnostic shape of the BUG-5 fix. `poll` returns `Ok(Some(_))`
/// once a decision is reached, `Ok(None)` while still pending; transport errors
/// propagate. The trailing read closes the window where a decision lands between
/// the last in-loop poll and the timeout (the in-process twin is
/// `await_capability_decision`; this serves the WASM-API bridge's HTTP poll).
async fn poll_then_final_read<T, F, Fut>(
    max_polls: usize,
    poll_ms: u64,
    mut poll: F,
) -> anyhow::Result<Option<T>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<T>>>,
{
    for _ in 0..max_polls {
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
        if let Some(decided) = poll().await? {
            return Ok(Some(decided));
        }
    }
    poll().await
}

/// A decided outcome of polling the remote capability endpoint over HTTP.
enum RemoteCapabilityPoll {
    /// The capability was granted; carries the encoded token.
    Token(String),
    /// A terminal error (denied/expired) already shaped as the bridge response.
    Terminal(serde_json::Value),
}

/// Per-act spend-meter policy for the carrier act path: a shared meter plus the default per-capsule
/// budget to lazily provision on first sight. Carried as `Option` on [`BridgeContext`] — `None` ⇒
/// metering OFF (acts flow unmetered, today's behavior). Enabled only when an operator configures a
/// default budget (`ELASTOS_DEFAULT_SPEND_BUDGET`); a default of `0` is fail-closed-zero (every act
/// refused), so an unset/empty env stays unmetered while an explicit `0` hard-stops all acts.
#[derive(Clone)]
pub struct SpendPolicy {
    pub meter: Arc<elastos_runtime::primitives::spend::SpendMeter>,
    pub default_budget: elastos_runtime::primitives::spend::SpendUnits,
}

/// Spend charged per carrier act (v0: one unit per act — bounds the NUMBER of acts a capsule may
/// take; provider-reported variable cost, e.g. AI tokens consumed, is the follow-up).
const CARRIER_ACT_COST: elastos_runtime::primitives::spend::SpendUnits = 1;

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
    /// Per-act spend budget enforcement; `None` ⇒ unmetered (see [`SpendPolicy`]).
    pub spend_policy: Option<SpendPolicy>,
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
) -> Result<tokio::task::JoinHandle<()>> {
    // Remove stale socket and create a listener BEFORE crosvm starts.
    // crosvm --serial type=unix-stream connects to this socket on launch.
    let _ = tokio::fs::remove_file(socket_path).await;
    let listener = tokio::net::UnixListener::bind(socket_path)
        .context("Failed to bind microVM Carrier bridge socket")?;

    let socket_display = socket_path.display().to_string();
    let ctx = bridge_ctx;

    // Accept one bidirectional connection in background — crosvm connects when
    // the VM boots. The supported contract is a single `unix-stream` socket
    // with `input-unix-stream` enabled on the crosvm side. The handle is returned
    // so the supervisor can abort this task on VM teardown (BUG-2: it was detached
    // and leaked, along with the unix socket file).
    let task = tokio::spawn(async move {
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
        let mut reader = BufReader::new(reader);

        loop {
            let line = match read_bounded_line(&mut reader).await {
                Ok(BoundedLine::Eof) => break, // EOF — guest shut down
                Ok(BoundedLine::Line(line)) => line,
                Ok(BoundedLine::TooLarge) => {
                    tracing::warn!(
                        "Carrier bridge: oversized line (> {} bytes), dropping",
                        MAX_LINE_BYTES
                    );
                    let error = oversized_request_error();
                    let _ = writer.write_all(error.to_string().as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                    continue;
                }
                Err(e) => {
                    tracing::debug!("Carrier bridge read error: {}", e);
                    break;
                }
            };

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

    Ok(task)
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
            use std::io::Write;

            let mut reader = std::io::BufReader::new(pipes.capsule_stdout);
            let mut writer = pipes.capsule_stdin;
            let ctx = Some(ctx);

            loop {
                let line = match read_bounded_line_sync(&mut reader) {
                    Ok(BoundedLine::Eof) => break,
                    Ok(BoundedLine::Line(line)) => line,
                    Ok(BoundedLine::TooLarge) => {
                        tracing::warn!("WASM bridge: oversized line (> {} bytes), dropping", MAX_LINE_BYTES);
                        let error = oversized_request_error();
                        let _ = writeln!(writer, "{}", error);
                        let _ = writer.flush();
                        continue;
                    }
                    Err(e) => {
                        tracing::debug!("WASM bridge read error: {}", e);
                        break;
                    }
                };

                if line.trim().is_empty() {
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
            use std::io::Write;

            let mut reader = std::io::BufReader::new(pipes.capsule_stdout);
            let mut writer = pipes.capsule_stdin;

            loop {
                let line = match read_bounded_line_sync(&mut reader) {
                    Ok(BoundedLine::Eof) => break,
                    Ok(BoundedLine::Line(line)) => line,
                    Ok(BoundedLine::TooLarge) => {
                        tracing::warn!(
                            "WASM API bridge: oversized line (> {} bytes), dropping",
                            MAX_LINE_BYTES
                        );
                        let error = oversized_request_error();
                        let _ = writeln!(writer, "{}", error);
                        let _ = writer.flush();
                        continue;
                    }
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
///
/// `pub(crate)` so the `elastos mcp serve` edge can reuse the ONE canonical
/// gate-then-dispatch (validate → send_raw) verbatim, holding the bridge token, rather
/// than re-implementing enforcement at the MCP edge.
pub(crate) async fn handle_request(
    line: &str,
    ctx: &Option<BridgeContext>,
) -> Result<serde_json::Value> {
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
            // The id of the (single-use-)consumed token is captured so that a
            // PROVABLY no-op failure — the provider was never invoked (routing
            // failure) — can refund the use instead of burning the grant (BUG-4).
            // Reaching the provider dispatch below implies a token was validated
            // and its single use consumed — so this is bound exactly once (no dead
            // `None` init); the only fall-through is the validated path.
            let consumed_token_id: elastos_runtime::capability::token::TokenId = if !token_b64
                .is_empty()
            {
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
                        // Validation passed — the single use is now consumed.
                        *token.id()
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
            };

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

            // SPEND METER (act-over-MCP): charge the capsule's budget for this act BEFORE dispatch,
            // keyed on the canonical capsule id. Fail-closed: an exhausted budget REFUSES the act and
            // refunds the single-use token — nothing reached the provider, so a replay is a guaranteed
            // no-op, the SAME provably-no-op refund as the NoProvider routing-failure branch below.
            if let Some(policy) = &bridge_ctx.spend_policy {
                // Lazily provision the per-capsule default on first sight (idempotent — never resets
                // an existing balance). default_budget == 0 ⇒ fail-closed-zero (every act refused).
                policy
                    .meter
                    .ensure_budget(&bridge_ctx.capsule_id, policy.default_budget);
                match policy
                    .meter
                    .try_debit(&bridge_ctx.capsule_id, CARRIER_ACT_COST)
                {
                    // Reserved the minimum 1 unit (fail-closed if broke). The SpendDebit is recorded
                    // POST-dispatch — after we know the act stuck and the provider's actual cost — so
                    // the refund branches don't leave a phantom debit in the custody log.
                    Ok(_remaining) => {}
                    Err(spend_err) => {
                        let remaining_use = bridge_ctx
                            .capability_manager
                            .refund_use(&consumed_token_id)
                            .await;
                        bridge_ctx.capability_manager.audit_log().emit_best_effort(
                            elastos_runtime::primitives::audit::AuditEvent::BudgetExhausted {
                                timestamp: elastos_runtime::primitives::time::SecureTimestamp::now(
                                ),
                                capsule_id: bridge_ctx.capsule_id.clone(),
                                operation: dispatch.operation.clone(),
                                requested: CARRIER_ACT_COST,
                            },
                        );
                        tracing::warn!(
                            "bridge: spend budget exhausted for capsule '{}' op '{}' ({}); refused \
                             before dispatch, refunded the single-use grant (use count now {})",
                            bridge_ctx.capsule_id,
                            dispatch.operation,
                            spend_err,
                            remaining_use,
                        );
                        return Ok(serde_json::json!({
                            "id": id,
                            "response": {
                                "type": "error",
                                "code": "budget_exhausted",
                                "message": "Spend budget exhausted for this capsule",
                            }
                        }));
                    }
                }
            }

            match bridge_ctx
                .provider_registry
                .send_raw(&dispatch.scheme, &dispatch.request)
                .await
            {
                Ok(result) => {
                    // The act stuck. Record the real charge: the reserved 1 unit plus any provider-
                    // reported overage. A provider may report `cost_units` (real resource consumed,
                    // e.g. AI tokens); absent ⇒ the flat 1/act (back-compatible). The overage is
                    // charged saturating — the act already happened, so an over-budget cost drains
                    // the remainder to zero and the NEXT act is refused fail-closed by `try_debit`.
                    if let Some(policy) = &bridge_ctx.spend_policy {
                        let reported = result
                            .get("cost_units")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(CARRIER_ACT_COST)
                            .max(CARRIER_ACT_COST);
                        let mut charged = CARRIER_ACT_COST;
                        if reported > CARRIER_ACT_COST {
                            charged += policy.meter.debit_saturating(
                                &bridge_ctx.capsule_id,
                                reported - CARRIER_ACT_COST,
                            );
                        }
                        let remaining = policy.meter.remaining(&bridge_ctx.capsule_id);
                        bridge_ctx.capability_manager.audit_log().emit_best_effort(
                            elastos_runtime::primitives::audit::AuditEvent::SpendDebit {
                                timestamp: elastos_runtime::primitives::time::SecureTimestamp::now(
                                ),
                                capsule_id: bridge_ctx.capsule_id.clone(),
                                operation: dispatch.operation.clone(),
                                cost: charged,
                                remaining,
                            },
                        );
                    }
                    serde_json::json!({
                        "type": "carrier_result",
                        "result": result,
                    })
                }
                Err(elastos_runtime::provider::ProviderError::NoProvider(scheme)) => {
                    // BUG-4 safe slice: the registry matched NO provider, so NOTHING
                    // ran — the consumed single use was provably a no-op. Refund it
                    // so the holder is not charged for a routing failure. This is the
                    // ONLY error class where a refund cannot enable a double-effect:
                    // every provider op is non-atomic on its own Err path (write-then-
                    // fail is possible), so an op failure keeps the use consumed
                    // (fail-closed) — see docs/KNOWN_GAPS.md BUG-4.
                    let remaining = bridge_ctx
                        .capability_manager
                        .refund_use(&consumed_token_id)
                        .await;
                    // Nothing acted ⇒ also refund the spend debit (the same provably-no-op contract).
                    if let Some(policy) = &bridge_ctx.spend_policy {
                        policy
                            .meter
                            .refund(&bridge_ctx.capsule_id, CARRIER_ACT_COST);
                    }
                    tracing::warn!(
                        "bridge: no provider for scheme '{}'; refunded the unused \
                         single-use grant (use count now {})",
                        scheme,
                        remaining,
                    );
                    serde_json::json!({
                        "type": "error",
                        "code": "provider_not_found",
                        "message": "No provider for the requested scheme",
                    })
                }
                Err(elastos_runtime::provider::ProviderError::DidNotAct(reason)) => {
                    // BUG-4 (op-failure slice): the provider rejected the request
                    // BEFORE any side effect (its DidNotAct ocap contract), so the
                    // consumed single use was a no-op — refund it. A replay of the
                    // same rejected request is idempotent, so this cannot double-act.
                    let remaining = bridge_ctx
                        .capability_manager
                        .refund_use(&consumed_token_id)
                        .await;
                    // Provably pre-effect ⇒ also refund the spend debit (replay is idempotent).
                    if let Some(policy) = &bridge_ctx.spend_policy {
                        policy
                            .meter
                            .refund(&bridge_ctx.capsule_id, CARRIER_ACT_COST);
                    }
                    tracing::warn!(
                        "bridge: provider rejected '{}/{}' before acting ({}); refunded \
                         the unused single-use grant (use count now {})",
                        dispatch.scheme,
                        dispatch.operation,
                        reason,
                        remaining,
                    );
                    serde_json::json!({
                        "type": "error",
                        "code": "rejected",
                        "message": "Provider rejected the request before acting",
                    })
                }
                Err(e) => {
                    // The provider RAN and may have partially acted, so the single
                    // use stays consumed (refunding could enable a re-run of a
                    // partially-applied effect — BUG-4). Only NoProvider (routing)
                    // and DidNotAct (pre-effect rejection) are refund-safe.
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
                    .create_request_with_capsule(
                        elastos_runtime::session::SessionId(ctx.capsule_id.clone()),
                        resource_id.clone(),
                        action,
                        // The carrier already knows the real capsule identity
                        // ("vm-{name}"); record it on the request (G-ID interim).
                        Some(ctx.capsule_id.clone()),
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
                    // Await the consent-broker's decision (AutoGrantEngine or
                    // manual): poll the store, then a final read so a grant that
                    // lands after the last poll is not dropped to a timeout (BUG-5).
                    match await_capability_decision(
                        &ctx.pending_store,
                        &request_id,
                        CAPABILITY_APPROVAL_MAX_POLLS,
                        CAPABILITY_APPROVAL_POLL_MS,
                    )
                    .await
                    {
                        CapabilityDecision::Granted(token) => {
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
                        }
                        CapabilityDecision::Denied(reason) => {
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
                        CapabilityDecision::Expired => {
                            return Ok(serde_json::json!({
                                "id": id,
                                "response": {
                                    "type": "error",
                                    "code": "expired",
                                    "message": "capability request expired",
                                },
                            }));
                        }
                        CapabilityDecision::TimedOut => {
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

pub(crate) fn encode_bridge_capability_token(
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

                // Poll the remote decision, then a final read so a grant landing
                // after the last poll is not dropped to a timeout (BUG-5, HTTP twin
                // of the in-process `await_capability_decision`).
                let outcome = poll_then_final_read(
                    CAPABILITY_APPROVAL_MAX_POLLS,
                    CAPABILITY_APPROVAL_POLL_MS,
                    || async {
                        let resp = client
                            .get(api_base.join(&format!("/api/capability/request/{}", request_id))?)
                            .header("Authorization", format!("Bearer {}", client_token))
                            .send()
                            .await?;
                        let status: serde_json::Value = resp.json().await?;
                        if let Some(granted) = status.get("token").and_then(|t| t.as_str()) {
                            return Ok(Some(RemoteCapabilityPoll::Token(granted.to_string())));
                        }
                        match status.get("status").and_then(|s| s.as_str()) {
                            Some("denied") | Some("expired") => {
                                Ok(Some(RemoteCapabilityPoll::Terminal(serde_json::json!({
                                    "id": id,
                                    "response": {
                                        "type": "error",
                                        "code": status.get("status").and_then(|s| s.as_str()).unwrap_or("error"),
                                        "message": status.get("reason").and_then(|r| r.as_str()).unwrap_or("capability request failed"),
                                    }
                                }))))
                            }
                            _ => Ok(None),
                        }
                    },
                )
                .await?;

                match outcome {
                    Some(RemoteCapabilityPoll::Token(token)) => serde_json::json!({
                        "type": "capability_token",
                        "token": token,
                    }),
                    Some(RemoteCapabilityPoll::Terminal(json)) => return Ok(json),
                    None => {
                        return Err(anyhow::anyhow!(
                            "capability request still pending after 30s"
                        ))
                    }
                }
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
            spend_policy: None,
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

    /// BUG-4 safe slice: when the registry matches NO provider (routing failure),
    /// nothing ran, so the consumed single-use grant is refunded and stays usable.
    /// Proven through the public `handle_request` path: the SAME single-use token
    /// reaches the provider lookup TWICE. Under the old burn-on-failure code the
    /// second call would be `capability_denied` (use limit exceeded).
    #[tokio::test]
    async fn carrier_invoke_refunds_single_use_grant_on_missing_provider() {
        use elastos_runtime::capability::token::TokenConstraints;

        let ctx = bridge_context(); // empty ProviderRegistry → every scheme is NoProvider
        let uri = "elastos://rights/has_access_by_content_id";
        let operation = "has_access_by_content_id";
        let probe = serde_json::json!({
            "type": "carrier_invoke",
            "uri": uri,
            "operation": operation,
            "body": {}
        });
        // Compute the exact resource + action the bridge will enforce, so the
        // token matches and validation actually consumes the single use.
        let dispatch = carrier_invoke_dispatch(&probe, None).expect("dispatch parses");
        let required = required_action_for(&dispatch.operation);
        let token = ctx.capability_manager.grant(
            &ctx.capsule_id,
            ResourceId::new(&dispatch.resource),
            required,
            TokenConstraints::new(0, false, None, Some(1)), // single-use
            None,
        );
        let token_b64 = encode_bridge_capability_token(&token);

        let call = |tok: String| {
            serde_json::json!({
                "id": 1,
                "request": {
                    "type": "carrier_invoke",
                    "uri": uri,
                    "operation": operation,
                    "token": tok,
                    "body": {}
                }
            })
            .to_string()
        };

        let ctx_opt = Some(ctx.clone());

        // 1st call: provider missing → routing failure → refund the unused use.
        let r1 = handle_request(&call(token_b64.clone()), &ctx_opt)
            .await
            .expect("bridge responds");
        assert_eq!(r1["response"]["code"], "provider_not_found");

        // 2nd call with the SAME token: the refund made the single use available
        // again, so validation passes and we reach the provider lookup once more —
        // NOT capability_denied, which is what burning the grant would have caused.
        let r2 = handle_request(&call(token_b64), &ctx_opt)
            .await
            .expect("bridge responds");
        assert_eq!(
            r2["response"]["code"], "provider_not_found",
            "refunded single-use grant is reusable; old code would return capability_denied"
        );
    }

    // --- BUG-4 (op-failure slice): DidNotAct refunds; an acted failure does not ---

    /// A provider that PROVABLY rejects before any side effect (the DidNotAct ocap
    /// contract) — e.g. a read-only / validate-first provider.
    struct RejectsBeforeActingProvider;
    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for RejectsBeforeActingProvider {
        async fn handle(
            &self,
            _r: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            Err(elastos_runtime::provider::ProviderError::DidNotAct(
                "unused".into(),
            ))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["rights"]
        }
        fn name(&self) -> &'static str {
            "test-rejects-before-acting"
        }
        async fn send_raw(
            &self,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
            Err(elastos_runtime::provider::ProviderError::DidNotAct(
                "precondition failed; nothing mutated".into(),
            ))
        }
    }

    /// A provider that MUTATED and then failed — refunding here would let the
    /// holder re-run a partially-applied effect, so the use must stay consumed.
    struct ActsThenFailsProvider;
    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for ActsThenFailsProvider {
        async fn handle(
            &self,
            _r: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            Err(elastos_runtime::provider::ProviderError::Provider(
                "unused".into(),
            ))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["rights"]
        }
        fn name(&self) -> &'static str {
            "test-acts-then-fails"
        }
        async fn send_raw(
            &self,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
            Err(elastos_runtime::provider::ProviderError::Provider(
                "wrote then failed".into(),
            ))
        }
    }

    // Grant a single-use rights token + a two-arg `handle_request` driver over the
    // shared rights-op dispatch, for the two op-failure cases below.
    async fn single_use_rights_call(ctx: &BridgeContext) -> (String, impl Fn(String) -> String) {
        use elastos_runtime::capability::token::TokenConstraints;
        let uri = "elastos://rights/has_access_by_content_id";
        let operation = "has_access_by_content_id";
        let probe = serde_json::json!({
            "type": "carrier_invoke", "uri": uri, "operation": operation, "body": {}
        });
        let dispatch = carrier_invoke_dispatch(&probe, None).expect("dispatch parses");
        let required = required_action_for(&dispatch.operation);
        let token = ctx.capability_manager.grant(
            &ctx.capsule_id,
            ResourceId::new(&dispatch.resource),
            required,
            TokenConstraints::new(0, false, None, Some(1)),
            None,
        );
        let token_b64 = encode_bridge_capability_token(&token);
        let call = move |tok: String| {
            serde_json::json!({
                "id": 1,
                "request": {
                    "type": "carrier_invoke", "uri": uri, "operation": operation,
                    "token": tok, "body": {}
                }
            })
            .to_string()
        };
        (token_b64, call)
    }

    /// DidNotAct (provably pre-effect) refunds the single use — the SAME token
    /// reaches the provider twice (second call is still `rejected`, not denied).
    #[tokio::test]
    async fn carrier_invoke_refunds_single_use_grant_on_did_not_act() {
        let ctx = bridge_context();
        ctx.provider_registry
            .register(Arc::new(RejectsBeforeActingProvider))
            .await;
        let (token_b64, call) = single_use_rights_call(&ctx).await;
        let ctx_opt = Some(ctx.clone());

        let r1 = handle_request(&call(token_b64.clone()), &ctx_opt)
            .await
            .unwrap();
        assert_eq!(r1["response"]["code"], "rejected");

        let r2 = handle_request(&call(token_b64), &ctx_opt).await.unwrap();
        assert_eq!(
            r2["response"]["code"], "rejected",
            "DidNotAct refunds the single use; the grant is reusable"
        );
    }

    // --- spend meter: the act-over-MCP budget bounds the number of acts, fail-closed ---

    /// With budget N, the first N acts dispatch and the (N+1)th is REFUSED before the provider —
    /// and the over-limit act's single-use token is refunded (nothing acted).
    #[tokio::test]
    async fn carrier_act_is_refused_when_spend_budget_is_exhausted() {
        use elastos_runtime::capability::token::TokenConstraints;
        use elastos_runtime::primitives::spend::SpendMeter;

        let mut ctx = bridge_context();
        let meter = Arc::new(SpendMeter::new());
        ctx.spend_policy = Some(super::SpendPolicy {
            meter: meter.clone(),
            default_budget: 2,
        });
        ctx.provider_registry
            .register(Arc::new(RightsOkProvider))
            .await;

        let uri = "elastos://rights/has_access_by_content_id";
        let operation = "has_access_by_content_id";
        let probe = serde_json::json!({
            "type": "carrier_invoke", "uri": uri, "operation": operation, "body": {}
        });
        let dispatch = carrier_invoke_dispatch(&probe, None).expect("dispatch parses");
        // The token is NOT the limiting factor here (10 uses); the budget (2) is.
        let token = encode_bridge_capability_token(&ctx.capability_manager.grant(
            &ctx.capsule_id,
            ResourceId::new(&dispatch.resource),
            required_action_for(&dispatch.operation),
            TokenConstraints::new(0, false, None, Some(10)),
            None,
        ));
        let call = |tok: &str| {
            serde_json::json!({
                "id": 1,
                "request": {
                    "type": "carrier_invoke", "uri": uri, "operation": operation,
                    "token": tok, "body": {}
                }
            })
            .to_string()
        };
        let ctx_opt = Some(ctx.clone());

        // Budget 2 → the first two acts reach the provider.
        for i in 1..=2 {
            let r = handle_request(&call(&token), &ctx_opt).await.unwrap();
            assert_eq!(
                r["response"]["type"], "carrier_result",
                "act {i} should dispatch within budget: {r}"
            );
        }
        assert_eq!(meter.remaining(&ctx.capsule_id), 0, "budget fully spent");

        // Third act: refused fail-closed, before the provider.
        let denied = handle_request(&call(&token), &ctx_opt).await.unwrap();
        assert_eq!(
            denied["response"]["code"], "budget_exhausted",
            "the over-budget act must be refused before dispatch: {denied}"
        );
        assert_eq!(
            meter.remaining(&ctx.capsule_id),
            0,
            "a refused act charges nothing further"
        );
    }

    /// An act the provider rejects BEFORE acting (DidNotAct) refunds the spend debit — a no-op act
    /// costs no budget, exactly mirroring the single-use token refund on the same branch.
    #[tokio::test]
    async fn did_not_act_refunds_the_spend_debit() {
        use elastos_runtime::primitives::spend::SpendMeter;

        let mut ctx = bridge_context();
        let meter = Arc::new(SpendMeter::new());
        ctx.spend_policy = Some(super::SpendPolicy {
            meter: meter.clone(),
            default_budget: 1,
        });
        ctx.provider_registry
            .register(Arc::new(RejectsBeforeActingProvider))
            .await;

        let (token_b64, call) = single_use_rights_call(&ctx).await;
        let ctx_opt = Some(ctx.clone());

        let r = handle_request(&call(token_b64), &ctx_opt).await.unwrap();
        assert_eq!(r["response"]["code"], "rejected");
        assert_eq!(
            meter.remaining(&ctx.capsule_id),
            1,
            "DidNotAct refunds the spend; the budget is intact for a real act"
        );
    }

    /// A provider that reports the units it actually consumed (`cost_units`), so the meter bounds
    /// real spend, not just the call count.
    struct CostReportingProvider;
    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for CostReportingProvider {
        async fn handle(
            &self,
            _r: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            Err(elastos_runtime::provider::ProviderError::Provider(
                "test cost provider answers via send_raw only".into(),
            ))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["rights"]
        }
        fn name(&self) -> &'static str {
            "test-cost-reporting"
        }
        async fn send_raw(
            &self,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
            Ok(serde_json::json!({ "status": "ok", "cost_units": 5 }))
        }
    }

    /// Variable cost: a provider reporting 5 units exhausts a budget of 8 in TWO acts (5 + drain to
    /// 0), and the third act is refused before dispatch — the budget bounds real spend, not calls.
    #[tokio::test]
    async fn variable_cost_charges_provider_reported_units() {
        use elastos_runtime::capability::token::TokenConstraints;
        use elastos_runtime::primitives::spend::SpendMeter;

        let mut ctx = bridge_context();
        let meter = Arc::new(SpendMeter::new());
        ctx.spend_policy = Some(super::SpendPolicy {
            meter: meter.clone(),
            default_budget: 8,
        });
        ctx.provider_registry
            .register(Arc::new(CostReportingProvider))
            .await;

        let uri = "elastos://rights/has_access_by_content_id";
        let operation = "has_access_by_content_id";
        let probe = serde_json::json!({
            "type": "carrier_invoke", "uri": uri, "operation": operation, "body": {}
        });
        let dispatch = carrier_invoke_dispatch(&probe, None).expect("dispatch parses");
        let token = encode_bridge_capability_token(&ctx.capability_manager.grant(
            &ctx.capsule_id,
            ResourceId::new(&dispatch.resource),
            required_action_for(&dispatch.operation),
            TokenConstraints::new(0, false, None, Some(10)),
            None,
        ));
        let call = |tok: &str| {
            serde_json::json!({
                "id": 1,
                "request": {
                    "type": "carrier_invoke", "uri": uri, "operation": operation,
                    "token": tok, "body": {}
                }
            })
            .to_string()
        };
        let ctx_opt = Some(ctx.clone());

        let r1 = handle_request(&call(&token), &ctx_opt).await.unwrap();
        assert_eq!(r1["response"]["type"], "carrier_result");
        assert_eq!(
            meter.remaining(&ctx.capsule_id),
            3,
            "first act charged the reported 5 of 8"
        );

        let r2 = handle_request(&call(&token), &ctx_opt).await.unwrap();
        assert_eq!(r2["response"]["type"], "carrier_result");
        assert_eq!(
            meter.remaining(&ctx.capsule_id),
            0,
            "second act drains the remaining 3 (saturating — the act already happened)"
        );

        let r3 = handle_request(&call(&token), &ctx_opt).await.unwrap();
        assert_eq!(
            r3["response"]["code"], "budget_exhausted",
            "the over-budget third act is refused before dispatch: {r3}"
        );
    }

    /// An ACTED failure (provider ran and may have partially mutated) keeps the
    /// single use consumed — the second call is denied (refund would double-act).
    #[tokio::test]
    async fn carrier_invoke_keeps_single_use_consumed_on_acted_failure() {
        let ctx = bridge_context();
        ctx.provider_registry
            .register(Arc::new(ActsThenFailsProvider))
            .await;
        let (token_b64, call) = single_use_rights_call(&ctx).await;
        let ctx_opt = Some(ctx.clone());

        let r1 = handle_request(&call(token_b64.clone()), &ctx_opt)
            .await
            .unwrap();
        assert_eq!(r1["response"]["code"], "provider_error");

        let r2 = handle_request(&call(token_b64), &ctx_opt).await.unwrap();
        assert_eq!(
            r2["response"]["code"], "capability_denied",
            "an acted failure keeps the use consumed (no unsafe refund)"
        );
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
            spend_policy: None,
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

    #[tokio::test]
    async fn carrier_inspect_discover_is_admin_locked() {
        // The cross-capsule capability map (op=discover) is a System-gateway-only
        // surface. Over the CARRIER it is fail-closed: required_action_for("discover")
        // is the Admin default (discover is deliberately absent from
        // inspect_op_required_action), so a routine inspect/* Read token -- the SAME
        // grant that DOES reach op=capsules -- is denied for discover BEFORE the
        // provider runs. discover does not inherit the existing System inspect ops'
        // carrier reachability.
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

        // The SAME inspect/* Read grant that reaches op=capsules above.
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
            spend_policy: None,
        });

        let line = serde_json::json!({
            "id": 1,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://inspect/discover",
                "operation": "discover",
                "token": token,
            }
        })
        .to_string();
        let resp = handle_request(&line, &ctx).await.unwrap();
        // Denied at the capability gate (Admin required, Read held) — the provider
        // never runs, the capability map is never disclosed.
        assert_eq!(resp["response"]["type"], "error");
        assert_eq!(resp["response"]["code"], "capability_denied");
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
            spend_policy: None,
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

    // G3 (act leg): the gate the agent is SHOWN in preview must equal the gate the
    // carrier actually ENFORCES when the SAME provider operation is dispatched and
    // executed. Preview (invoke::plan_provider_operation over the manifest's
    // authority) and enforcement (provider_resource::required_action_for, the
    // carrier verb map) are two DISJOINT derivations with no shared function, so
    // proving them equal through a REAL executed dispatch closes the "shown one
    // gate, a different one enforced" hazard. Target: rights-provider's read-only
    // `has_access_by_content_id` (key-free, fund-free, engine-free). An in-process
    // rights provider lets an authorized dispatch actually reach execution.
    struct RightsOkProvider;

    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for RightsOkProvider {
        async fn handle(
            &self,
            _r: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            Err(elastos_runtime::provider::ProviderError::Provider(
                "test rights provider answers via send_raw only".into(),
            ))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["rights"]
        }
        fn name(&self) -> &'static str {
            "test-rights-ok"
        }
        async fn send_raw(
            &self,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
            Ok(serde_json::json!({ "status": "ok", "allowed": true }))
        }
    }

    #[tokio::test]
    async fn carrier_rights_op_gate_enforces_exactly_the_previewed_action() {
        use crate::inspect_provider::{CatalogInspectSource, InspectProvider, InspectSource};
        use crate::provider_resource::build_capability_resource;

        // Seed the rights-provider authority into a catalog tempdir so the inspect
        // `plan` preview reflects the manifest the runtime ships.
        let tmp = tempfile::tempdir().unwrap();
        let capsule_dir = tmp.path().join("capsules").join("rights-provider");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "rights-provider",
                "role": "provider", "type": "microvm", "entrypoint": "rootfs.ext4",
                "provides": "elastos://rights/*",
                "authority": {
                    "reason": "test",
                    "capabilities": [{
                        "resource": "elastos://rights/*",
                        "actions": ["read"],
                        "operations": ["has_access_by_content_id"]
                    }],
                    "audit_events": ["rights.status"]
                }
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
        // In-process rights provider so an AUTHORIZED dispatch actually executes.
        registry.register(Arc::new(RightsOkProvider)).await;

        let ctx = Some(BridgeContext {
            provider_registry: registry,
            capability_manager: capability_manager.clone(),
            pending_store: Arc::new(
                elastos_runtime::capability::pending::PendingRequestStore::new(audit_log),
            ),
            capsule_id: "test-capsule".to_string(),
            principal_id: None,
            data_dir: None,
            spend_policy: None,
        });

        // 1. PREVIEW (executed through the carrier): the action the agent is SHOWN
        //    for has_access_by_content_id, read out of a real `plan` dispatch.
        let preview_token = encode_bridge_capability_token(&capability_manager.grant(
            "test-capsule",
            ResourceId::new("elastos://inspect/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        ));
        let plan_line = serde_json::json!({
            "id": 1,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://inspect/plan",
                "operation": "plan",
                "token": preview_token,
                "body": { "id": "capsule:rights-provider", "operation": "has_access_by_content_id" }
            }
        })
        .to_string();
        let preview = handle_request(&plan_line, &ctx).await.unwrap();
        assert_eq!(preview["response"]["type"], "carrier_result");
        assert_eq!(
            preview["response"]["result"]["data"]["capability_actions"],
            serde_json::json!(["read"]),
            "preview must surface the manifest-declared action for the op: {preview}"
        );

        // The carrier derives the SAME resource for the dispatched op; grant at it
        // exactly so the gate's resource check matches and the ONLY variable is the
        // action (so a denial below is provably action-driven, not resource-driven).
        let rights_resource =
            build_capability_resource("rights", "has_access_by_content_id", &serde_json::json!({}))
                .expect("rights op resource builds");

        // 2. ENFORCE / ALLOW (executed): a token sized to the PREVIEWED action
        //    (Read) clears the gate for the ACTUAL op — required_action_for(
        //    has_access_by_content_id) = Read — and REACHES the provider. The
        //    executed dispatch is the rights op itself, so the gate enforced is
        //    that op's gate, NOT the `plan` gate.
        let allow_token = encode_bridge_capability_token(&capability_manager.grant(
            "test-capsule",
            ResourceId::new(&rights_resource),
            Action::Read,
            TokenConstraints::default(),
            None,
        ));
        let allow_line = serde_json::json!({
            "id": 2,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://rights/access/has_access_by_content_id",
                "operation": "has_access_by_content_id",
                "token": allow_token,
            }
        })
        .to_string();
        let allowed = handle_request(&allow_line, &ctx).await.unwrap();
        assert_eq!(allowed["response"]["type"], "carrier_result");
        assert_eq!(
            allowed["response"]["result"]["status"], "ok",
            "a token sized to the previewed action must pass the enforced gate AND reach the provider: {allowed}"
        );

        // 3. ENFORCE / FAIL-CLOSED (executed, action-isolated): a token one action
        //    OFF (Write) at the SAME matching resource is denied BEFORE the provider
        //    — proving the denial is action-driven, and that only the exact
        //    previewed action passes (exact-equality gate, no action hierarchy).
        let wrong_token = encode_bridge_capability_token(&capability_manager.grant(
            "test-capsule",
            ResourceId::new(&rights_resource),
            Action::Write,
            TokenConstraints::default(),
            None,
        ));
        let deny_line = serde_json::json!({
            "id": 3,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://rights/access/has_access_by_content_id",
                "operation": "has_access_by_content_id",
                "token": wrong_token,
            }
        })
        .to_string();
        let denied = handle_request(&deny_line, &ctx).await.unwrap();
        assert_eq!(
            denied["response"]["code"], "capability_denied",
            "resource matches, so the denial is purely the action inequality (Write != Read): {denied}"
        );
    }

    // Conformance pin: for EVERY operation the shipped rights-provider manifest
    // declares, the PREVIEW action set (the real invoke::plan_provider_operation
    // derivation over the manifest authority) must equal the action the carrier
    // gate enforces (provider_resource::required_action_for). These are two
    // hand-written, independent tables with NO shared function (agreement is
    // by-convention today). This converts that convention into an enforced
    // invariant: add an op whose declared actions diverge from the verb map and
    // this fails LOUDLY. Only the ACTION dimension is pinned; the manifest's
    // elastos://rights/* resource and the per-op carrier resource diverge by
    // construction and are tracked as a separate gap.
    #[test]
    fn rights_fixture_preview_actions_match_verb_map() {
        use crate::provider_resource::required_action_for;
        use elastos_common::CapsuleManifest;
        use elastos_runtime::invoke::plan_provider_operation;

        let manifest: CapsuleManifest = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../capsules/rights-provider/capsule.json"
        )))
        .expect("rights-provider manifest parses");
        let authority = manifest
            .authority
            .expect("rights-provider declares provider authority");

        // Enumerate WHATEVER the fixture declares (not a hand-copied op list) so a
        // newly-added op is auto-covered by the pin.
        let mut ops: Vec<String> = authority
            .capabilities
            .iter()
            .flat_map(|cap| cap.operations.iter().cloned())
            .collect();
        ops.sort();
        ops.dedup();
        assert!(!ops.is_empty(), "rights fixture declares at least one op");

        for op in &ops {
            let plan = plan_provider_operation(&authority, op)
                .unwrap_or_else(|e| panic!("preview failed for op {op}: {e:?}"));
            // Full action SET (a union, never just [0]) so a future multi-action
            // block cannot pass by matching only the first element.
            let mut previewed: Vec<String> = plan.actions.iter().map(|a| a.to_string()).collect();
            previewed.sort();
            let enforced = vec![required_action_for(op).to_string()];
            assert_eq!(
                previewed, enforced,
                "op {op}: previewed gate {previewed:?} must equal the carrier-enforced action {enforced:?}"
            );
        }
    }

    /// G3b: UNIVERSAL preview==enforce — for EVERY shipped provider manifest, the
    /// previewed action set (`plan_provider_operation`, manifest authority) must
    /// equal the carrier-enforced action (`required_action_for`, verb map) for
    /// every declared op, OR the op is a KNOWN, tracked divergence. The two tables
    /// stay DISJOINT (no shared function), so drift fails loudly. A NEW divergence
    /// or a silently-fixed known one fails this test.
    #[test]
    fn all_provider_manifests_preview_actions_match_verb_map_or_tracked() {
        use crate::provider_resource::required_action_for;
        use elastos_common::CapsuleManifest;
        use elastos_runtime::invoke::plan_provider_operation;
        use std::collections::BTreeSet;

        // KNOWN preview≠enforce ledger: ops where the verb-map-enforced action is
        // NOT in the manifest's declared set for that op. ALL are fail-CLOSED today
        // (previewed-but-denied — the user is shown a weaker action than enforcement
        // demands), so none is an escalation. A 4-agent classification swarm
        // produced the PROVISIONAL triage below — the class comments are guidance
        // (confirm per-op at fix time); the (provider, op) PAIRS are authoritative
        // (the test enforces only those). Fix is per-op follow-up (G3b in
        // docs/KNOWN_GAPS.md). This set must SHRINK, never grow — a NEW drift fails
        // this test, and removing a fixed op without updating here also fails (so
        // the ledger cannot rot).
        let known_divergences: BTreeSet<(&str, &str)> = [
            // -- object-provider `share` STAYS: grants access — security-touching,
            //    held for a dedicated review (Miller).
            ("object-provider", "share"),
            // -- class C: EXECUTE / actuator. net/exit egress + browser-actuator
            //    drained (now declare `execute`); drm `open` remains (protected-
            //    content session — held for a content-protection review).
            ("drm-provider", "open"),
            // -- class E: HIGH-RISK (keys / signing / spend / secret export / decrypt);
            //    Miller's verdict: KEEP Admin — do NOT loosen without a dedicated review.
            ("wallet-provider", "approve_approval"),
            ("wallet-provider", "complete_approval"),
            ("wallet-provider", "export_managed_secret"),
            ("wallet-provider", "reject_approval"),
            ("wallet-provider", "sign_approved"),
            ("key-provider", "release"),
            ("decrypt-provider", "open_session"),
            ("decrypt-provider", "render"),
            ("encrypt-provider", "seal"),
            ("chain-provider", "broadcast_transaction"),
            ("chain-provider", "prepare_transaction"),
        ]
        .into_iter()
        .collect();

        let capsules_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");
        let mut parsed = 0usize;
        let mut found: BTreeSet<(String, String)> = BTreeSet::new();

        for entry in std::fs::read_dir(capsules_dir).expect("capsules dir exists") {
            let dir = entry.expect("dir entry").path();
            let manifest_path = dir.join("capsule.json");
            if !manifest_path.exists() {
                continue;
            }
            let provider = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let raw = std::fs::read_to_string(&manifest_path).expect("read manifest");
            // Skip non-provider or non-conforming manifests (no authority / parse).
            let manifest: CapsuleManifest = match serde_json::from_str(&raw) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let Some(authority) = manifest.authority else {
                continue;
            };
            parsed += 1;

            let mut ops: Vec<String> = authority
                .capabilities
                .iter()
                .flat_map(|cap| cap.operations.iter().cloned())
                .collect();
            ops.sort();
            ops.dedup();

            for op in &ops {
                let previewed: Vec<String> = match plan_provider_operation(&authority, op) {
                    Ok(plan) => {
                        let mut a: Vec<String> =
                            plan.actions.iter().map(|x| x.to_string()).collect();
                        a.sort();
                        a
                    }
                    Err(_) => {
                        found.insert((provider.clone(), op.clone()));
                        continue;
                    }
                };
                // The honesty invariant: the action the verb map ENFORCES for the
                // op must be among the actions the manifest DECLARES for it. A
                // multi-action capability block legitimately previews a union, so
                // membership (not set-equality) is the universal check; a true
                // drift is when enforcement requires an action the manifest never
                // granted for that op (e.g. a verb-map Admin fallthrough).
                let enforced = required_action_for(op).to_string();
                if !previewed.contains(&enforced) {
                    found.insert((provider.clone(), op.clone()));
                }
            }
        }

        assert!(
            parsed >= 10,
            "expected to cover many provider manifests, only parsed {parsed}"
        );

        let found_refs: BTreeSet<(&str, &str)> = found
            .iter()
            .map(|(p, o)| (p.as_str(), o.as_str()))
            .collect();
        let new_drift: Vec<_> = found_refs.difference(&known_divergences).collect();
        let healed: Vec<_> = known_divergences.difference(&found_refs).collect();
        assert!(
            new_drift.is_empty() && healed.is_empty(),
            "G3b preview!=enforce ledger out of date.\n  parsed manifests: {parsed}\n  NEW drift (add to known or fix the verb map): {new_drift:?}\n  HEALED (remove from known): {healed:?}\n  full found set: {found:?}"
        );
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

    // The FIVE-BEAT LOOP turning ONCE over the crown-jewel rights op
    // `has_access_by_content_id`, on ONE shared file-backed SIGNED audit chain:
    // PLAN (preview the gate from the manifest) -> CONSENT (the REAL request ->
    // grant_request records a fail-closed signed CapabilityApproved, then mints a
    // token) -> ACT (that real token clears the carrier gate and executes against a
    // mock rights provider, recording CapabilityUse) -> AUDIT (the on-disk signed
    // chain re-verifies; consent + act are present on the ring).
    //
    // Honesty scope: only the CONSENT beat is fail-closed (CapabilityApproved via
    // emit+?). CapabilityRequested / CapabilityGrant / CapabilityUse are best-effort
    // (they land + verify on a healthy sink; asserted by PRESENCE, not fail-closed).
    // Post-flip (G-ID): the token is minted at the requester's REAL capsule identity
    // (session.vm_id) and the carrier validates against the SAME session.vm_id, so
    // they agree by the canonical identity field -- the production path, not a test
    // artifice. The session-id shim is retired.
    #[tokio::test]
    async fn five_beat_loop_turns_once_on_one_signed_chain_over_a_real_consent_token() {
        use crate::api::handlers::capability::{
            grant_request, request_capability, CapabilityState, GrantRequestInput,
            RequestCapabilityInput,
        };
        use crate::inspect_provider::{CatalogInspectSource, InspectProvider, InspectSource};
        use crate::provider_resource::build_capability_resource;
        use axum::extract::State;
        use axum::{Extension, Json};
        use elastos_runtime::session::Session;

        let tmp = tempfile::tempdir().unwrap();
        let capsule_dir = tmp.path().join("capsules").join("rights-provider");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "rights-provider",
                "role": "provider", "type": "microvm", "entrypoint": "rootfs.ext4",
                "provides": "elastos://rights/*",
                "authority": {
                    "reason": "test",
                    "capabilities": [{
                        "resource": "elastos://rights/*",
                        "actions": ["read"],
                        "operations": ["has_access_by_content_id"]
                    }],
                    "audit_events": ["rights.status"]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        // ONE shared, file-backed SIGNED audit chain for the whole loop.
        let audit = Arc::new(
            elastos_runtime::primitives::audit::AuditLog::with_file(tmp.path().join("audit.log"))
                .unwrap(),
        );
        let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics = Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        // ONE shared manager (ONE signing key, so the minted token validates on the
        // same key the gate checks) + ONE shared pending store, both off one chain.
        let capability_manager = Arc::new(elastos_runtime::capability::CapabilityManager::new(
            store,
            audit.clone(),
            metrics,
        ));
        let pending_store =
            Arc::new(elastos_runtime::capability::pending::PendingRequestStore::new(audit.clone()));
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        let source: Arc<dyn InspectSource> = Arc::new(CatalogInspectSource::new(
            tmp.path().join("capsules"),
            Arc::downgrade(&registry),
        ));
        registry
            .register(Arc::new(InspectProvider::new(source)))
            .await;
        registry.register(Arc::new(RightsOkProvider)).await;

        // The CONSENT-layer handler state and the ACT-layer bridge share the SAME
        // manager + pending store + signed chain.
        let cap_state = CapabilityState {
            pending_store: pending_store.clone(),
            capability_manager: capability_manager.clone(),
            policy_evaluator: Arc::new(elastos_runtime::capability::PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit.clone(),
            )),
            standing_service: Arc::new(capability_manager.standing_grant_service()),
        };

        // ONE capsule session carrying its real identity (vm_id). Post-flip the mint
        // keys the token on session.vm_id and the carrier validates against the SAME
        // vm_id, so they agree by the real identity field, not a test artifice.
        let requester = Session::new_capsule("rights-app".to_string());
        let capsule_id = requester
            .vm_id
            .clone()
            .expect("a capsule session carries its vm_id");
        let bridge = Some(BridgeContext {
            provider_registry: registry,
            capability_manager: capability_manager.clone(),
            pending_store: pending_store.clone(),
            capsule_id: capsule_id.clone(),
            principal_id: None,
            data_dir: None,
            spend_policy: None,
        });

        // The capability resource the carrier enforces for this op; request + grant
        // at exactly this so the minted token validates at the gate.
        let rights_resource =
            build_capability_resource("rights", "has_access_by_content_id", &serde_json::json!({}))
                .expect("rights op resource builds");

        // BEAT 1 -- PLAN: the gate is previewed from the manifest (read-only, no row).
        let plan_token = encode_bridge_capability_token(&capability_manager.grant(
            &capsule_id,
            ResourceId::new("elastos://inspect/*"),
            Action::Read,
            TokenConstraints::default(),
            None,
        ));
        let plan_line = serde_json::json!({
            "id": 1,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://inspect/plan",
                "operation": "plan",
                "token": plan_token,
                "body": { "id": "capsule:rights-provider", "operation": "has_access_by_content_id" }
            }
        })
        .to_string();
        let preview = handle_request(&plan_line, &bridge).await.unwrap();
        assert_eq!(
            preview["response"]["result"]["data"]["capability_actions"],
            serde_json::json!(["read"]),
            "PLAN: the previewed gate is derived from the manifest: {preview}"
        );

        // BEAT 2 -- CONSENT: request -> the REAL grant_request records a signed,
        // fail-closed CapabilityApproved BEFORE minting the token.
        let req_out = request_capability(
            State(cap_state.clone()),
            Extension(requester.clone()),
            Json(RequestCapabilityInput {
                resource: rights_resource.clone(),
                action: "read".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("request accepted")
        .0;
        assert_eq!(req_out.status, "pending", "request is pending: {req_out:?}");
        let request_id = req_out.request_id.expect("a pending request id");

        let grant_out = grant_request(
            State(cap_state.clone()),
            Extension(requester.clone()),
            Json(GrantRequestInput {
                request_id,
                duration: "session".to_string(),
                rationale: Some("e2e shell approval".to_string()),
            }),
        )
        .await
        .expect("grant approved + attested")
        .0;
        assert!(grant_out.success, "consent granted: {grant_out:?}");
        let token = grant_out.token.expect("the consent path minted a token");

        // The PLAN dispatch above was itself a gated carrier call and already
        // recorded one CapabilityUse; capture that baseline so the ACT can be
        // attributed exactly one MORE use (a delta, not an absolute count).
        let uses_before_act = audit
            .recent_events_filtered(50, Some("capability_use"))
            .len();

        // BEAT 3 -- ACT: the REAL consent token clears the carrier gate for the op
        // and EXECUTES against the mock rights provider.
        let act_line = serde_json::json!({
            "id": 2,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://rights/access/has_access_by_content_id",
                "operation": "has_access_by_content_id",
                "token": token,
            }
        })
        .to_string();
        let acted = handle_request(&act_line, &bridge).await.unwrap();
        assert_eq!(acted["response"]["type"], "carrier_result");
        assert_eq!(
            acted["response"]["result"]["status"], "ok",
            "ACT: the real consent token clears the gate and reaches the provider: {acted}"
        );

        // BEAT 4 -- AUDIT, two distinct surfaces asserted SEPARATELY:
        // (a) the in-memory ring holds the consent + act decisions,
        let approved = audit.recent_events_filtered(50, Some("capability_approved"));
        assert_eq!(
            approved.len(),
            1,
            "CONSENT recorded exactly once (CapabilityApproved)"
        );
        assert_eq!(
            audit
                .recent_events_filtered(50, Some("capability_use"))
                .len(),
            uses_before_act + 1,
            "ACT recorded exactly one new CapabilityUse"
        );
        // (b) the on-disk SIGNED chain independently re-verifies every signature.
        use ed25519_dalek::VerifyingKey;
        let hex = audit.verifying_key_hex().expect("signed chain has a key");
        let bytes: [u8; 32] = hex::decode(&hex).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&bytes).unwrap();
        assert!(
            audit.verify_chain(Some(&vk)).is_ok(),
            "AUDIT: the on-disk signed chain re-verifies"
        );

        // Negative control: a token minted via the SAME real consent path but for
        // action Write (same session, same resource) is DENIED at the carrier for
        // the Read op -- and the denial is itself honestly attested as a FAILED
        // CapabilityUse (success=false), so the chain records refusals, not just
        // grants (no silent drop, no spurious success).
        let req2 = request_capability(
            State(cap_state.clone()),
            Extension(requester.clone()),
            Json(RequestCapabilityInput {
                resource: rights_resource.clone(),
                action: "write".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("write request accepted")
        .0;
        let grant2 = grant_request(
            State(cap_state.clone()),
            Extension(requester.clone()),
            Json(GrantRequestInput {
                request_id: req2.request_id.expect("write request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect("write grant attested")
        .0;
        let write_token = grant2.token.expect("write token minted");
        let denied_line = serde_json::json!({
            "id": 3,
            "request": {
                "type": "carrier_invoke",
                "uri": "elastos://rights/access/has_access_by_content_id",
                "operation": "has_access_by_content_id",
                "token": write_token,
            }
        })
        .to_string();
        let denied = handle_request(&denied_line, &bridge).await.unwrap();
        assert_eq!(
            denied["response"]["code"], "capability_denied",
            "a Write token is denied for the Read op (action-isolated): {denied}"
        );
        let has_failed_use = audit
            .recent_events_filtered(50, Some("capability_use"))
            .iter()
            .any(|e| {
                matches!(
                    e,
                    elastos_runtime::primitives::audit::AuditEvent::CapabilityUse {
                        success: false,
                        ..
                    }
                )
            });
        assert!(
            has_failed_use,
            "the denied act is attested as a failed CapabilityUse (success=false)"
        );
    }

    // --- BUG-6: bounded line reads (untrusted-guest OOM hardening) ---

    #[tokio::test]
    async fn read_bounded_line_reads_complete_lines_then_eof() {
        let data = b"{\"a\":1}\n{\"b\":2}\n";
        let mut reader = BufReader::new(&data[..]);
        assert!(
            matches!(read_bounded_line(&mut reader).await.unwrap(), BoundedLine::Line(l) if l == "{\"a\":1}")
        );
        assert!(
            matches!(read_bounded_line(&mut reader).await.unwrap(), BoundedLine::Line(l) if l == "{\"b\":2}")
        );
        assert!(matches!(
            read_bounded_line(&mut reader).await.unwrap(),
            BoundedLine::Eof
        ));
    }

    #[tokio::test]
    async fn read_bounded_line_strips_crlf() {
        let data = b"hello\r\n";
        let mut reader = BufReader::new(&data[..]);
        assert!(
            matches!(read_bounded_line(&mut reader).await.unwrap(), BoundedLine::Line(l) if l == "hello")
        );
    }

    /// THE bug: an oversized line with no newline must be rejected WITHOUT being
    /// fully buffered, and the stream must realign so the NEXT request parses.
    #[tokio::test]
    async fn read_bounded_line_bounds_oversized_then_realigns() {
        let mut data = vec![b'a'; MAX_LINE_BYTES + 4096]; // > cap, no newline yet
        data.push(b'\n'); // terminator of the oversized line
        data.extend_from_slice(b"{\"ok\":1}\n"); // the next, valid request
        let mut reader = BufReader::new(&data[..]);

        // 1) oversized line is reported TooLarge (and drained internally)
        assert!(matches!(
            read_bounded_line(&mut reader).await.unwrap(),
            BoundedLine::TooLarge
        ));
        // 2) the stream realigned — the following request reads cleanly
        assert!(
            matches!(read_bounded_line(&mut reader).await.unwrap(), BoundedLine::Line(l) if l == "{\"ok\":1}")
        );
        assert!(matches!(
            read_bounded_line(&mut reader).await.unwrap(),
            BoundedLine::Eof
        ));
    }

    /// An oversized line that never terminates (EOF mid-flood) is TooLarge, then
    /// the drain hits EOF cleanly — no infinite loop, no unbounded buffer.
    #[tokio::test]
    async fn read_bounded_line_oversized_at_eof() {
        let data = vec![b'a'; MAX_LINE_BYTES + 4096]; // no newline, then EOF
        let mut reader = BufReader::new(&data[..]);
        assert!(matches!(
            read_bounded_line(&mut reader).await.unwrap(),
            BoundedLine::TooLarge
        ));
        assert!(matches!(
            read_bounded_line(&mut reader).await.unwrap(),
            BoundedLine::Eof
        ));
    }

    /// The synchronous twin enforces the same bound + realign over `BufRead`.
    #[test]
    fn read_bounded_line_sync_bounds_oversized_then_realigns() {
        let mut data = vec![b'a'; MAX_LINE_BYTES + 4096];
        data.push(b'\n');
        data.extend_from_slice(b"{\"ok\":1}\n");
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(data));

        assert!(matches!(
            read_bounded_line_sync(&mut reader).unwrap(),
            BoundedLine::TooLarge
        ));
        assert!(
            matches!(read_bounded_line_sync(&mut reader).unwrap(), BoundedLine::Line(l) if l == "{\"ok\":1}")
        );
        assert!(matches!(
            read_bounded_line_sync(&mut reader).unwrap(),
            BoundedLine::Eof
        ));
    }

    // --- BUG-5: a late-landing grant is not dropped to a timeout ---

    /// THE bug: a grant that lands AFTER the poll loop's last iteration must still
    /// be returned. With `max_polls = 0` the loop body never runs, so ONLY the
    /// post-loop final read can find the grant — proving the trailing read closes
    /// the window the old code dropped.
    #[tokio::test]
    async fn await_capability_decision_catches_a_grant_after_the_loop() {
        use elastos_runtime::capability::pending::GrantDuration;
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::session::SessionId;

        let store = PendingRequestStore::new(Arc::new(AuditLog::new()));
        let req = store
            .create_request(
                SessionId("vm-test".to_string()),
                ResourceId::new("elastos://peer/*"),
                Action::Execute,
            )
            .await;
        let request_id = req.id.to_string();

        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let token = CapabilityToken::new(
            "vm-test".to_string(),
            signing_key.verifying_key().to_bytes(),
            ResourceId::new("elastos://peer/*"),
            Action::Execute,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );
        store
            .grant_request(&request_id, token, GrantDuration::Once)
            .await
            .expect("grant should succeed");

        // max_polls = 0 → the loop never polls; only the trailing read can catch it.
        let decision = await_capability_decision(&store, &request_id, 0, 1).await;
        assert!(
            matches!(decision, CapabilityDecision::Granted(_)),
            "the grant landing after the loop is returned, not dropped (BUG-5)"
        );
    }

    /// An unresolved request times out cleanly — the loop polls, finds nothing,
    /// the final read finds nothing → TimedOut (no false grant from the new read).
    #[tokio::test]
    async fn await_capability_decision_times_out_when_unresolved() {
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::session::SessionId;

        let store = PendingRequestStore::new(Arc::new(AuditLog::new()));
        let req = store
            .create_request(
                SessionId("vm-test".to_string()),
                ResourceId::new("elastos://peer/*"),
                Action::Execute,
            )
            .await;
        let decision = await_capability_decision(&store, &req.id.to_string(), 2, 1).await;
        assert!(matches!(decision, CapabilityDecision::TimedOut));
    }

    /// BUG-5 (HTTP twin): `poll_then_final_read` must catch a decision that lands
    /// AFTER the loop. With `max_polls = 0` the loop never runs, so ONLY the
    /// trailing read can return it — the exact gap the WASM-API HTTP poll had.
    #[tokio::test]
    async fn poll_then_final_read_catches_a_decision_after_the_loop() {
        use std::cell::Cell;

        let calls = Cell::new(0);
        let out: Option<&str> = poll_then_final_read(0, 1, || {
            calls.set(calls.get() + 1);
            async { Ok(Some("granted")) }
        })
        .await
        .unwrap();
        assert_eq!(
            out,
            Some("granted"),
            "the trailing read returns the decision"
        );
        assert_eq!(
            calls.get(),
            1,
            "exactly the one trailing poll ran (loop was skipped)"
        );

        // A poll that stays pending times out to None after the trailing read.
        let pending: Option<&str> = poll_then_final_read(2, 1, || async { Ok(None) })
            .await
            .unwrap();
        assert!(pending.is_none());
    }
}
