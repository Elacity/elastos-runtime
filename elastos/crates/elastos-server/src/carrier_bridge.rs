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

/// Maximum byte length of a single Carrier-bridge framed line. Lines
/// longer than this are dropped with a `request_too_large` error
/// envelope written back to the guest. **Phase 10 Day 4-8**: hoisted
/// from the two inline copies (microVM bridge loop + WASM bridge loop)
/// into a single public constant so the fuzz harness and the bridge
/// loops cannot drift.
pub const CARRIER_MAX_LINE_BYTES: usize = 1_048_576;

/// Typed parser result for the fuzz harness. The production bridge
/// loops keep using `anyhow::Result` for source-line continuity; this
/// enum exists so fuzz can distinguish a rejection-class from a true
/// panic-class finding.
#[derive(Debug)]
pub enum CarrierFrameError {
    /// Raw byte length exceeded `CARRIER_MAX_LINE_BYTES` before any
    /// JSON parse was attempted. Production bridge writes back
    /// `{"id":0,"type":"error","error":"request_too_large"}` and
    /// continues; fuzz treats this as a clean rejection.
    LineTooLarge { len: usize },
    /// Raw bytes were not valid UTF-8. Production `read_line` produces
    /// a `String` so this branch isn't reachable on the live host
    /// path; surfaced here for fuzz coverage of the conversion.
    InvalidUtf8(std::str::Utf8Error),
    /// `serde_json::from_str` rejected the trimmed line.
    InvalidJson(serde_json::Error),
}

/// **Phase 10 Day 4-8 — trust-boundary framing parser surface.**
///
/// Pure function that mirrors the framing + JSON-parse logic embedded
/// in [`run_carrier_bridge_loop`] and [`spawn_wasm_carrier_bridge`].
/// Exists solely so the fuzz harness at `fuzz/fuzz_targets/
/// carrier_bridge_framing.rs` can exercise the parser with arbitrary
/// bytes without spinning up an async runtime, a Unix socket, or a
/// `BridgeContext`.
///
/// Semantics, in order:
/// 1. If `bytes.len() > CARRIER_MAX_LINE_BYTES`: return
///    `Err(LineTooLarge)` immediately — no JSON parse attempted.
/// 2. If `bytes` is not valid UTF-8: return `Err(InvalidUtf8)`.
/// 3. Trim leading/trailing ASCII whitespace per `str::trim`.
/// 4. If the trimmed line is empty: return `Ok(None)` (production
///    bridges skip these via `if line.trim().is_empty() { continue; }`).
/// 5. Otherwise `serde_json::from_str(trimmed)` — on failure return
///    `Err(InvalidJson)`, on success return `Ok(Some(value))`.
///
/// **Invariants the fuzz harness asserts on this function:**
/// - It never panics.
/// - For inputs longer than `CARRIER_MAX_LINE_BYTES` the function
///   short-circuits and returns `Err(LineTooLarge)` without
///   allocating proportional to input size.
/// - The function is total: every byte slice produces either `Ok(_)`
///   or `Err(_)`.
pub fn parse_carrier_line(bytes: &[u8]) -> Result<Option<serde_json::Value>, CarrierFrameError> {
    if bytes.len() > CARRIER_MAX_LINE_BYTES {
        return Err(CarrierFrameError::LineTooLarge { len: bytes.len() });
    }
    let s = std::str::from_utf8(bytes).map_err(CarrierFrameError::InvalidUtf8)?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(CarrierFrameError::InvalidJson)?;
    Ok(Some(value))
}

/// **Phase 10.5 M1 — byte-budgeted line reader.**
///
/// Reads bytes from `reader` into `buf` until either a newline
/// is consumed (inclusive, matching [`tokio::io::AsyncBufReadExt::read_line`]
/// shape) or `max_bytes` bytes have been buffered, whichever
/// comes first. Returns the number of bytes pushed onto `buf`.
///
/// **Why it exists:** `BufReader::read_line` is unbounded. A
/// guest writing `b"A" * 10_000_000_000` without a `\n` would
/// grow the receiving `String` until the host runs out of
/// memory — the post-read length check in
/// [`run_carrier_bridge_loop`] fires *after* the allocation has
/// already happened, which is too late.
///
/// **Calling convention:** callers pass `CARRIER_MAX_LINE_BYTES
/// + 1` so the post-read check `n > CARRIER_MAX_LINE_BYTES`
/// fires without truncating an attacker-supplied payload
/// mid-byte. This `+1` headroom convention is the contract
/// between this helper and the bridge loop's oversized-line
/// handler.
///
/// **Semantics:**
/// - On EOF before any byte: returns `Ok(0)`.
/// - On newline within `max_bytes`: returns `Ok(n)` with the
///   newline included in `buf`.
/// - On reaching `max_bytes` without a newline: returns
///   `Ok(max_bytes)`; the caller must call [`drain_to_newline`]
///   to resync the stream to the start of the next line.
///
/// Memory footprint: bounded by `max_bytes` (size of `buf` on
/// successful return) plus the inner `BufReader`'s 8 KiB
/// internal scratch — constant in the size of attacker input.
async fn read_line_byte_budgeted<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut total = 0usize;
    loop {
        let (consumed, found_newline) = {
            let chunk = reader.fill_buf().await?;
            if chunk.is_empty() {
                return Ok(total);
            }
            let remaining = max_bytes.saturating_sub(total);
            let take = chunk.len().min(remaining);
            let scan = &chunk[..take];
            if let Some(pos) = scan.iter().position(|&b| b == b'\n') {
                buf.extend_from_slice(&scan[..=pos]);
                (pos + 1, true)
            } else {
                buf.extend_from_slice(scan);
                (take, false)
            }
        };
        reader.consume(consumed);
        total += consumed;
        if found_newline {
            return Ok(total);
        }
        if total >= max_bytes {
            return Ok(total);
        }
    }
}

/// **Phase 10.5 M1 — resync helper.**
///
/// Discard bytes from `reader` until the next `\n` is consumed
/// or EOF. Memory footprint is O(internal `BufReader` buffer
/// size) — bytes are scanned then consumed, never accumulated.
///
/// Called by [`run_carrier_bridge_loop`] after an oversized
/// line is detected, so the next iteration starts on a clean
/// line boundary. If the producer never sends a newline, the
/// function loops until the underlying stream closes (EOF);
/// memory remains O(1) the entire time.
async fn drain_to_newline<R>(reader: &mut R) -> std::io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        let (consumed, found_newline) = {
            let chunk = reader.fill_buf().await?;
            if chunk.is_empty() {
                return Ok(());
            }
            if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                (pos + 1, true)
            } else {
                (chunk.len(), false)
            }
        };
        reader.consume(consumed);
        if found_newline {
            return Ok(());
        }
    }
}

/// Resources needed by the bridge to handle requests.
#[derive(Clone)]
pub struct BridgeContext {
    pub provider_registry: Arc<ProviderRegistry>,
    pub capability_manager: Arc<CapabilityManager>,
    pub pending_store: Arc<elastos_runtime::capability::pending::PendingRequestStore>,
    /// Capsule identity for token minting (session ID or capsule name)
    pub capsule_id: String,
    /// Optional bridge-termination observer. **Phase 4 Day 6.**
    ///
    /// When set, [`run_carrier_bridge_loop`] calls
    /// `notify.notify_waiters()` on EVERY exit path (EOF, read
    /// error, write error, oversized-line teardown) before
    /// returning. Lets the supervisor's `stop_capsule` await
    /// the bridge's natural termination after `vm.stop()`
    /// resolves, rather than relying on send-and-pray
    /// observability.
    ///
    /// `None` for legacy callers (Linux crosvm bridges,
    /// WASM-stdio bridges) where lifecycle observation is not
    /// wired today.
    pub on_terminate: Option<Arc<tokio::sync::Notify>>,
}

/// Spawn a Carrier bridge handler for a microVM capsule on a
/// **path** (Linux / crosvm flow).
///
/// Binds a Unix listener that crosvm's `--serial type=unix-stream`
/// connects to at VM start, then hands the accepted stream into
/// the shared bridge loop. Must be called BEFORE starting the
/// VM so the socket exists when crosvm launches.
///
/// macOS / Vz capsules use [`spawn_carrier_bridge_on_stream`]
/// instead, because the host endpoint comes directly from a
/// `socketpair(AF_UNIX, SOCK_STREAM)` carrier-console attachment
/// — there is no listener to bind. The shared bridge loop is
/// the same.
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
        run_carrier_bridge_loop(stream, bridge_ctx, socket_display).await;
    });

    Ok(())
}

/// Spawn a Carrier bridge handler on an **already-connected**
/// `tokio::net::UnixStream` — **Phase 3 Day 4** entry point for
/// the macOS / Vz flow.
///
/// On Mac, the host endpoint of the Carrier console is the
/// host-side fd of a `socketpair(AF_UNIX, SOCK_STREAM)` set up
/// by `elastos-vz::ffi::console::build_carrier_console_slot`.
/// The supervisor takes that fd via `RunningVm::take_carrier_host_fd`,
/// converts it to a `tokio::net::UnixStream`, and hands it to
/// this function — no listener / bind / accept needed.
///
/// The bridge dispatch loop is byte-identical to
/// [`spawn_carrier_bridge`]; only the connection-acquisition
/// half differs.
pub fn spawn_carrier_bridge_on_stream(
    stream: tokio::net::UnixStream,
    _provider_registry: Arc<ProviderRegistry>,
    _session_token: String,
    bridge_ctx: Option<BridgeContext>,
    label: String,
) {
    tokio::spawn(async move {
        tracing::info!(
            "Carrier microVM bridge: pre-connected stream attached for {}",
            label
        );
        run_carrier_bridge_loop(stream, bridge_ctx, label).await;
    });
}

/// Shared per-connection bridge dispatch loop. Reads newline
/// delimited JSON `RequestEnvelope`s off the supplied stream,
/// dispatches them through `bridge_ctx` providers, and writes
/// `ResponseEnvelope`s back on the same stream.
///
/// `label` is a human-readable identifier used in trace log
/// lines (socket path on Linux, capsule handle on Mac).
async fn run_carrier_bridge_loop(
    stream: tokio::net::UnixStream,
    ctx: Option<BridgeContext>,
    label: String,
) {
    // Phase 4 Day 6: stash the termination observer up front
    // so every exit path can fire it without re-matching on
    // `ctx`.
    let on_terminate = ctx.as_ref().and_then(|c| c.on_terminate.clone());

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Phase 10.5 M1: the framed-line read is byte-budgeted at
    // `CARRIER_MAX_LINE_BYTES + 1`. Pre-Phase-10.5 this was
    // `read_line(&mut line)`, which is unbounded — a guest could
    // grow the host's `String` to multi-GiB before the post-read
    // length check fired (which was too late by then). The new
    // helper allocates at most `CARRIER_MAX_LINE_BYTES + 1`
    // bytes; the `+1` headroom lets the post-read check below
    // distinguish "exactly at limit" from "over limit" without
    // truncating attacker-supplied payloads mid-byte.
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut line = String::new();
    loop {
        buf.clear();
        line.clear();
        let n = match read_line_byte_budgeted(&mut reader, &mut buf, CARRIER_MAX_LINE_BYTES + 1)
            .await
        {
            Ok(0) => break, // EOF — guest shut down
            Ok(n) => n,
            Err(e) => {
                tracing::debug!("Carrier bridge read error: {}", e);
                break;
            }
        };

        if n > CARRIER_MAX_LINE_BYTES {
            tracing::warn!(
                "Carrier bridge: oversized line ({} bytes, cap {}), dropping and resyncing",
                n,
                CARRIER_MAX_LINE_BYTES
            );
            // Resync the stream to the next line boundary so a
            // subsequent well-formed request is dispatched
            // cleanly. `drain_to_newline` is O(1) memory.
            if let Err(e) = drain_to_newline(&mut reader).await {
                tracing::debug!("Carrier bridge drain-after-overflow error: {}", e);
                break;
            }
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

        // Convert to UTF-8. `read_line` previously enforced this
        // implicitly via its `&mut String` signature; we replicate
        // the behaviour explicitly so a non-UTF-8 framed line
        // does not break the loop — log and continue, matching
        // the pre-Phase-10.5 read-error path semantics.
        match std::str::from_utf8(&buf) {
            Ok(s) => line.push_str(s),
            Err(e) => {
                tracing::warn!(
                    "Carrier bridge: framed line is not valid UTF-8: {} (dropping)",
                    e
                );
                continue;
            }
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
    tracing::info!("Carrier bridge closed for {}", label);

    // Phase 4 Day 6: fire the termination observer (if wired)
    // on EVERY loop exit so the supervisor's `stop_capsule`
    // can await natural bridge teardown deterministically.
    // `notify_waiters()` is fire-and-forget — if no one is
    // listening, the signal is dropped (the post-stop poll
    // uses a bounded `tokio::time::timeout` either way, so a
    // missed signal degrades to a best-effort warn).
    if let Some(notify) = on_terminate {
        notify.notify_waiters();
    }
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

                if line.len() > CARRIER_MAX_LINE_BYTES {
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

    /// Phase 3 Day 4: prove the bridge dispatch loop can be
    /// driven by a pre-connected `tokio::net::UnixStream` (the
    /// Mac flow), not just an `accept()`-derived one (the Linux
    /// flow). Sends a `ping` request through one half of a
    /// socketpair, expects a `pong` response on the other.
    ///
    /// `ctx = None` is the worst-case bridge state (no provider
    /// registry, no capability manager) — only the built-in
    /// handlers (ping / get_runtime_info) work in that mode,
    /// which is exactly what this test exercises. A pong proves
    /// the per-stream dispatch loop is wired correctly.
    #[tokio::test]
    async fn spawn_carrier_bridge_on_stream_handles_ping_pong_over_socketpair() {
        use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        // Build a connected pair of fds — same shape as the Vz
        // carrier console socketpair.
        let mut sv = [0i32; 2];
        let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) };
        assert_eq!(rc, 0, "socketpair must succeed for the test fixture");

        // Set both ends non-blocking so tokio is happy.
        for fd in sv {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
            assert_eq!(rc, 0);
        }

        let host_fd = unsafe { OwnedFd::from_raw_fd(sv[0]) };
        let test_fd = unsafe { OwnedFd::from_raw_fd(sv[1]) };

        // Convert each end to a `tokio::net::UnixStream`.
        let host_stream = tokio::net::UnixStream::from_std(unsafe {
            std::os::unix::net::UnixStream::from_raw_fd(host_fd.into_raw_fd())
        })
        .expect("host-side tokio UnixStream from_std");
        let mut test_stream = tokio::net::UnixStream::from_std(unsafe {
            std::os::unix::net::UnixStream::from_raw_fd(test_fd.into_raw_fd())
        })
        .expect("test-side tokio UnixStream from_std");

        // We need a non-empty `ProviderRegistry` clone for the
        // signature, but `ctx: None` means the dispatch loop
        // never touches it. Use `Arc::new(ProviderRegistry::new())`.
        let registry = Arc::new(ProviderRegistry::new());

        spawn_carrier_bridge_on_stream(
            host_stream,
            registry,
            String::new(),
            None,
            "test:phase3-day4-pingpong".to_string(),
        );

        // Send a ping line on the test side.
        let ping = b"{\"id\":42,\"request\":{\"type\":\"ping\"}}\n";
        test_stream
            .write_all(ping)
            .await
            .expect("write ping to socketpair");
        test_stream.flush().await.expect("flush ping to socketpair");

        // Read the pong response back.
        let (reader, _writer) = test_stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut response_line = String::new();
        // Allow a generous timeout — the bridge dispatch loop
        // runs in a separate task and the kernel may briefly
        // buffer.
        let read_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_line(&mut response_line),
        )
        .await;
        assert!(
            read_result.is_ok(),
            "bridge must respond within 2s; got timeout"
        );
        assert!(
            read_result.unwrap().is_ok(),
            "read_line must succeed; got error"
        );

        // The bridge wraps the inner response in `{"id":42,"response":{...}}`.
        let parsed: serde_json::Value =
            serde_json::from_str(&response_line).expect("response is JSON");
        assert_eq!(parsed["id"], 42);
        assert_eq!(
            parsed["response"]["type"], "pong",
            "expected a pong response from the ping request, got: {response_line}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 4 Day 2 — Carrier-bridge multiplex audit.
    //
    // Production reality: every microVM gets its own carrier
    // socketpair and the bridge dispatch loop is detached via
    // `tokio::spawn` — the `JoinHandle` is intentionally
    // discarded. Bridge termination is socket-driven: dropping
    // the guest endpoint (which the supervisor does by dropping
    // the `RunningCapsule`'s `carrier_host_fd`) causes the next
    // `read_line` to return `Ok(0)` (EOF) and the loop to break
    // cleanly.
    //
    // The audit question is: can N bridges run side-by-side
    // sharing the same `Arc<ProviderRegistry>` without
    // cross-talk? The tests below build three independent
    // socketpairs, attach three bridges to one registry, and
    // assert per-bridge isolation in both the steady-state
    // (ping/pong round-tripping on each) and shutdown
    // (dropping one guest endpoint terminates only that
    // bridge) cases.
    // ---------------------------------------------------------------

    /// Helper: build a non-blocking socketpair and return both
    /// halves as `tokio::net::UnixStream`s.
    #[cfg(target_os = "macos")]
    fn build_socketpair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

        let mut sv = [0i32; 2];
        let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) };
        assert_eq!(rc, 0, "socketpair must succeed");

        for fd in sv {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
            assert_eq!(rc, 0);
        }

        let a = unsafe { OwnedFd::from_raw_fd(sv[0]) };
        let b = unsafe { OwnedFd::from_raw_fd(sv[1]) };

        let host = tokio::net::UnixStream::from_std(unsafe {
            std::os::unix::net::UnixStream::from_raw_fd(a.into_raw_fd())
        })
        .expect("host-side UnixStream::from_std");
        let test = tokio::net::UnixStream::from_std(unsafe {
            std::os::unix::net::UnixStream::from_raw_fd(b.into_raw_fd())
        })
        .expect("test-side UnixStream::from_std");
        (host, test)
    }

    /// Helper: drive a ping/pong round-trip on a test-side
    /// stream and parse the JSON response. Returns `None` if
    /// the bridge does not respond within 2s — surface the
    /// timeout as a `None` so callers can distinguish "bridge
    /// alive but slow" (rare) from "bridge dead" (expected
    /// after shutdown).
    #[cfg(target_os = "macos")]
    async fn ping_bridge(
        stream: &mut tokio::net::UnixStream,
        request_id: u64,
    ) -> Option<serde_json::Value> {
        let req = format!("{{\"id\":{request_id},\"request\":{{\"type\":\"ping\"}}}}\n");
        stream.write_all(req.as_bytes()).await.expect("write ping");
        stream.flush().await.expect("flush ping");

        let mut buf = [0u8; 4096];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::io::AsyncReadExt::read(stream, &mut buf),
        )
        .await
        .ok()?
        .ok()?;
        if read == 0 {
            return None;
        }
        // The bridge writes one line per response; the kernel
        // may briefly buffer but `read` returns when any bytes
        // are available. We trust the bridge wrote a complete
        // line (the implementation does `write_all` + `flush`).
        let line = std::str::from_utf8(&buf[..read]).ok()?;
        let trimmed = line.trim_end();
        serde_json::from_str(trimmed).ok()
    }

    /// Three concurrent bridges sharing ONE `ProviderRegistry`
    /// must each respond to its own ping with a pong, without
    /// any cross-VM message contamination. Proves the detached-
    /// spawn model has per-bridge isolation under N>1.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_concurrent_carrier_bridges_isolate_per_capsule() {
        let registry = Arc::new(ProviderRegistry::new());

        let mut clients: Vec<tokio::net::UnixStream> = Vec::with_capacity(3);
        let labels = ["alpha", "bravo", "charlie"];
        for label in labels.iter() {
            let (host, test) = build_socketpair();
            spawn_carrier_bridge_on_stream(
                host,
                registry.clone(),
                String::new(),
                None,
                format!("test:phase4-day2-multiplex-{label}"),
            );
            clients.push(test);
        }

        // Issue one ping per bridge, identified by a distinct
        // request id. Each bridge must echo back its OWN id —
        // never another bridge's.
        for (idx, stream) in clients.iter_mut().enumerate() {
            let request_id = 1000 + idx as u64;
            let response = ping_bridge(stream, request_id)
                .await
                .unwrap_or_else(|| panic!("bridge {idx} ({}) failed to respond", labels[idx]));
            assert_eq!(
                response["id"],
                serde_json::Value::from(request_id),
                "bridge {idx} ({}) must echo its OWN request id; got: {response}",
                labels[idx]
            );
            assert_eq!(
                response["response"]["type"], "pong",
                "bridge {idx} ({}) must respond with pong; got: {response}",
                labels[idx]
            );
        }
    }

    /// Dropping ONE bridge's guest endpoint must terminate
    /// ONLY that bridge's dispatch loop (the next read EOFs,
    /// the loop breaks). The other two bridges keep serving
    /// requests on the same shared `ProviderRegistry`. Proves
    /// the "supervisor drops `RunningCapsule.carrier_host_fd`,
    /// bridge dies" contract holds under N>1.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_one_carrier_endpoint_terminates_only_that_bridge() {
        let registry = Arc::new(ProviderRegistry::new());

        let (host_a, mut client_a) = build_socketpair();
        let (host_b, client_b) = build_socketpair();
        let (host_c, mut client_c) = build_socketpair();

        spawn_carrier_bridge_on_stream(
            host_a,
            registry.clone(),
            String::new(),
            None,
            "test:phase4-day2-shutdown-alpha".into(),
        );
        spawn_carrier_bridge_on_stream(
            host_b,
            registry.clone(),
            String::new(),
            None,
            "test:phase4-day2-shutdown-bravo".into(),
        );
        spawn_carrier_bridge_on_stream(
            host_c,
            registry.clone(),
            String::new(),
            None,
            "test:phase4-day2-shutdown-charlie".into(),
        );

        // Sanity: alpha + charlie alive and responding.
        let resp = ping_bridge(&mut client_a, 2001)
            .await
            .expect("alpha pre-shutdown ping");
        assert_eq!(resp["id"], serde_json::Value::from(2001u64));
        let resp = ping_bridge(&mut client_c, 2003)
            .await
            .expect("charlie pre-shutdown ping");
        assert_eq!(resp["id"], serde_json::Value::from(2003u64));

        // Drop bravo's guest endpoint — its bridge's next
        // read_line returns Ok(0) (EOF) and the loop breaks.
        drop(client_b);

        // Give Tokio one yield to let the EOF propagate
        // through the (idle) bravo bridge. The other two are
        // unaffected.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Alpha + charlie must still serve a fresh ping.
        let resp = ping_bridge(&mut client_a, 2101)
            .await
            .expect("alpha post-bravo-shutdown ping must succeed");
        assert_eq!(resp["id"], serde_json::Value::from(2101u64));
        assert_eq!(resp["response"]["type"], "pong");

        let resp = ping_bridge(&mut client_c, 2103)
            .await
            .expect("charlie post-bravo-shutdown ping must succeed");
        assert_eq!(resp["id"], serde_json::Value::from(2103u64));
        assert_eq!(resp["response"]["type"], "pong");
    }

    // ---------------------------------------------------------------
    // Phase 10.5 M3 — JSON nesting-depth resilience verification.
    //
    // `serde_json` 1.0.149 documents a default 128-deep recursion
    // limit on `from_str` / `Deserializer`, so deeply-nested input
    // should `Err(RecursionLimitExceeded)` rather than overflow
    // the stack. The pre-review packet flagged that this had not
    // been verified empirically on our actual call path
    // (`parse_carrier_line` → `serde_json::from_str::<Value>`).
    //
    // These tests verify the guarantee in normal CI:
    //   1. A 200-deep nested array (well past 128) is rejected as
    //      `Err(CarrierFrameError::InvalidJson(_))`, not a panic.
    //   2. A 50-deep nested array (well under 128) is accepted as
    //      `Ok(Some(_))` (proves the cap is not over-eager).
    //
    // The fuzz corpus also gets two new seeds — `26-nested-129-deep`
    // and `27-envelope-nested-200-deep` — so subsequent libfuzzer
    // runs exercise both the bare-Value path and the typed
    // RequestEnvelope path.
    //
    // If serde_json's default ever changes upstream (or a transitive
    // feature flag disables the limit), this test fires with a
    // `STATUS_STACK_BUFFER_OVERRUN` / `SIGSEGV` and we escalate to
    // an explicit `Deserializer::with_recursion_limit(128)` wrapper.
    // ---------------------------------------------------------------

    /// 200-deep nested array (`[[[...]]]`) must be rejected as
    /// invalid JSON, not overflow the stack.
    #[test]
    fn parse_carrier_line_rejects_excessively_nested_json() {
        let depth = 200usize;
        let mut payload = String::with_capacity(2 * depth);
        for _ in 0..depth {
            payload.push('[');
        }
        for _ in 0..depth {
            payload.push(']');
        }
        let result = parse_carrier_line(payload.as_bytes());
        match result {
            Err(CarrierFrameError::InvalidJson(_)) => { /* expected */ }
            Err(other) => panic!("expected InvalidJson rejection, got: {other:?}"),
            Ok(value) => panic!("expected InvalidJson rejection, got Ok: {value:?}"),
        }
    }

    /// 50-deep nested array must be accepted — proves the depth
    /// cap is not over-eager. Pre-fix: this also passed, but if
    /// we ever lower the limit aggressively this catches the
    /// regression.
    #[test]
    fn parse_carrier_line_accepts_moderately_nested_json() {
        let depth = 50usize;
        let mut payload = String::with_capacity(2 * depth);
        for _ in 0..depth {
            payload.push('[');
        }
        for _ in 0..depth {
            payload.push(']');
        }
        let result = parse_carrier_line(payload.as_bytes());
        match result {
            Ok(Some(serde_json::Value::Array(_))) => { /* expected */ }
            other => panic!("expected Ok(Some(Array(...))) for 50-deep input, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Phase 10.5 M1 — bounded read regression test.
    //
    // Pre-Phase-10.5 the bridge loop called
    // `reader.read_line(&mut line).await` with no upper bound, so a
    // guest writing N bytes without a `\n` would grow the host's
    // `String` to N bytes before the post-read length check fired.
    // The fix replaces `read_line` with `read_line_byte_budgeted`
    // which caps the allocation at `CARRIER_MAX_LINE_BYTES + 1`.
    //
    // This test exercises the path end-to-end:
    //   1. Send 2 × CARRIER_MAX_LINE_BYTES bytes of 'A' with no
    //      newline. Pre-fix, this would either OOM the test
    //      process or, with the post-read check, block waiting for
    //      a newline that never came.
    //   2. Append a trailing `\n` + a well-formed `ping` request +
    //      another `\n`.
    //   3. Assert: the bridge writes back a `request_too_large`
    //      envelope (proves the cap fired) followed by a `pong`
    //      response (proves the loop drained to the next newline
    //      and resumed normal dispatch — no bridge teardown).
    //
    // If the unbounded-read regression ever returns, this test
    // will either time out (bridge stuck in `read_line` waiting
    // for `\n`) or trigger an OOM kill in CI.
    // ---------------------------------------------------------------
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_line_resyncs_and_continues_dispatch() {
        let registry = Arc::new(ProviderRegistry::new());
        let (host, mut client) = build_socketpair();

        spawn_carrier_bridge_on_stream(
            host,
            registry,
            String::new(),
            None,
            "test:phase10.5-m1-oversized".into(),
        );

        // 2 MiB of 'A' with no newline — pre-fix this would grow
        // the host `String` to 2 MiB before the length check
        // tripped (the cap is 1 MiB).
        let oversized = vec![b'A'; 2 * CARRIER_MAX_LINE_BYTES];
        client
            .write_all(&oversized)
            .await
            .expect("write oversized burst to bridge");
        // Newline to close out the oversized framed line so the
        // drain can resync without waiting for the rest of the
        // (never-coming) attacker payload.
        client.write_all(b"\n").await.expect("write closing \\n");
        // Then a clean ping to prove the bridge is still alive
        // and dispatching after the overflow event.
        let ping = b"{\"id\":91005,\"request\":{\"type\":\"ping\"}}\n";
        client
            .write_all(ping)
            .await
            .expect("write follow-up ping after overflow");
        client.flush().await.expect("flush after overflow + ping");

        // The bridge writes two responses back-to-back:
        //   1. `request_too_large` (for the oversized line)
        //   2. `pong` envelope echoing id=91005 (for the follow-up)
        //
        // Read them as raw bytes and split on `\n` — they may
        // arrive in one or two `read` chunks depending on kernel
        // buffering.
        let mut buf = [0u8; 8192];
        let mut accumulated = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!(
                    "bridge did not produce both responses within deadline; got: {}",
                    String::from_utf8_lossy(&accumulated)
                );
            }
            let read = tokio::time::timeout(
                remaining,
                tokio::io::AsyncReadExt::read(&mut client, &mut buf),
            )
            .await;
            match read {
                Ok(Ok(0)) => panic!(
                    "bridge closed early; partial accumulator: {}",
                    String::from_utf8_lossy(&accumulated)
                ),
                Ok(Ok(n)) => {
                    accumulated.extend_from_slice(&buf[..n]);
                    // Two `\n`-terminated frames means both
                    // responses have arrived.
                    if accumulated.iter().filter(|&&b| b == b'\n').count() >= 2 {
                        break;
                    }
                }
                Ok(Err(e)) => panic!("read from bridge failed: {e}"),
                Err(_) => panic!(
                    "deadline hit waiting for second response; accumulator: {}",
                    String::from_utf8_lossy(&accumulated)
                ),
            }
        }

        let text = String::from_utf8(accumulated).expect("bridge responses are UTF-8");
        let mut lines = text.lines();

        let first = lines.next().expect("first response line present");
        let first_json: serde_json::Value =
            serde_json::from_str(first).expect("first response is JSON");
        assert_eq!(
            first_json["type"], "error",
            "first response must be the error envelope; got: {first}"
        );
        assert_eq!(
            first_json["error"], "request_too_large",
            "first response must be `request_too_large`; got: {first}"
        );

        let second = lines.next().expect("second response line present");
        let second_json: serde_json::Value =
            serde_json::from_str(second).expect("second response is JSON");
        assert_eq!(
            second_json["id"],
            serde_json::Value::from(91005u64),
            "second response must echo the follow-up ping's id (proves resync \
             worked and dispatch resumed); got: {second}"
        );
        assert_eq!(
            second_json["response"]["type"], "pong",
            "second response must be a pong (proves the bridge is still \
             servicing the same connection after the overflow event); got: {second}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 10.5 M1 — direct helper unit tests.
    //
    // These exercise `read_line_byte_budgeted` in isolation so a
    // regression in the byte-budget arithmetic (off-by-one on the
    // `+1` headroom, premature return, etc.) is caught without
    // standing up a full bridge.
    // ---------------------------------------------------------------

    /// Happy path: a small newline-terminated line under the cap
    /// is read in full with the newline included (matching
    /// `read_line`'s shape).
    #[tokio::test]
    async fn read_line_byte_budgeted_returns_full_line_under_cap() {
        let mut reader = tokio::io::BufReader::new(&b"hello\nworld\n"[..]);
        let mut buf = Vec::new();
        let n = read_line_byte_budgeted(&mut reader, &mut buf, CARRIER_MAX_LINE_BYTES + 1)
            .await
            .expect("read should succeed");
        assert_eq!(n, 6);
        assert_eq!(&buf, b"hello\n");
    }

    /// Overflow path: a payload longer than `max_bytes` with no
    /// embedded newline returns exactly `max_bytes` bytes, so the
    /// caller's `> CARRIER_MAX_LINE_BYTES` check fires.
    #[tokio::test]
    async fn read_line_byte_budgeted_caps_at_max_bytes_when_no_newline() {
        let payload = vec![b'A'; 4096];
        let mut reader = tokio::io::BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let max = 1024usize;
        let n = read_line_byte_budgeted(&mut reader, &mut buf, max)
            .await
            .expect("read should succeed");
        assert_eq!(
            n, max,
            "must return exactly max_bytes when no newline found"
        );
        assert_eq!(buf.len(), max, "buf must be capped at max_bytes");
    }

    /// EOF path: stream that closes before any byte is read
    /// returns `Ok(0)`, matching `read_line`'s EOF shape so the
    /// caller's `match Ok(0) => break` arm works unchanged.
    #[tokio::test]
    async fn read_line_byte_budgeted_returns_zero_on_immediate_eof() {
        let mut reader = tokio::io::BufReader::new(&b""[..]);
        let mut buf = Vec::new();
        let n = read_line_byte_budgeted(&mut reader, &mut buf, 1024)
            .await
            .expect("read should succeed");
        assert_eq!(n, 0);
        assert!(buf.is_empty());
    }

    /// `drain_to_newline` consumes everything up to and including
    /// the next `\n` — and only that — so the next read picks up
    /// from the start of the following line.
    #[tokio::test]
    async fn drain_to_newline_resyncs_to_next_line_start() {
        let mut reader = tokio::io::BufReader::new(&b"AAAAAAAA\nBBBB\n"[..]);
        drain_to_newline(&mut reader)
            .await
            .expect("drain should succeed");
        let mut buf = Vec::new();
        let n = read_line_byte_budgeted(&mut reader, &mut buf, 1024)
            .await
            .expect("post-drain read should succeed");
        assert_eq!(n, 5);
        assert_eq!(&buf, b"BBBB\n");
    }

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
