//! Browser Net/Exit stream gateway helpers.

use super::*;
#[cfg(unix)]
use crate::carrier::{open_browser_carrier_stream, BrowserCarrierStreamRequest};
use anyhow::Context as _;
#[cfg(unix)]
use std::net::IpAddr;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;
#[cfg(unix)]
use std::pin::Pin;
#[cfg(unix)]
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
#[cfg(unix)]
use std::task::{Context, Poll};
#[cfg(unix)]
use tokio::io::{
    copy, copy_bidirectional, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf,
};
#[cfg(unix)]
use tokio::net::{TcpStream, UnixListener, UnixStream};

const BROWSER_RUNTIME_STREAM_TMP_DIR: &str = "elastos-browser-streams";
const BROWSER_ADAPTER_IPC_TMP_DIR: &str = "elastos-browser-adapter-ipc";
#[cfg(unix)]
const BROWSER_RUNTIME_RELAY_OPEN_MAX_BYTES: usize = 16 * 1024;
#[cfg(unix)]
const BROWSER_RUNTIME_STREAM_ACCEPT_TIMEOUT_SECS: u64 = 300;
#[cfg(unix)]
const BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT: u64 = 64;
const EXIT_STREAM_SESSION_SCHEMA: &str = "elastos.exit.stream-session/v1";
const EXIT_REMOTE_CARRIER_SESSION_SCHEMA: &str = "elastos.exit.remote-carrier-session/v1";

#[cfg(unix)]
static BROWSER_VZ_FIXED_MEDIA_LISTENERS: OnceLock<
    tokio::sync::Mutex<BTreeMap<PathBuf, watch::Sender<bool>>>,
> = OnceLock::new();
#[cfg(unix)]
static BROWSER_RUNTIME_STREAM_LISTENERS: OnceLock<
    tokio::sync::Mutex<BTreeMap<PathBuf, watch::Sender<bool>>>,
> = OnceLock::new();

#[cfg(unix)]
struct BrowserMediaCountedStream<T> {
    inner: T,
    read_bytes: Arc<AtomicU64>,
    written_bytes: Arc<AtomicU64>,
}

#[cfg(unix)]
impl<T> BrowserMediaCountedStream<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            read_bytes: Arc::new(AtomicU64::new(0)),
            written_bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    fn byte_counts(&self) -> (Arc<AtomicU64>, Arc<AtomicU64>) {
        (
            Arc::clone(&self.read_bytes),
            Arc::clone(&self.written_bytes),
        )
    }
}

#[cfg(unix)]
impl<T: AsyncRead + Unpin> AsyncRead for BrowserMediaCountedStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        match Pin::new(&mut this.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                this.read_bytes.fetch_add(
                    u64::try_from(buffer.filled().len().saturating_sub(before)).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
}

#[cfg(unix)]
impl<T: AsyncWrite + Unpin> AsyncWrite for BrowserMediaCountedStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(context, buffer) {
            Poll::Ready(Ok(written)) => {
                this.written_bytes.fetch_add(
                    u64::try_from(written).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                Poll::Ready(Ok(written))
            }
            result => result,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

#[cfg(unix)]
struct BrowserMediaSessionDiagnostic {
    generation: String,
    page_id: String,
    media_stream_id: String,
    budget: Arc<BrowserMediaDiagnosticBudget>,
    guest_to_turn: Arc<AtomicU64>,
    turn_to_guest: Arc<AtomicU64>,
    reported: bool,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserMediaDiagnosticDecision {
    Emit,
    SuppressionSummary,
    Suppressed,
}

#[cfg(unix)]
struct BrowserMediaDiagnosticBudget {
    generation: String,
    page_id: String,
    media_stream_id: String,
    events_seen: AtomicU64,
    suppression_reported: AtomicBool,
}

#[cfg(unix)]
impl BrowserMediaDiagnosticBudget {
    fn new(generation: String, page_id: String, media_stream_id: String) -> Self {
        Self {
            generation,
            page_id,
            media_stream_id,
            events_seen: AtomicU64::new(0),
            suppression_reported: AtomicBool::new(false),
        }
    }

    fn next(&self) -> BrowserMediaDiagnosticDecision {
        let ordinal = self
            .events_seen
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(1))
            })
            .unwrap_or(u64::MAX);
        if ordinal < BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT {
            BrowserMediaDiagnosticDecision::Emit
        } else if !self.suppression_reported.swap(true, Ordering::Relaxed) {
            BrowserMediaDiagnosticDecision::SuppressionSummary
        } else {
            BrowserMediaDiagnosticDecision::Suppressed
        }
    }

    fn event_allowed(&self) -> bool {
        match self.next() {
            BrowserMediaDiagnosticDecision::Emit => true,
            BrowserMediaDiagnosticDecision::SuppressionSummary => {
                tracing::warn!(
                    generation = self.generation,
                    page_id = self.page_id,
                    media_stream_id = self.media_stream_id,
                    media_event = "diagnostics_suppressed",
                    diagnostic_event_limit = BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT,
                    suppressed_events_at_least = 1_u64,
                    "Browser VZ media diagnostic"
                );
                false
            }
            BrowserMediaDiagnosticDecision::Suppressed => false,
        }
    }
}

#[cfg(unix)]
impl BrowserMediaSessionDiagnostic {
    fn mark_reported(&mut self) {
        self.reported = true;
    }
}

#[cfg(unix)]
impl Drop for BrowserMediaSessionDiagnostic {
    fn drop(&mut self) {
        if self.reported {
            return;
        }
        if self.budget.event_allowed() {
            tracing::warn!(
                generation = self.generation,
                page_id = self.page_id,
                media_stream_id = self.media_stream_id,
                media_event = "guest_relay_cancelled",
                guest_to_turn_bytes = self.guest_to_turn.load(Ordering::Relaxed),
                turn_to_guest_bytes = self.turn_to_guest.load(Ordering::Relaxed),
                "Browser VZ media diagnostic"
            );
        }
    }
}

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

pub(in crate::api::gateway) async fn browser_reserve_stream_session(
    registry: &ProviderRegistry,
    request: &serde_json::Value,
) -> Result<serde_json::Value, (&'static str, anyhow::Error)> {
    let net_request = serde_json::json!({
        "target": request.get("target").cloned().unwrap_or(serde_json::Value::Null),
        "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
        "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
        "stream_nonce": request.get("stream_nonce").cloned().unwrap_or(serde_json::Value::Null),
    });
    let net_call = browser_provider_resource_call(
        "net",
        "stream",
        "elastos://net/stream".to_string(),
        net_request,
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

    let mut exit_request = serde_json::json!({
        "target": request.get("target").cloned().unwrap_or(serde_json::Value::Null),
        "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
        "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
        "stream_nonce": request.get("stream_nonce").cloned().unwrap_or(serde_json::Value::Null),
    });
    if let Some(remote_exit_id) = request
        .get("remote_exit_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        exit_request["remote_exit_id"] = serde_json::json!(remote_exit_id);
    }
    let exit_call = browser_provider_resource_call(
        "exit",
        "open_stream",
        "elastos://exit/open_stream".to_string(),
        exit_request,
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
    let byte_transport = receipt
        .get("byte_transport")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let target = match byte_transport {
        "adapter_ipc" => {
            let relay = browser_stream_relay(&receipt)?;
            BrowserRuntimeStreamTarget::LocalRelay(relay)
        }
        "carrier_stream"
            if receipt.get("schema").and_then(|value| value.as_str())
                == Some(EXIT_REMOTE_CARRIER_SESSION_SCHEMA) =>
        {
            BrowserRuntimeStreamTarget::Carrier(browser_carrier_exit_route(&receipt)?)
        }
        _ => return Ok(receipt),
    };
    let stream_id = receipt
        .get("stream_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("{byte_transport} stream session missing stream_id"))?
        .to_string();
    let runtime_stream_path = browser_runtime_stream_socket_path(data_dir, &stream_id)
        .map_err(|err| anyhow::anyhow!("failed to allocate Browser runtime stream path: {err}"))?;
    spawn_browser_runtime_stream_listener(&runtime_stream_path, target)
        .await
        .map_err(|err| {
            anyhow::anyhow!(
                "failed to bind Browser runtime stream socket {}: {err}",
                runtime_stream_path.display()
            )
        })?;
    let adapter_ipc_path = browser_adapter_ipc_socket_path(data_dir, &stream_id)
        .map_err(|err| anyhow::anyhow!("failed to allocate Browser adapter IPC path: {err}"))?;
    let adapter_ipc_path_text = adapter_ipc_path.to_string_lossy().to_string();
    let adapter_ipc = receipt
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("stream session receipt must be an object"))?
        .entry("adapter_ipc")
        .or_insert_with(|| {
            serde_json::json!({
                "schema": "elastos.adapter-ipc/v1",
                "kind": "unix_socket",
                "path": adapter_ipc_path_text,
                "stream_id": stream_id,
            })
        })
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("adapter_ipc stream session missing descriptor"))?;
    adapter_ipc
        .entry("schema".to_string())
        .or_insert_with(|| serde_json::json!("elastos.adapter-ipc/v1"));
    adapter_ipc
        .entry("kind".to_string())
        .or_insert_with(|| serde_json::json!("unix_socket"));
    adapter_ipc
        .entry("path".to_string())
        .or_insert_with(|| serde_json::json!(adapter_ipc_path.to_string_lossy().to_string()));
    adapter_ipc
        .entry("stream_id".to_string())
        .or_insert_with(|| serde_json::json!(stream_id));
    adapter_ipc.insert(
        "runtime_stream_path".to_string(),
        serde_json::json!(runtime_stream_path.to_string_lossy().to_string()),
    );
    Ok(receipt)
}

#[cfg(unix)]
pub(in crate::api::gateway) async fn spawn_browser_vz_fixed_media_listener(
    authority: &serde_json::Value,
) -> anyhow::Result<()> {
    validate_live_browser_vz_transport_authority(authority).map_err(anyhow::Error::msg)?;
    let socket_path = authority
        .pointer("/media/runtime_socket_path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media Runtime socket is missing"))?;
    let target = authority
        .pointer("/media/target")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media target is missing"))?;
    let parsed = url::Url::parse(target)?;
    let host = parsed
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .filter(IpAddr::is_loopback)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media target must be loopback"))?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media target port is missing"))?;
    let target = std::net::SocketAddr::new(host, port);
    let generation = authority
        .get("generation")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media generation is missing"))?
        .to_string();
    let page_id = authority
        .get("page_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media page binding is missing"))?
        .to_string();
    let media_stream_id = authority
        .pointer("/media/stream_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media stream binding is missing"))?
        .to_string();
    let expires_at = authority
        .get("expires_at_unix_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("Browser VZ media expiry is missing"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .as_millis() as u64;
    let lifetime = Duration::from_millis(
        expires_at
            .checked_sub(now)
            .ok_or_else(|| anyhow::anyhow!("Browser VZ media authority is expired"))?,
    );
    let socket_path = PathBuf::from(socket_path);
    let listeners = BROWSER_VZ_FIXED_MEDIA_LISTENERS.get_or_init(Default::default);
    let mut listeners = listeners.lock().await;
    if listeners.contains_key(&socket_path) {
        anyhow::bail!("Browser VZ media listener binding already exists");
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(&socket_path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => anyhow::bail!(
            "Browser VZ media Runtime socket path already exists: {}",
            socket_path.display()
        ),
        Err(err) => return Err(err.into()),
    }
    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(
        &socket_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    let (cancel, mut cancelled) = watch::channel(false);
    listeners.insert(socket_path.clone(), cancel);
    drop(listeners);
    tokio::spawn(async move {
        let media_diagnostics = Arc::new(BrowserMediaDiagnosticBudget::new(
            generation.clone(),
            page_id.clone(),
            media_stream_id.clone(),
        ));
        let mut sessions = tokio::task::JoinSet::new();
        let expiry = tokio::time::sleep(lifetime);
        tokio::pin!(expiry);
        loop {
            tokio::select! {
                _ = cancelled.changed() => break,
                _ = &mut expiry => break,
                accepted = listener.accept() => match accepted {
                    Ok((guest, _)) => {
                        let generation = generation.clone();
                        let page_id = page_id.clone();
                        let media_stream_id = media_stream_id.clone();
                        let media_diagnostics = Arc::clone(&media_diagnostics);
                        if media_diagnostics.event_allowed() {
                            tracing::info!(
                                generation,
                                page_id,
                                media_stream_id,
                                media_event = "guest_relay_accepted",
                                "Browser VZ media diagnostic"
                            );
                        }
                        sessions.spawn(async move {
                            let mut guest = BrowserMediaCountedStream::new(guest);
                            let (guest_to_turn, turn_to_guest) = guest.byte_counts();
                            let mut diagnostic = BrowserMediaSessionDiagnostic {
                                generation: generation.clone(),
                                page_id: page_id.clone(),
                                media_stream_id: media_stream_id.clone(),
                                budget: Arc::clone(&media_diagnostics),
                                guest_to_turn: Arc::clone(&guest_to_turn),
                                turn_to_guest: Arc::clone(&turn_to_guest),
                                reported: false,
                            };
                            let mut turn = match TcpStream::connect(target).await {
                                Ok(turn) => {
                                    if media_diagnostics.event_allowed() {
                                        tracing::info!(
                                            generation,
                                            page_id,
                                            media_stream_id,
                                            media_event = "turn_connected",
                                            "Browser VZ media diagnostic"
                                        );
                                    }
                                    turn
                                }
                                Err(error) => {
                                    if media_diagnostics.event_allowed() {
                                        tracing::warn!(
                                            generation,
                                            page_id,
                                            media_stream_id,
                                            media_event = "turn_connect_failed",
                                            error_kind = ?error.kind(),
                                            "Browser VZ media diagnostic"
                                        );
                                    }
                                    diagnostic.mark_reported();
                                    return;
                                }
                            };
                            let result = copy_bidirectional(&mut guest, &mut turn).await;
                            let guest_to_turn_bytes = guest_to_turn.load(Ordering::Relaxed);
                            let turn_to_guest_bytes = turn_to_guest.load(Ordering::Relaxed);
                            if media_diagnostics.event_allowed() {
                                match result {
                                    Ok(_) => tracing::info!(
                                        generation,
                                        page_id,
                                        media_stream_id,
                                        media_event = "guest_relay_closed",
                                        guest_to_turn_bytes,
                                        turn_to_guest_bytes,
                                        "Browser VZ media diagnostic"
                                    ),
                                    Err(error) => tracing::warn!(
                                        generation,
                                        page_id,
                                        media_stream_id,
                                        media_event = "guest_relay_failed",
                                        guest_to_turn_bytes,
                                        turn_to_guest_bytes,
                                        error_kind = ?error.kind(),
                                        "Browser VZ media diagnostic"
                                    ),
                                }
                            }
                            diagnostic.mark_reported();
                        });
                    }
                    Err(_) => break,
                },
                result = sessions.join_next(), if !sessions.is_empty() => {
                    if let Some(Err(error)) = result {
                        if media_diagnostics.event_allowed() {
                            tracing::warn!(
                                generation,
                                page_id,
                                media_stream_id,
                                media_event = "guest_relay_task_failed",
                                error = %error,
                                "Browser VZ media diagnostic"
                            );
                        }
                    }
                }
            }
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
        drop(listener);
        let _ = std::fs::remove_file(&socket_path);
        let listeners = BROWSER_VZ_FIXED_MEDIA_LISTENERS.get_or_init(Default::default);
        listeners.lock().await.remove(&socket_path);
    });
    Ok(())
}

#[cfg(not(unix))]
pub(in crate::api::gateway) async fn spawn_browser_vz_fixed_media_listener(
    _authority: &serde_json::Value,
) -> anyhow::Result<()> {
    anyhow::bail!("Browser VZ media forwarding requires a Unix Runtime host")
}

pub(in crate::api::gateway) async fn close_browser_vz_fixed_media_listener(
    authority: &serde_json::Value,
) -> Result<(), String> {
    validate_browser_vz_transport_authority(authority)?;
    let socket_path = PathBuf::from(
        authority
            .pointer("/media/runtime_socket_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Browser VZ media Runtime socket is missing".to_string())?,
    );
    #[cfg(unix)]
    {
        let listeners = BROWSER_VZ_FIXED_MEDIA_LISTENERS.get_or_init(Default::default);
        let cancel = listeners.lock().await.get(&socket_path).cloned();
        if let Some(cancel) = cancel {
            cancel
                .send(true)
                .map_err(|_| "Browser VZ media listener owner disappeared".to_string())?;
        } else if socket_path.exists() {
            let metadata = std::fs::symlink_metadata(&socket_path)
                .map_err(|err| format!("Browser VZ media socket metadata failed: {err}"))?;
            if !metadata.file_type().is_socket() {
                return Err(
                    "Browser VZ media path exists without an exact Runtime socket".to_string(),
                );
            }
            match UnixStream::connect(&socket_path).await {
                Ok(stream) => {
                    drop(stream);
                    return Err(
                        "Browser VZ media socket has a live owner outside this Runtime".to_string(),
                    );
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(&socket_path).map_err(|err| {
                        format!("Browser VZ stale media socket retirement failed: {err}")
                    })?;
                    return Ok(());
                }
                Err(err) => {
                    return Err(format!(
                        "Browser VZ media socket ownership is indeterminate: {err}"
                    ))
                }
            }
        } else {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let registered = listeners.lock().await.contains_key(&socket_path);
            if !registered && !socket_path.exists() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Browser VZ media listener cleanup timed out".to_string());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        Err("Browser VZ media forwarding requires a Unix Runtime host".to_string())
    }
}

#[derive(Debug, Clone)]
struct BrowserExitRelay {
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct BrowserCarrierExitRoute {
    connect_ticket: String,
    peer_did: Option<String>,
    carrier_service: String,
    grant_id: String,
    principal_id: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone)]
enum BrowserRuntimeStreamTarget {
    LocalRelay(Option<BrowserExitRelay>),
    Carrier(BrowserCarrierExitRoute),
}

#[cfg(unix)]
#[derive(Debug, Default)]
struct BrowserRelayOpenLog {
    schema: String,
    stream_id: String,
    target: String,
    scheme: String,
    host: String,
    reason: Option<String>,
}

#[cfg(unix)]
async fn read_browser_relay_open_line(
    stream: &mut UnixStream,
) -> anyhow::Result<(Vec<u8>, BrowserRelayOpenLog)> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        if let Err(err) = stream.read_exact(&mut byte).await {
            if line.is_empty() {
                anyhow::bail!("browser runtime stream closed before relay-open handshake: {err}");
            }
            anyhow::bail!(
                "browser runtime stream closed during {} relay-open handshake: {err}",
                browser_runtime_stream_line_kind(&line)
            );
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if line.len() > BROWSER_RUNTIME_RELAY_OPEN_MAX_BYTES {
            anyhow::bail!("browser runtime relay-open handshake is too large");
        }
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(
        line.strip_suffix(b"\n").unwrap_or(line.as_slice()),
    )
    .map_err(|err| {
        anyhow::anyhow!(
            "browser runtime relay-open handshake is not JSON ({}): {err}",
            browser_runtime_stream_line_kind(&line)
        )
    })?;
    let log = BrowserRelayOpenLog {
        schema: parsed
            .get("schema")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        stream_id: parsed
            .get("stream_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        target: parsed
            .get("target")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        scheme: parsed
            .get("scheme")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        host: parsed
            .get("host")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        reason: parsed
            .get("reason")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string()),
    };
    Ok((line, log))
}

#[cfg(unix)]
fn browser_runtime_stream_line_kind(line: &[u8]) -> &'static str {
    if line.is_empty() {
        return "empty";
    }
    if line.starts_with(b"{") {
        return "json";
    }
    for method in [
        b"GET ".as_slice(),
        b"POST ".as_slice(),
        b"PUT ".as_slice(),
        b"DELETE ".as_slice(),
        b"HEAD ".as_slice(),
        b"CONNECT ".as_slice(),
        b"OPTIONS ".as_slice(),
        b"PATCH ".as_slice(),
    ] {
        if line.starts_with(method) {
            return "http";
        }
    }
    "unknown"
}

#[cfg(unix)]
fn browser_runtime_stream_id_is_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

#[cfg(unix)]
fn browser_runtime_stream_target_is_supported(value: &str) -> bool {
    value.starts_with("tcp://") || value.starts_with("tls://")
}

#[cfg(unix)]
fn validate_browser_runtime_relay_open(open_log: &BrowserRelayOpenLog) -> anyhow::Result<()> {
    if open_log.schema != "elastos.exit.relay-open/v1" {
        anyhow::bail!("browser runtime relay-open schema mismatch");
    }
    if !browser_runtime_stream_id_is_safe(&open_log.stream_id) {
        anyhow::bail!("browser runtime relay-open stream_id must be a safe identifier");
    }
    if !browser_runtime_stream_target_is_supported(&open_log.target) {
        anyhow::bail!("browser runtime relay-open target must use tcp:// or tls://");
    }
    Ok(())
}

#[cfg(unix)]
fn browser_carrier_stream_public_ip(ip: IpAddr) -> anyhow::Result<()> {
    match ip {
        IpAddr::V4(ip) => {
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
            {
                anyhow::bail!("private IP blocked: {ip}");
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
            {
                anyhow::bail!("private IP blocked: {ip}");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn browser_carrier_stream_public_host(host: &str) -> anyhow::Result<()> {
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() {
        anyhow::bail!("browser runtime Carrier relay-open target requires a host");
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\\' | b'\0'))
    {
        anyhow::bail!("browser runtime Carrier relay-open target host is invalid: {host}");
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        anyhow::bail!("private host blocked: {host}");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        browser_carrier_stream_public_ip(ip)?;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_browser_carrier_stream_target(open_log: &BrowserRelayOpenLog) -> anyhow::Result<()> {
    validate_browser_runtime_relay_open(open_log)?;
    let target = url::Url::parse(&open_log.target).map_err(|err| {
        anyhow::anyhow!("browser runtime Carrier relay-open target is invalid: {err}")
    })?;
    if !open_log.scheme.is_empty() && open_log.scheme != target.scheme() {
        anyhow::bail!("browser runtime Carrier relay-open scheme hint does not match target");
    }
    let host = target.host_str().ok_or_else(|| {
        anyhow::anyhow!("browser runtime Carrier relay-open target requires a host")
    })?;
    if !open_log.host.is_empty()
        && !open_log
            .host
            .trim_matches(['[', ']'])
            .eq_ignore_ascii_case(host.trim_matches(['[', ']']))
    {
        anyhow::bail!("browser runtime Carrier relay-open host hint does not match target");
    }
    let _port = target.port_or_known_default().ok_or_else(|| {
        anyhow::anyhow!("browser runtime Carrier relay-open target requires a port")
    })?;
    browser_carrier_stream_public_host(host)
}

#[cfg(unix)]
fn browser_carrier_stream_request_for_open(
    route: &BrowserCarrierExitRoute,
    open_log: &BrowserRelayOpenLog,
) -> anyhow::Result<BrowserCarrierStreamRequest> {
    validate_browser_carrier_stream_target(open_log)?;
    Ok(BrowserCarrierStreamRequest {
        connect_ticket: route.connect_ticket.clone(),
        peer_did: route.peer_did.clone(),
        carrier_service: route.carrier_service.clone(),
        grant_id: route.grant_id.clone(),
        stream_id: open_log.stream_id.clone(),
        target: open_log.target.clone(),
        principal_id: route.principal_id.clone(),
        reason: open_log.reason.clone().or_else(|| route.reason.clone()),
        timeout_ms: Some(5_000),
    })
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
    Ok(Some(BrowserExitRelay {
        path: PathBuf::from(path),
    }))
}

fn browser_carrier_exit_route(
    receipt: &serde_json::Value,
) -> anyhow::Result<BrowserCarrierExitRoute> {
    let carrier = receipt
        .get("carrier")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow::anyhow!("remote Carrier exit receipt missing carrier descriptor"))?;
    if carrier.get("schema").and_then(|value| value.as_str())
        != Some("elastos.exit.remote-carrier/v1")
    {
        anyhow::bail!("remote Carrier exit descriptor schema mismatch");
    }
    if carrier.get("transport").and_then(|value| value.as_str()) != Some("carrier_stream") {
        anyhow::bail!("remote Carrier exit descriptor must use carrier_stream transport");
    }
    let connect_ticket = carrier
        .get("connect_ticket")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty() && value.trim() == *value)
        .ok_or_else(|| anyhow::anyhow!("remote Carrier exit descriptor missing connect_ticket"))?;
    if connect_ticket
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        anyhow::bail!("remote Carrier exit connect_ticket must not contain whitespace or NUL");
    }
    let carrier_service = carrier
        .get("carrier_service")
        .and_then(|value| value.as_str())
        .filter(|value| *value == "elastos://exit/open_stream")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "remote Carrier exit carrier_service must be elastos://exit/open_stream"
            )
        })?;
    let grant_id = carrier
        .get("grant_id")
        .and_then(|value| value.as_str())
        .or_else(|| receipt.get("grant_id").and_then(|value| value.as_str()))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote Carrier exit receipt missing grant_id"))?;
    let _stream_id = receipt
        .get("stream_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote Carrier exit receipt missing stream_id"))?;
    let _target = receipt
        .get("target")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote Carrier exit receipt missing target"))?;
    let peer_did = carrier
        .get("peer_did")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string());
    let principal_id = receipt
        .get("principal_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string());
    let reason = receipt
        .get("reason")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    Ok(BrowserCarrierExitRoute {
        connect_ticket: connect_ticket.to_string(),
        peer_did,
        carrier_service: carrier_service.to_string(),
        grant_id: grant_id.to_string(),
        principal_id,
        reason,
    })
}

#[cfg(unix)]
async fn bridge_browser_runtime_stream_to_local_relay(
    session_path: &FsPath,
    stream: &mut UnixStream,
    relay: Option<BrowserExitRelay>,
) -> anyhow::Result<()> {
    let Some(relay) = relay else {
        tracing::info!(
            path = %session_path.display(),
            "browser runtime stream accepted and closed fail-closed"
        );
        return Ok(());
    };
    let (open_line, open_log) = read_browser_relay_open_line(stream).await?;
    validate_browser_runtime_relay_open(&open_log)?;
    let mut relay_stream = UnixStream::connect(&relay.path).await?;
    relay_stream.write_all(&open_line).await?;
    match copy_bidirectional(stream, &mut relay_stream).await {
        Ok((to_relay, to_engine)) => {
            tracing::info!(
                path = %session_path.display(),
                relay = %relay.path.display(),
                stream_id = %open_log.stream_id,
                target = %open_log.target,
                scheme = %open_log.scheme,
                host = %open_log.host,
                to_relay,
                to_engine,
                "browser runtime stream relay session closed"
            );
            Ok(())
        }
        Err(err) => {
            if browser_stream_copy_was_cancelled(&err) {
                tracing::info!(
                    path = %session_path.display(),
                    relay = %relay.path.display(),
                    stream_id = %open_log.stream_id,
                    target = %open_log.target,
                    scheme = %open_log.scheme,
                    host = %open_log.host,
                    error = %err,
                    "browser runtime stream relay session closed by peer"
                );
                Ok(())
            } else {
                tracing::warn!(
                    path = %session_path.display(),
                    relay = %relay.path.display(),
                    stream_id = %open_log.stream_id,
                    target = %open_log.target,
                    scheme = %open_log.scheme,
                    host = %open_log.host,
                    error = %err,
                    "browser runtime stream relay session failed"
                );
                Err(err.into())
            }
        }
    }
}

#[cfg(unix)]
fn browser_stream_copy_was_cancelled(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
}

#[cfg(unix)]
async fn bridge_browser_runtime_stream_to_carrier(
    session_path: &FsPath,
    mut stream: UnixStream,
    route: BrowserCarrierExitRoute,
) -> anyhow::Result<()> {
    let (open_line, open_log) = read_browser_relay_open_line(&mut stream).await?;
    let request = browser_carrier_stream_request_for_open(&route, &open_log)?;
    let stream_id = request.stream_id.clone();
    let target = request.target.clone();
    let mut carrier_stream = open_browser_carrier_stream(&request).await?;
    carrier_stream.send.write_all(&open_line).await?;
    let (carrier_send, carrier_recv) = (&mut carrier_stream.send, &mut carrier_stream.recv);
    let (mut stream_read, mut stream_write) = stream.into_split();
    let to_carrier = async {
        let copied = copy(&mut stream_read, carrier_send).await?;
        carrier_send.finish()?;
        carrier_send.stopped().await.ok();
        Ok::<u64, anyhow::Error>(copied)
    };
    let from_carrier = async {
        let copied = copy(carrier_recv, &mut stream_write).await?;
        stream_write.shutdown().await.ok();
        Ok::<u64, anyhow::Error>(copied)
    };
    let (to_carrier, to_engine) = tokio::try_join!(to_carrier, from_carrier)?;
    tracing::info!(
        path = %session_path.display(),
        stream_id = %stream_id,
        target = %target,
        to_carrier,
        to_engine,
        "browser runtime stream Carrier session closed"
    );
    Ok(())
}

#[cfg(unix)]
async fn spawn_browser_runtime_stream_listener(
    path: &FsPath,
    target: BrowserRuntimeStreamTarget,
) -> anyhow::Result<()> {
    spawn_browser_runtime_stream_listener_with_accept_timeout(
        path,
        target,
        Duration::from_secs(BROWSER_RUNTIME_STREAM_ACCEPT_TIMEOUT_SECS),
    )
    .await
}

#[cfg(unix)]
async fn spawn_browser_runtime_stream_listener_with_accept_timeout(
    path: &FsPath,
    target: BrowserRuntimeStreamTarget,
    accept_timeout: Duration,
) -> anyhow::Result<()> {
    let listeners = BROWSER_RUNTIME_STREAM_LISTENERS.get_or_init(Default::default);
    let mut listeners = listeners.lock().await;
    if listeners.contains_key(path) {
        anyhow::bail!(
            "Runtime browser stream listener binding already exists: {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() {
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(stream) => {
                    drop(stream);
                    anyhow::bail!(
                        "Runtime browser stream socket has a live owner: {}",
                        path.display()
                    );
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(path)?;
                }
                Err(err) => {
                    anyhow::bail!(
                        "Runtime browser stream socket ownership is indeterminate at {}: {err}",
                        path.display()
                    );
                }
            }
        } else {
            anyhow::bail!("Runtime browser stream path exists and is not a Unix socket");
        }
    }
    let listener = UnixListener::bind(path)
        .map_err(|err| anyhow::anyhow!("UnixListener::bind({}) failed: {err}", path.display()))?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let cleanup_path = path.to_path_buf();
    let (cancel, mut cancelled) = watch::channel(false);
    listeners.insert(cleanup_path.clone(), cancel);
    drop(listeners);
    tokio::spawn(async move {
        let mut accepted_any = false;
        let mut sessions = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = cancelled.changed() => break,
                accepted = tokio::time::timeout(accept_timeout, listener.accept()) => match accepted {
                Ok(Ok((mut stream, _addr))) => {
                    accepted_any = true;
                    let target = target.clone();
                    let session_path = cleanup_path.clone();
                    sessions.spawn(async move {
                        match target {
                            BrowserRuntimeStreamTarget::LocalRelay(relay) => {
                                if let Err(err) = bridge_browser_runtime_stream_to_local_relay(
                                    &session_path,
                                    &mut stream,
                                    relay,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        path = %session_path.display(),
                                        error = %err,
                                        "browser runtime stream local relay failed"
                                    );
                                }
                            }
                            BrowserRuntimeStreamTarget::Carrier(route) => {
                                if let Err(err) = bridge_browser_runtime_stream_to_carrier(
                                    &session_path,
                                    stream,
                                    route,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        path = %session_path.display(),
                                        error = %err,
                                        "browser runtime stream Carrier relay failed"
                                    );
                                }
                            }
                        }
                    });
                }
                Ok(Err(err)) => {
                    tracing::info!(
                        path = %cleanup_path.display(),
                        error = %err,
                        "browser runtime stream listener failed"
                    );
                    break;
                }
                Err(_) => {
                    if !accepted_any {
                        tracing::debug!(
                            path = %cleanup_path.display(),
                            "browser runtime stream listener expired before first use"
                        );
                        break;
                    }
                }
                },
                result = sessions.join_next(), if !sessions.is_empty() => {
                    if let Some(Err(err)) = result {
                        tracing::warn!(
                            path = %cleanup_path.display(),
                            error = %err,
                            "browser runtime stream relay task failed"
                        );
                    }
                }
            };
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
        drop(listener);
        let _ = std::fs::remove_file(&cleanup_path);
        let listeners = BROWSER_RUNTIME_STREAM_LISTENERS.get_or_init(Default::default);
        listeners.lock().await.remove(&cleanup_path);
    });
    Ok(())
}

#[cfg(unix)]
pub(in crate::api::gateway) async fn close_browser_runtime_stream_listener(
    data_dir: &FsPath,
    stream_id: &str,
) -> Result<(), String> {
    let socket_path = browser_runtime_stream_socket_path(data_dir, stream_id)
        .map_err(|err| format!("Browser runtime stream socket binding is invalid: {err}"))?;
    let listeners = BROWSER_RUNTIME_STREAM_LISTENERS.get_or_init(Default::default);
    let cancel = listeners.lock().await.get(&socket_path).cloned();
    if let Some(cancel) = cancel {
        cancel
            .send(true)
            .map_err(|_| "Browser runtime stream listener owner disappeared".to_string())?;
    } else if socket_path.exists() {
        let metadata = std::fs::symlink_metadata(&socket_path)
            .map_err(|err| format!("Browser runtime stream socket metadata failed: {err}"))?;
        if !metadata.file_type().is_socket() {
            return Err(
                "Browser runtime stream path exists without an exact Runtime socket".to_string(),
            );
        }
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                drop(stream);
                return Err(
                    "Browser runtime stream socket has a live owner outside this Runtime"
                        .to_string(),
                );
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) =>
            {
                std::fs::remove_file(&socket_path).map_err(|err| {
                    format!("Browser runtime stale stream socket retirement failed: {err}")
                })?;
                return Ok(());
            }
            Err(err) => {
                return Err(format!(
                    "Browser runtime stream socket ownership is indeterminate: {err}"
                ))
            }
        }
    } else {
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let registered = listeners.lock().await.contains_key(&socket_path);
        if !registered && !socket_path.exists() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Browser runtime stream listener cleanup timed out".to_string());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(not(unix))]
pub(in crate::api::gateway) async fn close_browser_runtime_stream_listener(
    _data_dir: &FsPath,
    _stream_id: &str,
) -> Result<(), String> {
    Err("Browser runtime stream sockets require a Unix host adapter".to_string())
}

#[cfg(not(unix))]
async fn spawn_browser_runtime_stream_listener(
    _path: &FsPath,
    _target: BrowserRuntimeStreamTarget,
) -> anyhow::Result<()> {
    anyhow::bail!("Browser runtime stream sockets require a Unix host adapter");
}

pub(in crate::api::gateway) fn browser_runtime_stream_socket_path(
    data_dir: &FsPath,
    stream_id: &str,
) -> anyhow::Result<PathBuf> {
    browser_stream_socket_path(data_dir, stream_id, BROWSER_RUNTIME_STREAM_TMP_DIR)
}

fn browser_adapter_ipc_socket_path(data_dir: &FsPath, stream_id: &str) -> anyhow::Result<PathBuf> {
    browser_stream_socket_path(data_dir, stream_id, BROWSER_ADAPTER_IPC_TMP_DIR)
}

fn browser_stream_socket_path(
    data_dir: &FsPath,
    stream_id: &str,
    directory: &str,
) -> anyhow::Result<PathBuf> {
    if !stream_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
    {
        anyhow::bail!("stream_id must be a safe identifier");
    }
    let digest = Sha256::digest(format!("{}\n{stream_id}", data_dir.to_string_lossy()).as_bytes());
    let socket_name = format!("{}.sock", hex::encode(&digest[..16]));
    // Unix socket paths have a small platform limit. Keep Browser stream sockets
    // in /tmp rather than platform temp roots like macOS /var/folders/.../T.
    let stream_dir = browser_runtime_stream_root().join(browser_stream_dir_name(directory));
    ensure_private_stream_dir(&stream_dir)?;
    Ok(stream_dir.join(socket_name))
}

#[cfg(unix)]
fn browser_runtime_stream_root() -> PathBuf {
    PathBuf::from("/tmp")
}

#[cfg(not(unix))]
fn browser_runtime_stream_root() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(unix)]
fn browser_stream_dir_name(directory: &str) -> String {
    format!("{directory}-{}", unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn browser_stream_dir_name(directory: &str) -> String {
    directory.to_string()
}

#[cfg(unix)]
fn ensure_private_stream_dir(path: &FsPath) -> anyhow::Result<()> {
    use std::io::ErrorKind;
    use std::os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    };

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to create Browser stream root {path:?}"));
        }
    }

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect Browser stream root {path:?}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("Browser stream root must be a real directory");
    }

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    let directory = options
        .open(path)
        .with_context(|| format!("failed to open Browser stream root {path:?}"))?;
    let metadata = directory
        .metadata()
        .with_context(|| format!("failed to inspect opened Browser stream root {path:?}"))?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        anyhow::bail!("Browser stream root must be owned by the effective user");
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        anyhow::bail!("Browser stream root must use mode 0700");
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_stream_dir(path: &FsPath) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create Browser stream root {path:?}"))
}

pub(in crate::api::gateway) fn validate_browser_stream_receipt(
    receipt: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    match receipt.get("schema").and_then(|value| value.as_str()) {
        Some(EXIT_STREAM_SESSION_SCHEMA) => Ok(receipt),
        Some(EXIT_REMOTE_CARRIER_SESSION_SCHEMA) => {
            if receipt
                .get("byte_transport")
                .and_then(|value| value.as_str())
                == Some("carrier_stream")
            {
                browser_carrier_exit_route(&receipt)?;
                return Ok(receipt);
            }
            anyhow::bail!("remote Carrier exit receipt must use carrier_stream byte_transport");
        }
        _ => {
            anyhow::bail!(
                "stream provider did not return an elastos.exit.stream-session/v1 or elastos.exit.remote-carrier-session/v1 receipt"
            );
        }
    }
}

pub(in crate::api::gateway) fn browser_visible_stream_session(
    receipt: &serde_json::Value,
) -> serde_json::Value {
    let mut visible = receipt.clone();
    if let Some(object) = visible.as_object_mut() {
        object.remove("adapter_ipc");
        object.remove("relay_ipc");
    }
    scrub_browser_stream_authority_fields(&mut visible);
    visible
}

pub(in crate::api::gateway) fn browser_engine_stream_session(
    receipt: &serde_json::Value,
) -> serde_json::Value {
    let has_adapter_ipc = receipt.get("adapter_ipc").is_some();
    let engine_schema = if has_adapter_ipc {
        serde_json::json!(EXIT_STREAM_SESSION_SCHEMA)
    } else {
        receipt
            .get("schema")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    let engine_byte_transport = if has_adapter_ipc {
        serde_json::json!("adapter_ipc")
    } else {
        receipt
            .get("byte_transport")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    let mut engine_session = serde_json::json!({
        "schema": engine_schema,
        "stream_id": receipt.get("stream_id").cloned().unwrap_or(serde_json::Value::Null),
        "target": receipt.get("target").cloned().unwrap_or(serde_json::Value::Null),
        "byte_transport": engine_byte_transport,
        "adapter_ipc": receipt.get("adapter_ipc").cloned().unwrap_or(serde_json::Value::Null),
    });
    if let Some(relay_ipc) = receipt.get("relay_ipc").filter(|value| !value.is_null()) {
        if let Some(object) = engine_session.as_object_mut() {
            object.insert("relay_ipc".to_string(), relay_ipc.clone());
        }
    }
    engine_session
}

pub(in crate::api::gateway) fn browser_stream_cleanup(
    receipt: &serde_json::Value,
) -> Option<BrowserStreamCleanup> {
    let schema = receipt.get("schema").and_then(serde_json::Value::as_str);
    let byte_transport = receipt
        .get("byte_transport")
        .and_then(serde_json::Value::as_str);
    if !matches!(
        (schema, byte_transport),
        (Some(EXIT_STREAM_SESSION_SCHEMA), Some("adapter_ipc"))
            | (
                Some(EXIT_REMOTE_CARRIER_SESSION_SCHEMA),
                Some("carrier_stream")
            )
    ) {
        return None;
    }
    let stream_id = receipt
        .get("stream_id")
        .and_then(|value| value.as_str())
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
        })?
        .to_string();
    let principal_id = receipt
        .get("principal_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())?
        .to_string();
    Some(BrowserStreamCleanup {
        stream_id,
        principal_id,
    })
}

fn scrub_browser_stream_authority_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("connect_ticket");
            for value in object.values_mut() {
                scrub_browser_stream_authority_fields(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                scrub_browser_stream_authority_fields(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
    #[cfg(unix)]
    use iroh::Watcher as _;
    use serde_json::json;
    #[cfg(unix)]
    use std::sync::Arc;

    #[test]
    fn engine_stream_session_keeps_relay_ipc_for_vm_launch() {
        let receipt = json!({
            "schema": EXIT_STREAM_SESSION_SCHEMA,
            "byte_transport": "adapter_ipc",
            "stream_id": "stream:relay-ipc-test",
            "target": "tls://example.com:443",
            "adapter_ipc": {
                "schema": "elastos.adapter-ipc/v1",
                "kind": "unix_socket",
                "path": "/tmp/browser-adapter.sock",
                "stream_id": "stream:relay-ipc-test"
            },
            "relay_ipc": {
                "schema": "elastos.exit.relay-ipc/v1",
                "kind": "unix_socket",
                "path": "/tmp/browser-relay.sock",
                "stream_id": "stream:relay-ipc-test"
            }
        });

        let visible = browser_visible_stream_session(&receipt);
        assert!(visible.get("adapter_ipc").is_none());
        assert!(visible.get("relay_ipc").is_none());

        let engine = browser_engine_stream_session(&receipt);
        assert_eq!(
            engine
                .pointer("/adapter_ipc/schema")
                .and_then(|value| value.as_str()),
            Some("elastos.adapter-ipc/v1")
        );
        assert_eq!(
            engine
                .pointer("/relay_ipc/kind")
                .and_then(|value| value.as_str()),
            Some("unix_socket")
        );
        assert_eq!(
            engine
                .pointer("/relay_ipc/path")
                .and_then(|value| value.as_str()),
            Some("/tmp/browser-relay.sock")
        );
    }

    #[tokio::test]
    async fn attach_runtime_stream_path_repairs_adapter_ipc_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let receipt = json!({
            "schema": EXIT_STREAM_SESSION_SCHEMA,
            "byte_transport": "adapter_ipc",
            "stream_id": "stream:missing-path-test",
            "target": "tls://example.com:443",
            "adapter_ipc": {
                "schema": "elastos.adapter-ipc/v1",
                "kind": "unix_socket",
                "stream_id": "stream:missing-path-test"
            }
        });

        let attached = browser_attach_runtime_stream_path(dir.path(), receipt)
            .await
            .unwrap();
        let adapter_ipc = attached.get("adapter_ipc").unwrap();
        let path = adapter_ipc
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap();
        let runtime_stream_path = adapter_ipc
            .get("runtime_stream_path")
            .and_then(|value| value.as_str())
            .unwrap();
        let runtime_stream_path = PathBuf::from(runtime_stream_path);

        #[cfg(unix)]
        let expected_adapter_root = format!("/tmp/elastos-browser-adapter-ipc-{}/", unsafe {
            libc::geteuid()
        });
        #[cfg(not(unix))]
        let expected_adapter_root = format!(
            "{}/elastos-browser-adapter-ipc/",
            std::env::temp_dir().display()
        );
        #[cfg(unix)]
        let expected_runtime_root = format!("/tmp/elastos-browser-streams-{}/", unsafe {
            libc::geteuid()
        });
        #[cfg(not(unix))]
        let expected_runtime_root = format!(
            "{}/elastos-browser-streams/",
            std::env::temp_dir().display()
        );

        assert!(path.starts_with(&expected_adapter_root));
        assert!(path.ends_with(".sock"));
        assert!(runtime_stream_path.starts_with(expected_runtime_root));
        assert_ne!(PathBuf::from(path), runtime_stream_path);
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(
                &std::fs::symlink_metadata(&runtime_stream_path)
                    .unwrap()
                    .permissions(),
            ) & 0o777,
            0o600
        );
        close_browser_runtime_stream_listener(dir.path(), "stream:missing-path-test")
            .await
            .unwrap();
        assert!(!runtime_stream_path.exists());
    }

    #[test]
    fn local_adapter_stream_receipt_retains_exact_cleanup_binding() {
        let cleanup = browser_stream_cleanup(&json!({
            "schema": EXIT_STREAM_SESSION_SCHEMA,
            "byte_transport": "adapter_ipc",
            "stream_id": "stream:local-cleanup-test",
            "principal_id": "person:local:cleanup-test",
        }))
        .expect("local Runtime stream cleanup");
        assert_eq!(cleanup.stream_id, "stream:local-cleanup-test");
        assert_eq!(cleanup.principal_id, "person:local:cleanup-test");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn accepted_runtime_stream_listener_survives_multiple_accept_timeouts() {
        let dir = tempfile::tempdir().unwrap();
        let runtime_path = dir.path().join("runtime.sock");
        let relay_path = dir.path().join("relay.sock");
        let relay_listener = UnixListener::bind(&relay_path).unwrap();
        spawn_browser_runtime_stream_listener_with_accept_timeout(
            &runtime_path,
            BrowserRuntimeStreamTarget::LocalRelay(Some(BrowserExitRelay { path: relay_path })),
            Duration::from_millis(100),
        )
        .await
        .unwrap();

        let relay = tokio::spawn(async move {
            let (mut stream, _) = relay_listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(350)).await;
            stream.write_all(b"x").await.unwrap();
        });
        let mut browser = UnixStream::connect(&runtime_path).await.unwrap();
        browser
            .write_all(
                br#"{"schema":"elastos.exit.relay-open/v1","stream_id":"stream:lifetime-test","target":"tls://example.com:443","scheme":"tls","host":"example.com"}
"#,
            )
            .await
            .unwrap();
        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(2), browser.read_exact(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(byte, [b'x']);
        assert!(runtime_path.exists());

        let listeners = BROWSER_RUNTIME_STREAM_LISTENERS.get_or_init(Default::default);
        listeners
            .lock()
            .await
            .get(&runtime_path)
            .cloned()
            .unwrap()
            .send(true)
            .unwrap();
        for _ in 0..200 {
            if !runtime_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!runtime_path.exists());
        relay.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_stream_socket_ownership_is_data_root_scoped_and_live_safe() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let stream_id = "stream:two-runtime-roots";
        let first_path = browser_runtime_stream_socket_path(first.path(), stream_id).unwrap();
        let second_path = browser_runtime_stream_socket_path(second.path(), stream_id).unwrap();
        assert_ne!(first_path, second_path);

        let foreign = std::os::unix::net::UnixListener::bind(&first_path).unwrap();
        let error = spawn_browser_runtime_stream_listener(
            &first_path,
            BrowserRuntimeStreamTarget::LocalRelay(None),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("live owner"));
        assert!(first_path.exists());

        spawn_browser_runtime_stream_listener(
            &second_path,
            BrowserRuntimeStreamTarget::LocalRelay(None),
        )
        .await
        .unwrap();
        close_browser_runtime_stream_listener(second.path(), stream_id)
            .await
            .unwrap();
        assert!(first_path.exists());
        let connection = std::os::unix::net::UnixStream::connect(&first_path).unwrap();
        drop(connection);
        drop(foreign);
        std::fs::remove_file(first_path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn browser_stream_root_is_created_owner_only_and_reused() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser-stream-root");
        ensure_private_stream_dir(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        ensure_private_stream_dir(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn browser_stream_root_rejects_group_readable_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser-stream-root");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = ensure_private_stream_dir(&path).unwrap_err();
        assert!(err.to_string().contains("0700"));
    }

    #[cfg(unix)]
    #[test]
    fn browser_stream_root_rejects_owner_writable_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser-stream-root");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o733)).unwrap();
        let err = ensure_private_stream_dir(&path).unwrap_err();
        assert!(err.to_string().contains("0700"));
    }

    #[cfg(unix)]
    #[test]
    fn browser_stream_root_rejects_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser-stream-root");
        std::fs::write(&path, b"not-a-directory").unwrap();
        let err = ensure_private_stream_dir(&path).unwrap_err();
        assert!(err.to_string().contains("real directory"));
    }

    #[cfg(unix)]
    #[test]
    fn browser_stream_root_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-root");
        let path = dir.path().join("browser-stream-root");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let err = ensure_private_stream_dir(&path).unwrap_err();
        assert!(err.to_string().contains("real directory"));
    }

    #[tokio::test]
    async fn attach_runtime_stream_path_preserves_existing_adapter_ipc_path() {
        let dir = tempfile::tempdir().unwrap();
        let existing_path = dir.path().join("adapter.sock");
        let existing_path = existing_path.to_string_lossy().to_string();
        let receipt = json!({
            "schema": EXIT_STREAM_SESSION_SCHEMA,
            "byte_transport": "adapter_ipc",
            "stream_id": "stream:existing-path-test",
            "target": "tls://example.com:443",
            "adapter_ipc": {
                "schema": "elastos.adapter-ipc/v1",
                "kind": "unix_socket",
                "path": existing_path,
                "stream_id": "stream:existing-path-test"
            }
        });

        let attached = browser_attach_runtime_stream_path(dir.path(), receipt)
            .await
            .unwrap();
        let adapter_ipc = attached.get("adapter_ipc").unwrap();
        assert_eq!(
            adapter_ipc.get("path").and_then(|value| value.as_str()),
            Some(existing_path.as_str())
        );
        assert_ne!(
            adapter_ipc.get("path").and_then(|value| value.as_str()),
            adapter_ipc
                .get("runtime_stream_path")
                .and_then(|value| value.as_str())
        );
    }

    #[test]
    fn carrier_stream_request_uses_runtime_relay_open_target() {
        let route = BrowserCarrierExitRoute {
            connect_ticket: "ticket:test".to_string(),
            peer_did: Some("did:elastos:seed".to_string()),
            carrier_service: "elastos://exit/open_stream".to_string(),
            grant_id: "operator-grant:seed:test".to_string(),
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
        };
        let open_log = BrowserRelayOpenLog {
            schema: "elastos.exit.relay-open/v1".to_string(),
            stream_id: "stream:native-proxy:whatismyip:1".to_string(),
            target: "tls://www.whatismyip.com:443".to_string(),
            scheme: "tls".to_string(),
            host: "www.whatismyip.com".to_string(),
            reason: Some("Native browser proxy request".to_string()),
        };

        let request = browser_carrier_stream_request_for_open(&route, &open_log).unwrap();

        assert_eq!(request.stream_id, "stream:native-proxy:whatismyip:1");
        assert_eq!(request.target, "tls://www.whatismyip.com:443");
        assert_eq!(request.principal_id.as_deref(), Some("person:local:test"));
        assert_eq!(
            request.reason.as_deref(),
            Some("Native browser proxy request")
        );
    }

    #[test]
    fn carrier_stream_request_rejects_untyped_runtime_relay_open() {
        let route = BrowserCarrierExitRoute {
            connect_ticket: "ticket:test".to_string(),
            peer_did: None,
            carrier_service: "elastos://exit/open_stream".to_string(),
            grant_id: "operator-grant:seed:test".to_string(),
            principal_id: None,
            reason: None,
        };
        let open_log = BrowserRelayOpenLog {
            schema: "legacy".to_string(),
            stream_id: "stream:native-proxy:test:1".to_string(),
            target: "tls://www.whatismyip.com:443".to_string(),
            scheme: "tls".to_string(),
            host: "www.whatismyip.com".to_string(),
            reason: None,
        };

        let err = browser_carrier_stream_request_for_open(&route, &open_log).unwrap_err();

        assert!(err.to_string().contains("schema"));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_stream_line_kind_classifies_http_without_logging_payload() {
        assert_eq!(
            browser_runtime_stream_line_kind(b"GET /api/capability/pending HTTP/1.1\r\n"),
            "http"
        );
        assert_eq!(
            browser_runtime_stream_line_kind(br#"{"schema":"elastos.exit.relay-open/v1"}"#),
            "json"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn media_counted_stream_retains_both_direction_totals() {
        let (mut guest_client, guest_relay) = tokio::io::duplex(64);
        let (turn_relay, mut turn_server) = tokio::io::duplex(64);
        let mut guest_relay = BrowserMediaCountedStream::new(guest_relay);
        let (guest_to_turn, turn_to_guest) = guest_relay.byte_counts();
        let bridge = tokio::spawn(async move {
            let mut turn_relay = turn_relay;
            copy_bidirectional(&mut guest_relay, &mut turn_relay).await
        });

        guest_client.write_all(b"offer").await.unwrap();
        guest_client.shutdown().await.unwrap();
        let mut offer = [0_u8; 5];
        turn_server.read_exact(&mut offer).await.unwrap();
        assert_eq!(&offer, b"offer");

        turn_server.write_all(b"answer").await.unwrap();
        turn_server.shutdown().await.unwrap();
        let mut answer = [0_u8; 6];
        guest_client.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"answer");

        bridge.await.unwrap().unwrap();
        assert_eq!(guest_to_turn.load(Ordering::Relaxed), 5);
        assert_eq!(turn_to_guest.load(Ordering::Relaxed), 6);
    }

    #[cfg(unix)]
    #[test]
    fn media_diagnostic_budget_bounds_reconnect_churn() {
        let budget = BrowserMediaDiagnosticBudget::new(
            "sha256:generation".to_string(),
            "page:test".to_string(),
            "stream:media-test".to_string(),
        );
        let mut emitted = 0_u64;
        let mut summaries = 0_u64;
        let mut suppressed = 0_u64;

        for _ in 0..(BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT * 20) {
            match budget.next() {
                BrowserMediaDiagnosticDecision::Emit => emitted += 1,
                BrowserMediaDiagnosticDecision::SuppressionSummary => summaries += 1,
                BrowserMediaDiagnosticDecision::Suppressed => suppressed += 1,
            }
        }

        assert_eq!(emitted, BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT);
        assert_eq!(summaries, 1);
        assert_eq!(suppressed, BROWSER_MEDIA_DIAGNOSTIC_EVENT_LIMIT * 19 - 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_runtime_bridge_rejects_raw_http_before_exit_relay() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Some(BrowserExitRelay {
            path: dir.path().join("missing-exit.sock"),
        });
        let session_path = dir.path().join("runtime-stream.sock");
        let (mut browser_side, mut runtime_side) = UnixStream::pair().unwrap();
        browser_side
            .write_all(b"GET /api/capability/pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        browser_side.shutdown().await.unwrap();

        let err =
            bridge_browser_runtime_stream_to_local_relay(&session_path, &mut runtime_side, relay)
                .await
                .unwrap_err();

        assert!(
            err.to_string().contains("not JSON (http)"),
            "expected raw HTTP classification, got {err}"
        );
    }

    #[test]
    fn carrier_stream_request_rejects_private_targets_before_carrier_connect() {
        let route = BrowserCarrierExitRoute {
            connect_ticket: "ticket:test".to_string(),
            peer_did: None,
            carrier_service: "elastos://exit/open_stream".to_string(),
            grant_id: "operator-grant:seed:test".to_string(),
            principal_id: None,
            reason: None,
        };
        for (target, host) in [
            ("tcp://localhost:61180", "localhost"),
            ("tcp://127.0.0.1:80", "127.0.0.1"),
            ("tls://printer.local:443", "printer.local"),
        ] {
            let open_log = BrowserRelayOpenLog {
                schema: "elastos.exit.relay-open/v1".to_string(),
                stream_id: "stream:native-proxy:test:1".to_string(),
                target: target.to_string(),
                scheme: target
                    .split_once("://")
                    .map(|(scheme, _)| scheme)
                    .unwrap()
                    .to_string(),
                host: host.to_string(),
                reason: None,
            };

            let err = browser_carrier_stream_request_for_open(&route, &open_log).unwrap_err();

            assert!(
                err.to_string().contains("private"),
                "expected private-target rejection for {target}, got {err}"
            );
        }
    }

    #[cfg(unix)]
    struct MockGatewayCarrierExitProvider {
        relay_path: String,
    }

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl Provider for MockGatewayCarrierExitProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock gateway carrier exit provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-gateway-carrier-exit-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            assert_eq!(
                request.get("op").and_then(|value| value.as_str()),
                Some("open_stream")
            );
            assert_eq!(
                request.get("target").and_then(|value| value.as_str()),
                Some("tls://www.whatismyip.com:443")
            );
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "schema": EXIT_STREAM_SESSION_SCHEMA,
                    "backend": "remote-local-exit",
                    "stream_id": "stream:remote-local:test",
                    "target": "tls://www.whatismyip.com:443",
                    "byte_transport": "adapter_ipc",
                    "relay_ipc": {
                        "schema": "elastos.exit.relay-ipc/v1",
                        "kind": "unix_socket",
                        "path": self.relay_path,
                        "stream_id": "stream:remote-local:test"
                    }
                }
            }))
        }
    }

    #[cfg(unix)]
    fn browser_gateway_test_carrier_ticket(endpoint: &iroh::Endpoint) -> String {
        let mut watcher = endpoint.watch_addr();
        let addr = watcher.get();
        let ticket_json = serde_json::json!({
            "topic": null,
            "endpoints": [addr],
        });
        let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap_or_default();
        let mut ticket_str = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
        ticket_str.make_ascii_lowercase();
        ticket_str
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn carrier_runtime_bridge_forwards_relay_open_line_to_seed_exit() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let remote_dir = tempfile::tempdir().unwrap();
        let relay_path = remote_dir.path().join("remote-exit.sock");
        let relay_listener = UnixListener::bind(&relay_path).unwrap();
        let relay_task = tokio::spawn(async move {
            let (mut relay, _addr) = relay_listener.accept().await.unwrap();
            let mut open_line = Vec::new();
            loop {
                let mut byte = [0_u8; 1];
                relay.read_exact(&mut byte).await.unwrap();
                open_line.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let parsed = serde_json::from_slice::<serde_json::Value>(
                open_line
                    .strip_suffix(b"\n")
                    .unwrap_or(open_line.as_slice()),
            )
            .unwrap();
            assert_eq!(
                parsed.get("schema").and_then(|value| value.as_str()),
                Some("elastos.exit.relay-open/v1")
            );
            assert_eq!(
                parsed.get("target").and_then(|value| value.as_str()),
                Some("tls://www.whatismyip.com:443")
            );

            let mut request = [0_u8; 4];
            relay.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            relay.write_all(b"pong").await.unwrap();
        });

        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider(
                "exit",
                Arc::new(MockGatewayCarrierExitProvider {
                    relay_path: relay_path.to_string_lossy().to_string(),
                }),
            )
            .await
            .unwrap();
        let (remote_sk, remote_did) = elastos_identity::derive_did(&[57_u8; 32]);
        let remote_node = crate::carrier::start_carrier_node_with_registry(
            &remote_sk,
            &remote_did,
            remote_dir.path().to_path_buf(),
            Some(Arc::downgrade(&registry)),
        )
        .await
        .unwrap();
        let route = BrowserCarrierExitRoute {
            connect_ticket: browser_gateway_test_carrier_ticket(&remote_node.endpoint),
            peer_did: Some(remote_did),
            carrier_service: "elastos://exit/open_stream".to_string(),
            grant_id: "operator-grant:seed:test".to_string(),
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
        };

        let (mut browser_side, runtime_side) = UnixStream::pair().unwrap();
        let session_path = remote_dir.path().join("runtime-stream.sock");
        let bridge_task = tokio::spawn(async move {
            bridge_browser_runtime_stream_to_carrier(&session_path, runtime_side, route).await
        });
        let relay_open = serde_json::json!({
            "schema": "elastos.exit.relay-open/v1",
            "stream_id": "stream:native-proxy:whatismyip:1",
            "target": "tls://www.whatismyip.com:443",
            "scheme": "tls",
            "host": "www.whatismyip.com",
            "reason": "test Browser relay open"
        });
        browser_side
            .write_all(format!("{relay_open}\n").as_bytes())
            .await
            .unwrap();
        browser_side.write_all(b"ping").await.unwrap();
        browser_side.shutdown().await.unwrap();

        let mut response = [0_u8; 4];
        browser_side.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");

        bridge_task.await.unwrap().unwrap();
        relay_task.await.unwrap();
        remote_node.endpoint.close().await;
    }
}
