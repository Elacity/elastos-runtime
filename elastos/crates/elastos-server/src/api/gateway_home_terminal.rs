use super::*;

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rand::RngCore;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex, Notify};

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

const HOME_CLI_CAPSULE_ID: &str = "home-cli";
const HOME_TERMINAL_CONTRACT_SCHEMA: &str = "elastos.home-cli.terminal-contract/v1";
const HOME_TERMINAL_START_SCHEMA: &str = "elastos.home-cli.terminal-start/v1";
const HOME_TERMINAL_SESSION_SCHEMA: &str = "elastos.home-cli.terminal-session/v1";
const HOME_TERMINAL_RESIZE_SCHEMA: &str = "elastos.home-cli.terminal-resize/v1";
const HOME_TERMINAL_CLOSE_SCHEMA: &str = "elastos.home-cli.terminal-close/v1";
const HOME_TERMINAL_EVENT_SCHEMA: &str = "elastos.home-cli.terminal-event/v1";
const HOME_TERMINAL_HOST_INTENT_SCHEMA: &str = "elastos.home.terminal-host-intent/v1";
const HOME_TERMINAL_INTENT_SCHEMA: &str = "elastos.home-cli.terminal-intent/v1";
const HOME_TERMINAL_EVENT_KEEPALIVE_SECS: u64 = 15;
const HOME_TERMINAL_EVENT_DISCONNECT_GRACE_SECS: u64 = 3;
const HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS: u64 = 20;
const HOME_TERMINAL_EXIT_AUTH_GRACE_MS: u64 = 2_000;
const HOME_TERMINAL_REPLAY_RETENTION_SECS: u64 = 30;
const HOME_TERMINAL_MAX_ACTIVE_SESSIONS: usize = 8;
const HOME_TERMINAL_MAX_SESSIONS_PER_PRINCIPAL: usize = 4;
const HOME_TERMINAL_MAX_SESSIONS_PER_AUTH_SESSION: usize = 1;
const HOME_TERMINAL_REPLAY_MAX_EVENTS: usize = 64;
const HOME_TERMINAL_REPLAY_MAX_BYTES: usize = 64 * 1024;
const HOME_TERMINAL_ARCHIVED_REPLAY_MAX_SESSIONS: usize = 8;
const HOME_TERMINAL_ARCHIVED_REPLAY_MAX_BYTES: usize = HOME_TERMINAL_REPLAY_MAX_BYTES * 8;
pub(super) const HOME_TERMINAL_INPUT_MAX_BYTES: usize = 16 * 1024;
pub(super) const HOME_TERMINAL_INTENT_MAX_BYTES: usize = 8 * 1024;
const HOME_TERMINAL_PROGRAM_ENV: &str = "ELASTOS_HOME_CLI_TERMINAL_PROGRAM";
const HOME_TERMINAL_ARGS_ENV: &str = "ELASTOS_HOME_CLI_TERMINAL_ARGS_JSON";
const HOME_TERMINAL_DEFAULT_COLS: u16 = 100;
const HOME_TERMINAL_DEFAULT_ROWS: u16 = 32;
const HOME_TERMINAL_MIN_COLS: u16 = 40;
const HOME_TERMINAL_MIN_ROWS: u16 = 12;
const HOME_TERMINAL_MAX_COLS: u16 = 180;
const HOME_TERMINAL_MAX_ROWS: u16 = 80;

static HOME_TERMINAL_SESSIONS: OnceLock<Mutex<HashMap<String, Arc<HomeTerminalSession>>>> =
    OnceLock::new();
static HOME_TERMINAL_ARCHIVED_REPLAYS: OnceLock<StdMutex<HomeTerminalArchivedReplayStore>> =
    OnceLock::new();

#[derive(Debug, Deserialize)]
pub(super) struct HomeTerminalStreamQuery {
    ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeTerminalResizeRequest {
    schema: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeTerminalHostIntentRequest {
    schema: Option<String>,
    action: Option<String>,
    target: Option<String>,
    action_id: Option<String>,
    source: Option<String>,
    contact_id: Option<String>,
    route: Option<String>,
    query: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeTerminalStartRequest {
    schema: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Debug, Clone, Copy)]
struct HomeTerminalSize {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Clone, Serialize)]
struct HomeTerminalContract {
    schema: String,
    renderer_contract: String,
    transport: String,
    transport_scope: String,
    pty: String,
    protocol: String,
    start: HomeTerminalEndpoint,
    events: HomeTerminalEndpoint,
    input: HomeTerminalEndpoint,
    resize: HomeTerminalEndpoint,
    intent: HomeTerminalEndpoint,
    close: HomeTerminalEndpoint,
    authority: String,
    process: String,
}

#[derive(Debug, Clone, Serialize)]
struct HomeTerminalEndpoint {
    method: &'static str,
    route: &'static str,
    auth: &'static str,
}

#[derive(Clone)]
struct HomeTerminalCommand {
    program: String,
    args: Vec<String>,
    label: String,
}

struct HomeTerminalSession {
    session_id: String,
    stream_ticket: String,
    input_ticket: String,
    principal_id: String,
    auth_session_id: String,
    grant_id: String,
    child_pid: Option<u32>,
    created_at_ms: u64,
    input: Mutex<Option<HomeTerminalInput>>,
    child: Mutex<Child>,
    events: broadcast::Sender<HomeTerminalBroadcastEvent>,
    replay: StdMutex<HomeTerminalReplayLog>,
    event_stream_generation: AtomicU64,
    input_stream_generation: AtomicU64,
    pty_reader_drained: AtomicBool,
    pty_reader_drained_notify: Notify,
}

enum HomeTerminalInput {
    #[cfg(unix)]
    Pty(File),
}

#[derive(Debug, Clone, Copy)]
struct HomeTerminalSessionLimits {
    total: usize,
    per_principal: usize,
    per_auth_session: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HomeTerminalSessionLifecycle {
    session_id: String,
    principal_id: String,
    auth_session_id: String,
    grant_id: String,
    created_at_ms: u64,
    event_stream_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HomeTerminalStartPlan {
    stale_session_ids: Vec<String>,
    replaced_session_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HomeTerminalCapacityError {
    message: String,
}

struct HomeTerminalCleanupTarget {
    session: Arc<HomeTerminalSession>,
    message: &'static str,
}

enum HomeTerminalStartError {
    Capacity(HomeTerminalCapacityError),
    Runtime(anyhow::Error),
}

impl From<anyhow::Error> for HomeTerminalStartError {
    fn from(err: anyhow::Error) -> Self {
        Self::Runtime(err)
    }
}

#[derive(Debug, Clone, Serialize)]
struct HomeTerminalEvent {
    schema: &'static str,
    session_id: String,
    stream: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Clone)]
struct HomeTerminalBroadcastEvent {
    id: u64,
    payload: HomeTerminalEvent,
}

#[derive(Debug, Clone)]
struct HomeTerminalReplayEntry {
    id: u64,
    payload: HomeTerminalEvent,
    bytes: usize,
}

#[derive(Default)]
struct HomeTerminalReplayLog {
    next_event_id: u64,
    events: VecDeque<HomeTerminalReplayEntry>,
    total_bytes: usize,
}

struct HomeTerminalArchivedReplay {
    stream_ticket: String,
    expires_at_ms: u64,
    replay: VecDeque<HomeTerminalReplayEntry>,
    total_bytes: usize,
}

#[derive(Default)]
struct HomeTerminalArchivedReplayStore {
    replays: HashMap<String, HomeTerminalArchivedReplay>,
    insertion_order: VecDeque<String>,
    total_bytes: usize,
}

impl HomeTerminalArchivedReplayStore {
    fn insert(&mut self, session_id: String, replay: HomeTerminalArchivedReplay, now_ms: u64) {
        self.retain_unexpired(now_ms);
        self.remove(&session_id);
        self.total_bytes = self.total_bytes.saturating_add(replay.total_bytes);
        self.insertion_order.push_back(session_id.clone());
        self.replays.insert(session_id, replay);
        while self.replays.len() > HOME_TERMINAL_ARCHIVED_REPLAY_MAX_SESSIONS
            || self.total_bytes > HOME_TERMINAL_ARCHIVED_REPLAY_MAX_BYTES
        {
            let Some(oldest_session_id) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.replays.remove(&oldest_session_id) {
                self.total_bytes = self.total_bytes.saturating_sub(removed.total_bytes);
            }
        }
    }

    fn retain_unexpired(&mut self, now_ms: u64) {
        self.insertion_order.retain(|session_id| {
            let keep = self
                .replays
                .get(session_id)
                .is_some_and(|replay| replay.expires_at_ms >= now_ms);
            if !keep {
                if let Some(removed) = self.replays.remove(session_id) {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.total_bytes);
                }
            }
            keep
        });
    }

    fn remove(&mut self, session_id: &str) -> Option<HomeTerminalArchivedReplay> {
        self.insertion_order.retain(|entry| entry != session_id);
        let removed = self.replays.remove(session_id)?;
        self.total_bytes = self.total_bytes.saturating_sub(removed.total_bytes);
        Some(removed)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct HomeTerminalAuthorizedIntent {
    schema: &'static str,
    action: String,
    target: String,
    action_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<serde_json::Value>,
}

pub(super) async fn home_cli_terminal_contract() -> Response {
    Json(HomeTerminalContract {
        schema: HOME_TERMINAL_CONTRACT_SCHEMA.to_string(),
        renderer_contract: "capsule-local xterm.js terminal over a Runtime-owned byte-stream contract".to_string(),
        transport: "runtime_pty_stream".to_string(),
        transport_scope: "local_runtime_adapter".to_string(),
        pty: "Runtime-owned PTY; xterm sends input bytes and renders PTY output without direct host process authority".to_string(),
        protocol: "SSE PTY output + WebSocket input + HTTP resize".to_string(),
        start: HomeTerminalEndpoint {
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions",
            auth: "home-cli launch token header",
        },
        events: HomeTerminalEndpoint {
            method: "GET",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/events?ticket=...",
            auth: "session stream ticket",
        },
        input: HomeTerminalEndpoint {
            method: "GET",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/input?ticket=...",
            auth: "session input ticket",
        },
        resize: HomeTerminalEndpoint {
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/resize",
            auth: "same home-cli launch token context that created the session",
        },
        intent: HomeTerminalEndpoint {
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/intent",
            auth: "same home-cli launch token context that created the session",
        },
        close: HomeTerminalEndpoint {
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/close",
            auth: "same home-cli launch token context that created the session",
        },
        authority: "Runtime owns the process, stream ticket, input gate, and lifecycle; the capsule only renders bytes and sends typed input.".to_string(),
        process: "elastos home".to_string(),
    })
    .into_response()
}

pub(super) async fn home_cli_terminal_start(
    State(state): State<GatewayState>,
    Extension(gateway_api_url): Extension<TrustedGatewayApiUrl>,
    headers: HeaderMap,
    start: Option<Json<HomeTerminalStartRequest>>,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_CLI_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let size = match terminal_start_size(start.map(|Json(start)| start)) {
        Ok(size) => size,
        Err(response) => return response,
    };
    match start_home_terminal_session(context, size, Some(gateway_api_url.0.to_string())).await {
        Ok(session) => Json(serde_json::json!({
            "schema": HOME_TERMINAL_SESSION_SCHEMA,
            "session_id": session.session_id,
            "transport": "runtime_pty_stream",
            "transport_scope": "local_runtime_adapter",
            "pty": true,
            "renderer_contract": "capsule-local xterm.js terminal over a Runtime-owned byte-stream contract",
            "dimensions": {
                "cols": size.cols,
                "rows": size.rows
            },
            "stream": {
                "schema": "elastos.runtime.stream/v1",
                "events_url": format!("/api/apps/home-cli/terminal/sessions/{}/events?ticket={}", session.session_id, session.stream_ticket),
                "input_socket_url": format!("/api/apps/home-cli/terminal/sessions/{}/input?ticket={}", session.session_id, session.input_ticket),
                "resize_url": format!("/api/apps/home-cli/terminal/sessions/{}/resize", session.session_id),
                "intent_url": format!("/api/apps/home-cli/terminal/sessions/{}/intent", session.session_id),
                "close_url": format!("/api/apps/home-cli/terminal/sessions/{}/close", session.session_id),
                "resize_schema": HOME_TERMINAL_RESIZE_SCHEMA,
                "intent_schema": HOME_TERMINAL_INTENT_SCHEMA,
                "event_schema": HOME_TERMINAL_EVENT_SCHEMA
            },
            "process": {
                "label": "elastos home",
                "argv": ["elastos", "home"],
                "mode": "tui"
            },
            "authority": {
                "app": HOME_CLI_CAPSULE_ID,
                "principal_id": session.principal_id
            }
        }))
        .into_response(),
        Err(HomeTerminalStartError::Capacity(err)) => home_terminal_capacity_response(err),
        Err(HomeTerminalStartError::Runtime(err)) => home_error_response(err),
    }
}

pub(super) async fn home_cli_terminal_events(
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<HomeTerminalStreamQuery>,
) -> Response {
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    cleanup_expired_home_terminal_archived_replays(now_unix_ms());
    let last_event_id = home_terminal_last_event_id(&headers);
    let session = match home_terminal_session(&session_id).await {
        Some(session) => session,
        None => {
            return archived_home_terminal_events_response(
                &session_id,
                query.ticket.as_deref(),
                last_event_id,
                now_unix_ms(),
            )
            .unwrap_or_else(|| {
                (StatusCode::NOT_FOUND, "terminal session not found").into_response()
            });
        }
    };
    if query.ticket.as_deref() != Some(session.stream_ticket.as_str()) {
        return (StatusCode::FORBIDDEN, "invalid terminal stream ticket").into_response();
    }
    let receiver = session.events.subscribe();
    let replay = home_terminal_replay_after(&session, last_event_id);
    let generation = session
        .event_stream_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    let state = HomeTerminalEventStreamState {
        receiver,
        replay,
        last_delivered_event_id: last_event_id.unwrap_or(0),
        _guard: HomeTerminalEventStreamGuard {
            session_id: session_id.clone(),
            generation,
        },
    };
    let stream = futures_lite::stream::unfold(state, |mut state| async move {
        if let Some(event) = next_home_terminal_stream_event(
            &mut state.replay,
            &mut state.last_delivered_event_id,
            &mut state.receiver,
        )
        .await
        {
            return Some((
                Ok::<SseEvent, Infallible>(home_terminal_sse_event(event)),
                state,
            ));
        }
        None
    });

    home_terminal_sse_response(
        Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(HOME_TERMINAL_EVENT_KEEPALIVE_SECS))
                    .text("keepalive"),
            )
            .into_response(),
    )
}

pub(super) async fn home_cli_terminal_input_socket(
    Path(session_id): Path<String>,
    Query(query): Query<HomeTerminalStreamQuery>,
    socket: WebSocketUpgrade,
) -> Response {
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    let session = match home_terminal_session(&session_id).await {
        Some(session) => session,
        None => return (StatusCode::NOT_FOUND, "terminal session not found").into_response(),
    };
    if !home_terminal_input_ticket_matches(query.ticket.as_deref(), &session.input_ticket) {
        return (StatusCode::FORBIDDEN, "invalid terminal input ticket").into_response();
    }
    let generation = session
        .input_stream_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    socket.on_upgrade(move |socket| home_terminal_input_socket(socket, session, generation))
}

pub(super) fn home_terminal_input_ticket_matches(presented: Option<&str>, expected: &str) -> bool {
    presented == Some(expected)
}

async fn home_terminal_input_socket(
    mut socket: WebSocket,
    session: Arc<HomeTerminalSession>,
    generation: u64,
) {
    while let Some(Ok(message)) = socket.recv().await {
        if session.input_stream_generation.load(Ordering::Acquire) != generation {
            break;
        }
        let data = match message {
            WebSocketMessage::Text(data) => data.into_bytes(),
            WebSocketMessage::Binary(data) => data,
            WebSocketMessage::Close(_) => break,
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => continue,
        };
        if data.is_empty() {
            continue;
        }
        if data.len() > HOME_TERMINAL_INPUT_MAX_BYTES {
            break;
        }
        let mut input = session.input.lock().await;
        let Some(input) = input.as_mut() else {
            break;
        };
        if input.write_all(&data).await.is_err() {
            break;
        }
    }
}

pub(super) async fn home_cli_terminal_resize(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(resize): Json<HomeTerminalResizeRequest>,
) -> Response {
    if let Some(schema) = resize.schema.as_deref() {
        if schema != HOME_TERMINAL_RESIZE_SCHEMA {
            return (
                StatusCode::BAD_REQUEST,
                "unsupported terminal resize schema",
            )
                .into_response();
        }
    }
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_CLI_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let session = match authorized_terminal_session(&session_id, &context).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let size = terminal_size_from_parts(resize.cols, resize.rows);
    let mut input_handle = session.input.lock().await;
    let Some(input_handle) = input_handle.as_mut() else {
        return (StatusCode::GONE, "terminal input is closed").into_response();
    };
    if let Err(err) = input_handle.resize(size) {
        return home_error_response(anyhow::anyhow!("terminal resize failed: {err}"));
    }
    Json(serde_json::json!({
        "schema": HOME_TERMINAL_RESIZE_SCHEMA,
        "session_id": session_id,
        "dimensions": {
            "cols": size.cols,
            "rows": size.rows
        }
    }))
    .into_response()
}

pub(super) async fn home_cli_terminal_intent(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(intent): Json<HomeTerminalHostIntentRequest>,
) -> Response {
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_CLI_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let session = match authorized_terminal_session(&session_id, &context).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let intent = match authorize_home_terminal_host_intent(intent) {
        Ok(intent) => intent,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    Json(serde_json::json!({
        "schema": HOME_TERMINAL_INTENT_SCHEMA,
        "session_id": session.session_id,
        "intent": intent,
    }))
    .into_response()
}

pub(super) async fn home_cli_terminal_close(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[HOME_CLI_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let session = match authorized_terminal_session(&session_id, &context).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    close_home_terminal_session(&session.session_id, "closed by Home CLI").await;
    Json(serde_json::json!({
        "schema": HOME_TERMINAL_CLOSE_SCHEMA,
        "session_id": session_id,
        "status": "closed"
    }))
    .into_response()
}

#[cfg(not(unix))]
async fn start_home_terminal_session(
    _context: HomeLaunchTokenContext,
    _size: HomeTerminalSize,
    _gateway_api_url: Option<String>,
) -> Result<Arc<HomeTerminalSession>, HomeTerminalStartError> {
    Err(HomeTerminalStartError::Runtime(anyhow::anyhow!(
        "Runtime PTY terminal is not supported on this platform"
    )))
}

#[cfg(unix)]
async fn start_home_terminal_session(
    context: HomeLaunchTokenContext,
    size: HomeTerminalSize,
    gateway_api_url: Option<String>,
) -> Result<Arc<HomeTerminalSession>, HomeTerminalStartError> {
    let cleanup = prepare_home_terminal_start(&context, now_unix_ms())
        .await
        .map_err(HomeTerminalStartError::Capacity)?;
    close_home_terminal_cleanup_targets(cleanup).await;

    let command_spec = home_terminal_command()?;
    let pty = open_home_terminal_pty(size)?;
    let mut command = Command::new(&command_spec.program);
    command
        .args(&command_spec.args)
        .stdin(Stdio::from(pty.slave_stdin))
        .stdout(Stdio::from(pty.slave_stdout))
        .stderr(Stdio::from(pty.slave_stderr))
        .kill_on_drop(true)
        .env("ELASTOS_HOME_TERMINAL", "1")
        .env("ELASTOS_HOME_TUI", "1")
        .env(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "1")
        .env("ELASTOS_TERM_COLS", size.cols.to_string())
        .env("ELASTOS_TERM_ROWS", size.rows.to_string())
        .env("ELASTOS_QUIET_RUNTIME_NOTICES", "1")
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    for (key, value) in home_terminal_authority_env(&context) {
        command.env(key, value);
    }
    if let Some(gateway_api_url) = gateway_api_url {
        command.env(HOME_CLI_GATEWAY_API_URL_ENV, gateway_api_url);
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command
        .spawn()
        .with_context(|| format!("failed to start terminal process {}", command_spec.label))?;
    let child_pid = child.id();
    let (events, _receiver) = broadcast::channel(256);
    let session = Arc::new(HomeTerminalSession {
        session_id: format!("term-{}", random_hex_token()),
        stream_ticket: format!("ticket-{}", random_hex_token()),
        input_ticket: format!("input-{}", random_hex_token()),
        principal_id: context.principal_id,
        auth_session_id: context.session_id,
        grant_id: context.grant_id,
        child_pid,
        created_at_ms: now_unix_ms(),
        input: Mutex::new(Some(pty.input)),
        child: Mutex::new(child),
        events,
        replay: StdMutex::new(HomeTerminalReplayLog::default()),
        event_stream_generation: AtomicU64::new(0),
        input_stream_generation: AtomicU64::new(0),
        pty_reader_drained: AtomicBool::new(false),
        pty_reader_drained_notify: Notify::new(),
    });
    insert_home_terminal_session(session.clone()).await;
    spawn_home_terminal_pty_reader(session.clone(), pty.reader);
    spawn_home_terminal_waiter(session.clone());
    spawn_home_terminal_attach_watchdog(session.clone());
    emit_home_terminal_event(
        &session,
        HomeTerminalEvent {
            schema: HOME_TERMINAL_EVENT_SCHEMA,
            session_id: session.session_id.clone(),
            stream: "lifecycle",
            data: None,
            exit_code: None,
            message: Some("started".to_string()),
        },
    );
    Ok(session)
}

#[cfg(unix)]
struct HomeTerminalPty {
    input: HomeTerminalInput,
    reader: File,
    slave_stdin: File,
    slave_stdout: File,
    slave_stderr: File,
}

#[cfg(unix)]
fn open_home_terminal_pty(size: HomeTerminalSize) -> anyhow::Result<HomeTerminalPty> {
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let winsize = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    #[cfg(target_os = "macos")]
    let mut winsize = winsize;
    #[cfg(target_os = "macos")]
    let winsize_ptr = &mut winsize;
    #[cfg(not(target_os = "macos"))]
    let winsize_ptr = &winsize;
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            winsize_ptr,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error()).context("open Runtime Home terminal PTY");
    }

    let master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    Ok(HomeTerminalPty {
        input: HomeTerminalInput::Pty(master.try_clone().context("clone PTY master for input")?),
        reader: master,
        slave_stdin: slave.try_clone().context("clone PTY slave for stdin")?,
        slave_stdout: slave.try_clone().context("clone PTY slave for stdout")?,
        slave_stderr: slave,
    })
}

impl HomeTerminalInput {
    async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            HomeTerminalInput::Pty(file) => {
                let mut writer = file.try_clone()?;
                let data = data.to_vec();
                tokio::task::spawn_blocking(move || writer.write_all(&data))
                    .await
                    .map_err(io::Error::other)?
            }
        }
    }

    fn resize(&mut self, size: HomeTerminalSize) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            HomeTerminalInput::Pty(file) => {
                let winsize = libc::winsize {
                    ws_row: size.rows,
                    ws_col: size.cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                let rc = unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &winsize) };
                if rc == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
        }
    }
}

#[allow(clippy::result_large_err)]
fn terminal_start_size(
    start: Option<HomeTerminalStartRequest>,
) -> Result<HomeTerminalSize, Response> {
    let Some(start) = start else {
        return Ok(HomeTerminalSize {
            cols: HOME_TERMINAL_DEFAULT_COLS,
            rows: HOME_TERMINAL_DEFAULT_ROWS,
        });
    };
    if let Some(schema) = start.schema.as_deref() {
        if schema != HOME_TERMINAL_START_SCHEMA {
            return Err(
                (StatusCode::BAD_REQUEST, "unsupported terminal start schema").into_response(),
            );
        }
    }
    Ok(terminal_size_from_parts(start.cols, start.rows))
}

fn terminal_size_from_parts(cols: Option<u16>, rows: Option<u16>) -> HomeTerminalSize {
    HomeTerminalSize {
        cols: cols
            .unwrap_or(HOME_TERMINAL_DEFAULT_COLS)
            .clamp(HOME_TERMINAL_MIN_COLS, HOME_TERMINAL_MAX_COLS),
        rows: rows
            .unwrap_or(HOME_TERMINAL_DEFAULT_ROWS)
            .clamp(HOME_TERMINAL_MIN_ROWS, HOME_TERMINAL_MAX_ROWS),
    }
}

fn home_terminal_command() -> anyhow::Result<HomeTerminalCommand> {
    if let Ok(program) = std::env::var(HOME_TERMINAL_PROGRAM_ENV) {
        let program = program.trim().to_string();
        if program.is_empty() {
            anyhow::bail!("{HOME_TERMINAL_PROGRAM_ENV} is empty");
        }
        let args = match std::env::var(HOME_TERMINAL_ARGS_ENV) {
            Ok(raw) if !raw.trim().is_empty() => serde_json::from_str::<Vec<String>>(&raw)
                .with_context(|| format!("parse {HOME_TERMINAL_ARGS_ENV}"))?,
            _ => Vec::new(),
        };
        return Ok(HomeTerminalCommand {
            label: "configured terminal command".to_string(),
            program,
            args,
        });
    }
    let program = std::env::current_exe()
        .context("resolve current elastos binary for Home terminal")?
        .to_string_lossy()
        .to_string();
    Ok(HomeTerminalCommand {
        program,
        args: vec!["home".to_string()],
        label: "elastos home".to_string(),
    })
}

#[cfg(unix)]
fn spawn_home_terminal_pty_reader(session: Arc<HomeTerminalSession>, mut reader: File) {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 4096];
        let mut decoder = HomeTerminalUtf8Decoder::default();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Some(data) = decoder.flush_lossy() {
                        send_home_terminal_stdout(&session, data);
                    }
                    break;
                }
                Ok(n) => {
                    if let Some(data) = decoder.push(&buffer[..n]) {
                        send_home_terminal_stdout(&session, data);
                    }
                }
                Err(err) => {
                    emit_home_terminal_event(
                        &session,
                        HomeTerminalEvent {
                            schema: HOME_TERMINAL_EVENT_SCHEMA,
                            session_id: session.session_id.clone(),
                            stream: "error",
                            data: None,
                            exit_code: None,
                            message: Some(format!("PTY read failed: {err}")),
                        },
                    );
                    break;
                }
            }
        }
        mark_home_terminal_pty_reader_drained(&session);
    });
}

fn mark_home_terminal_pty_reader_drained(session: &HomeTerminalSession) {
    session.pty_reader_drained.store(true, Ordering::SeqCst);
    session.pty_reader_drained_notify.notify_waiters();
}

#[derive(Default)]
struct HomeTerminalUtf8Decoder {
    pending: Vec<u8>,
}

impl HomeTerminalUtf8Decoder {
    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    output.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&self.pending[..valid_up_to])
                            .expect("valid_up_to marks a valid UTF-8 prefix")
                            .to_string();
                        output.push_str(&valid);
                        self.pending.drain(..valid_up_to);
                        continue;
                    }

                    let Some(invalid_len) = err.error_len() else {
                        break;
                    };
                    output.push_str(&String::from_utf8_lossy(&self.pending[..invalid_len]));
                    self.pending.drain(..invalid_len);
                }
            }
        }

        (!output.is_empty()).then_some(output)
    }

    fn flush_lossy(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let output = String::from_utf8_lossy(&self.pending).to_string();
        self.pending.clear();
        Some(output)
    }
}

fn send_home_terminal_stdout(session: &HomeTerminalSession, data: String) {
    emit_home_terminal_event(
        session,
        HomeTerminalEvent {
            schema: HOME_TERMINAL_EVENT_SCHEMA,
            session_id: session.session_id.clone(),
            stream: "stdout",
            data: Some(data),
            exit_code: None,
            message: None,
        },
    );
}

fn spawn_home_terminal_waiter(session: Arc<HomeTerminalSession>) {
    tokio::spawn(async move {
        let status = session.child.lock().await.wait().await;
        {
            let mut input_handle = session.input.lock().await;
            input_handle.take();
        }
        wait_home_terminal_pty_reader_drained(&session).await;
        let (exit_code, message) = match status {
            Ok(status) => (status.code(), "exited".to_string()),
            Err(err) => (None, format!("wait failed: {err}")),
        };
        emit_home_terminal_event(
            &session,
            HomeTerminalEvent {
                schema: HOME_TERMINAL_EVENT_SCHEMA,
                session_id: session.session_id.clone(),
                stream: "lifecycle",
                data: None,
                exit_code,
                message: Some(message),
            },
        );
        tokio::time::sleep(Duration::from_millis(HOME_TERMINAL_EXIT_AUTH_GRACE_MS)).await;
        if let Some(removed) = remove_home_terminal_session(&session.session_id).await {
            store_archived_home_terminal_replay(&removed, now_unix_ms());
        }
    });
}

async fn wait_home_terminal_pty_reader_drained(session: &HomeTerminalSession) {
    if session.pty_reader_drained.load(Ordering::SeqCst) {
        return;
    }
    let notified = session.pty_reader_drained_notify.notified();
    tokio::pin!(notified);
    tokio::select! {
        _ = &mut notified => {}
        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
    }
}

fn spawn_home_terminal_attach_watchdog(session: Arc<HomeTerminalSession>) {
    let session_id = session.session_id.clone();
    let created_at_ms = session.created_at_ms;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(
            HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS,
        ))
        .await;
        let Some(session) = home_terminal_session(&session_id).await else {
            return;
        };
        let lifecycle = home_terminal_session_lifecycle(&session);
        if lifecycle.created_at_ms == created_at_ms
            && home_terminal_session_is_stale(&lifecycle, now_unix_ms())
        {
            close_home_terminal_session(&session_id, "closed stale unattached terminal session")
                .await;
        }
    });
}

struct HomeTerminalEventStreamState {
    receiver: broadcast::Receiver<HomeTerminalBroadcastEvent>,
    replay: VecDeque<HomeTerminalBroadcastEvent>,
    last_delivered_event_id: u64,
    _guard: HomeTerminalEventStreamGuard,
}

struct HomeTerminalEventStreamGuard {
    session_id: String,
    generation: u64,
}

impl Drop for HomeTerminalEventStreamGuard {
    fn drop(&mut self) {
        let session_id = self.session_id.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(
                HOME_TERMINAL_EVENT_DISCONNECT_GRACE_SECS,
            ))
            .await;
            let Some(session) = home_terminal_session(&session_id).await else {
                return;
            };
            if session.event_stream_generation.load(Ordering::Acquire) == generation {
                close_home_terminal_session(&session_id, "closed after terminal stream disconnect")
                    .await;
            }
        });
    }
}

async fn prepare_home_terminal_start(
    context: &HomeLaunchTokenContext,
    now_ms: u64,
) -> Result<Vec<HomeTerminalCleanupTarget>, HomeTerminalCapacityError> {
    let mut sessions = home_terminal_sessions().lock().await;
    let lifecycles = sessions
        .values()
        .map(|session| home_terminal_session_lifecycle(session))
        .collect::<Vec<_>>();
    let plan =
        home_terminal_start_plan(&lifecycles, context, now_ms, home_terminal_session_limits())?;
    let mut cleanup = Vec::new();
    for session_id in plan.stale_session_ids {
        if let Some(session) = sessions.remove(&session_id) {
            cleanup.push(HomeTerminalCleanupTarget {
                session,
                message: "closed stale unattached terminal session",
            });
        }
    }
    for session_id in plan.replaced_session_ids {
        if let Some(session) = sessions.remove(&session_id) {
            cleanup.push(HomeTerminalCleanupTarget {
                session,
                message: "closed by replacement terminal session",
            });
        }
    }
    Ok(cleanup)
}

async fn cleanup_stale_home_terminal_sessions(now_ms: u64) {
    let mut sessions = home_terminal_sessions().lock().await;
    let stale_session_ids = sessions
        .values()
        .map(|session| home_terminal_session_lifecycle(session))
        .filter(|session| home_terminal_session_is_stale(session, now_ms))
        .map(|session| session.session_id)
        .collect::<Vec<_>>();
    let cleanup = stale_session_ids
        .into_iter()
        .filter_map(|session_id| {
            sessions
                .remove(&session_id)
                .map(|session| HomeTerminalCleanupTarget {
                    session,
                    message: "closed stale unattached terminal session",
                })
        })
        .collect::<Vec<_>>();
    drop(sessions);
    close_home_terminal_cleanup_targets(cleanup).await;
}

async fn close_home_terminal_cleanup_targets(targets: Vec<HomeTerminalCleanupTarget>) {
    for target in targets {
        close_home_terminal_session_handle(target.session, target.message).await;
    }
}

fn home_terminal_session_limits() -> HomeTerminalSessionLimits {
    HomeTerminalSessionLimits {
        total: HOME_TERMINAL_MAX_ACTIVE_SESSIONS,
        per_principal: HOME_TERMINAL_MAX_SESSIONS_PER_PRINCIPAL,
        per_auth_session: HOME_TERMINAL_MAX_SESSIONS_PER_AUTH_SESSION,
    }
}

fn home_terminal_start_plan(
    sessions: &[HomeTerminalSessionLifecycle],
    context: &HomeLaunchTokenContext,
    now_ms: u64,
    limits: HomeTerminalSessionLimits,
) -> Result<HomeTerminalStartPlan, HomeTerminalCapacityError> {
    let stale_session_ids = sessions
        .iter()
        .filter(|session| home_terminal_session_is_stale(session, now_ms))
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    let replaced_session_ids = sessions
        .iter()
        .filter(|session| !stale_session_ids.contains(&session.session_id))
        .filter(|session| home_terminal_session_matches_context(session, context))
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    let active = sessions
        .iter()
        .filter(|session| !stale_session_ids.contains(&session.session_id))
        .filter(|session| !replaced_session_ids.contains(&session.session_id))
        .collect::<Vec<_>>();

    if active.len() >= limits.total {
        return Err(HomeTerminalCapacityError {
            message: format!(
                "Home CLI terminal capacity unavailable: {}/{} Runtime PTY sessions are active",
                active.len(),
                limits.total
            ),
        });
    }

    let principal_count = active
        .iter()
        .filter(|session| session.principal_id == context.principal_id)
        .count();
    if principal_count >= limits.per_principal {
        return Err(HomeTerminalCapacityError {
            message: format!(
                "Home CLI terminal principal limit reached: {principal_count}/{} Runtime PTY sessions are active for this principal",
                limits.per_principal
            ),
        });
    }

    let auth_session_count = active
        .iter()
        .filter(|session| {
            session.principal_id == context.principal_id
                && session.auth_session_id == context.session_id
        })
        .count();
    if auth_session_count >= limits.per_auth_session {
        return Err(HomeTerminalCapacityError {
            message: format!(
                "Home CLI terminal session limit reached: {auth_session_count}/{} Runtime PTY sessions are active for this Home session",
                limits.per_auth_session
            ),
        });
    }

    Ok(HomeTerminalStartPlan {
        stale_session_ids,
        replaced_session_ids,
    })
}

fn home_terminal_session_is_stale(session: &HomeTerminalSessionLifecycle, now_ms: u64) -> bool {
    session.event_stream_generation == 0
        && now_ms.saturating_sub(session.created_at_ms)
            >= HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS * 1_000
}

fn home_terminal_session_matches_context(
    session: &HomeTerminalSessionLifecycle,
    context: &HomeLaunchTokenContext,
) -> bool {
    session.principal_id == context.principal_id
        && session.auth_session_id == context.session_id
        && session.grant_id == context.grant_id
}

fn home_terminal_session_lifecycle(session: &HomeTerminalSession) -> HomeTerminalSessionLifecycle {
    HomeTerminalSessionLifecycle {
        session_id: session.session_id.clone(),
        principal_id: session.principal_id.clone(),
        auth_session_id: session.auth_session_id.clone(),
        grant_id: session.grant_id.clone(),
        created_at_ms: session.created_at_ms,
        event_stream_generation: session.event_stream_generation.load(Ordering::Acquire),
    }
}

fn home_terminal_capacity_response(err: HomeTerminalCapacityError) -> Response {
    (StatusCode::TOO_MANY_REQUESTS, err.message).into_response()
}

async fn authorized_terminal_session(
    session_id: &str,
    context: &HomeLaunchTokenContext,
) -> Result<Arc<HomeTerminalSession>, Response> {
    let Some(session) = home_terminal_session(session_id).await else {
        return Err((StatusCode::NOT_FOUND, "terminal session not found").into_response());
    };
    let lifecycle = home_terminal_session_lifecycle(&session);
    if !home_terminal_session_matches_context(&lifecycle, context) {
        return Err((
            StatusCode::FORBIDDEN,
            "terminal session belongs to another launch context",
        )
            .into_response());
    }
    Ok(session)
}

fn home_terminal_sessions() -> &'static Mutex<HashMap<String, Arc<HomeTerminalSession>>> {
    HOME_TERMINAL_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn insert_home_terminal_session(session: Arc<HomeTerminalSession>) {
    home_terminal_sessions()
        .lock()
        .await
        .insert(session.session_id.clone(), session);
}

async fn home_terminal_session(session_id: &str) -> Option<Arc<HomeTerminalSession>> {
    home_terminal_sessions()
        .lock()
        .await
        .get(session_id)
        .cloned()
}

async fn close_home_terminal_session(session_id: &str, message: &str) -> Option<()> {
    let session = remove_home_terminal_session(session_id).await?;
    close_home_terminal_session_handle(session, message).await;
    Some(())
}

async fn close_home_terminal_session_handle(session: Arc<HomeTerminalSession>, message: &str) {
    let session_id = session.session_id.clone();
    {
        let mut input_handle = session.input.lock().await;
        input_handle.take();
    }
    kill_home_terminal_process(session.child_pid);
    emit_home_terminal_event(
        &session,
        HomeTerminalEvent {
            schema: HOME_TERMINAL_EVENT_SCHEMA,
            session_id,
            stream: "lifecycle",
            data: None,
            exit_code: None,
            message: Some(message.to_string()),
        },
    );
    store_archived_home_terminal_replay(&session, now_unix_ms());
}

fn home_terminal_last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

fn emit_home_terminal_event(session: &HomeTerminalSession, payload: HomeTerminalEvent) {
    emit_home_terminal_broadcast_event(&session.replay, &session.events, payload);
}

fn emit_home_terminal_broadcast_event(
    replay: &StdMutex<HomeTerminalReplayLog>,
    events: &broadcast::Sender<HomeTerminalBroadcastEvent>,
    payload: HomeTerminalEvent,
) {
    let mut replay = replay.lock().unwrap_or_else(|poison| poison.into_inner());
    replay.next_event_id = replay.next_event_id.saturating_add(1);
    let event = HomeTerminalBroadcastEvent {
        id: replay.next_event_id,
        payload,
    };
    record_home_terminal_replay(&mut replay, &event);
    let _ = events.send(event);
}

fn record_home_terminal_replay(
    replay: &mut HomeTerminalReplayLog,
    event: &HomeTerminalBroadcastEvent,
) {
    let bytes = estimated_home_terminal_event_bytes(&event.payload);
    replay.events.push_back(HomeTerminalReplayEntry {
        id: event.id,
        payload: event.payload.clone(),
        bytes,
    });
    replay.total_bytes = replay.total_bytes.saturating_add(bytes);
    while replay.events.len() > HOME_TERMINAL_REPLAY_MAX_EVENTS
        || replay.total_bytes > HOME_TERMINAL_REPLAY_MAX_BYTES
    {
        let Some(removed) = replay.events.pop_front() else {
            break;
        };
        replay.total_bytes = replay.total_bytes.saturating_sub(removed.bytes);
    }
}

fn home_terminal_replay_after(
    session: &HomeTerminalSession,
    last_event_id: Option<u64>,
) -> VecDeque<HomeTerminalBroadcastEvent> {
    let replay = session
        .replay
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    replay
        .events
        .iter()
        .filter(|entry| entry.id > last_event_id.unwrap_or(0))
        .map(|entry| HomeTerminalBroadcastEvent {
            id: entry.id,
            payload: entry.payload.clone(),
        })
        .collect()
}

fn archived_home_terminal_events_response(
    session_id: &str,
    ticket: Option<&str>,
    last_event_id: Option<u64>,
    now_ms: u64,
) -> Option<Response> {
    let replay = archived_home_terminal_replay(session_id, ticket, last_event_id, now_ms)?;
    let stream = futures_lite::stream::iter(
        replay
            .into_iter()
            .map(|event| Ok::<SseEvent, Infallible>(home_terminal_sse_event(event))),
    );
    Some(home_terminal_sse_response(Sse::new(stream).into_response()))
}

fn archived_home_terminal_replay(
    session_id: &str,
    ticket: Option<&str>,
    last_event_id: Option<u64>,
    now_ms: u64,
) -> Option<VecDeque<HomeTerminalBroadcastEvent>> {
    let mut archived = home_terminal_archived_replays()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    archived.retain_unexpired(now_ms);
    let replay = archived.replays.get(session_id)?;
    if replay.expires_at_ms < now_ms || ticket != Some(replay.stream_ticket.as_str()) {
        return None;
    }
    Some(
        replay
            .replay
            .iter()
            .filter(|entry| entry.id > last_event_id.unwrap_or(0))
            .map(|entry| HomeTerminalBroadcastEvent {
                id: entry.id,
                payload: entry.payload.clone(),
            })
            .collect(),
    )
}

fn store_archived_home_terminal_replay(session: &HomeTerminalSession, now_ms: u64) {
    let replay = {
        let replay = session
            .replay
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        replay.events.clone()
    };
    if replay.is_empty() {
        return;
    }
    home_terminal_archived_replays()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(
            session.session_id.clone(),
            HomeTerminalArchivedReplay {
                stream_ticket: session.stream_ticket.clone(),
                expires_at_ms: now_ms
                    .saturating_add(HOME_TERMINAL_REPLAY_RETENTION_SECS.saturating_mul(1_000)),
                total_bytes: replay.iter().map(|entry| entry.bytes).sum(),
                replay,
            },
            now_ms,
        );
}

fn cleanup_expired_home_terminal_archived_replays(now_ms: u64) {
    home_terminal_archived_replays()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .retain_unexpired(now_ms);
}

fn home_terminal_archived_replays() -> &'static StdMutex<HomeTerminalArchivedReplayStore> {
    HOME_TERMINAL_ARCHIVED_REPLAYS
        .get_or_init(|| StdMutex::new(HomeTerminalArchivedReplayStore::default()))
}

fn estimated_home_terminal_event_bytes(event: &HomeTerminalEvent) -> usize {
    event.data.as_ref().map_or(0, String::len)
        + event.message.as_ref().map_or(0, String::len)
        + event.session_id.len()
        + event.stream.len()
        + 64
}

async fn next_home_terminal_stream_event(
    replay: &mut VecDeque<HomeTerminalBroadcastEvent>,
    last_delivered_event_id: &mut u64,
    receiver: &mut broadcast::Receiver<HomeTerminalBroadcastEvent>,
) -> Option<HomeTerminalBroadcastEvent> {
    if let Some(event) = replay.pop_front() {
        *last_delivered_event_id = (*last_delivered_event_id).max(event.id);
        return Some(event);
    }
    loop {
        match receiver.recv().await {
            Ok(event) => {
                if event.id <= *last_delivered_event_id {
                    continue;
                }
                *last_delivered_event_id = event.id;
                return Some(event);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

fn home_terminal_sse_event(event: HomeTerminalBroadcastEvent) -> SseEvent {
    let data = serde_json::to_string(&event.payload).unwrap_or_else(|err| {
        serde_json::json!({
            "schema": HOME_TERMINAL_EVENT_SCHEMA,
            "stream": "error",
            "message": err.to_string()
        })
        .to_string()
    });
    SseEvent::default()
        .id(event.id.to_string())
        .event("terminal")
        .data(data)
}

fn home_terminal_sse_response(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    response
}

fn kill_home_terminal_process(child_pid: Option<u32>) {
    let Some(pid) = child_pid else {
        return;
    };
    #[cfg(unix)]
    unsafe {
        let pid = pid as i32;
        let _ = libc::kill(-pid, libc::SIGKILL);
        let _ = libc::kill(pid, libc::SIGKILL);
    }
}

async fn remove_home_terminal_session(session_id: &str) -> Option<Arc<HomeTerminalSession>> {
    home_terminal_sessions().lock().await.remove(session_id)
}

fn random_hex_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn home_terminal_authority_env(context: &HomeLaunchTokenContext) -> Vec<(&'static str, String)> {
    let mut env = vec![
        (
            HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID_ENV,
            context.principal_id.clone(),
        ),
        (
            HOME_CLI_AUTH_CONTEXT_SESSION_ID_ENV,
            context.session_id.clone(),
        ),
        (HOME_CLI_AUTH_CONTEXT_GRANT_ID_ENV, context.grant_id.clone()),
    ];
    if let Some(proof_binding_id) = context.proof_binding_id.clone() {
        env.push((HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID_ENV, proof_binding_id));
    }
    env
}

fn authorize_home_terminal_host_intent(
    intent: HomeTerminalHostIntentRequest,
) -> Result<HomeTerminalAuthorizedIntent, &'static str> {
    if intent.schema.as_deref() != Some(HOME_TERMINAL_HOST_INTENT_SCHEMA) {
        return Err("unsupported terminal host intent schema");
    }
    let action = required_intent_text(intent.action.as_deref(), "terminal host intent action")?;
    let target = required_intent_text(intent.target.as_deref(), "terminal host intent target")?;
    let action_id = required_intent_text(
        intent.action_id.as_deref(),
        "terminal host intent action_id",
    )?;

    if action == "sign-out" {
        if target != "home" || action_id != "auth-sign-out" {
            return Err("unauthorized terminal sign-out intent");
        }
        return Ok(HomeTerminalAuthorizedIntent {
            schema: HOME_TERMINAL_HOST_INTENT_SCHEMA,
            action,
            target,
            action_id,
            source: None,
            contact_id: None,
            route: None,
            query: None,
        });
    }

    if action == "active-shell" {
        if target != "home-gui" || action_id != "shell-switch:home-gui" {
            return Err("unauthorized terminal shell intent");
        }
        return Ok(HomeTerminalAuthorizedIntent {
            schema: HOME_TERMINAL_HOST_INTENT_SCHEMA,
            action,
            target,
            action_id,
            source: None,
            contact_id: None,
            route: None,
            query: None,
        });
    }

    if action != "open-target" {
        return Err("unsupported terminal host intent action");
    }
    if target == HOME_CLI_CAPSULE_ID || target == "home-gui" {
        return Err("unauthorized terminal open target");
    }

    if let Some(expected_target) = action_id.strip_prefix("open-gui:").map(str::trim) {
        if expected_target.is_empty() || expected_target != target {
            return Err("terminal open target does not match action_id");
        }
        if intent.query.is_some() {
            return Err("terminal open target query is not authorized");
        }
        return Ok(HomeTerminalAuthorizedIntent {
            schema: HOME_TERMINAL_HOST_INTENT_SCHEMA,
            action,
            target,
            action_id,
            source: None,
            contact_id: None,
            route: None,
            query: None,
        });
    }

    if let Some(expected_contact_id) = action_id.strip_prefix("people-message:").map(str::trim) {
        if expected_contact_id.is_empty() {
            return Err("terminal people action is missing a contact id");
        }
        let source = optional_intent_text(intent.source.as_deref());
        if source.as_deref() != Some("people-contact") {
            return Err("terminal people action has an unsupported source");
        }
        let contact_id =
            required_intent_text(intent.contact_id.as_deref(), "terminal people contact id")?;
        if contact_id != expected_contact_id {
            return Err("terminal people contact does not match action_id");
        }
        let route = required_intent_text(intent.route.as_deref(), "terminal people route")?;
        if terminal_host_intent_target_from_route(&route).as_deref() != Some(target.as_str()) {
            return Err("terminal people route does not match target");
        }
        if intent.query.is_some() {
            return Err("terminal people query is not authorized");
        }
        return Ok(HomeTerminalAuthorizedIntent {
            schema: HOME_TERMINAL_HOST_INTENT_SCHEMA,
            action,
            target,
            action_id,
            source,
            contact_id: Some(contact_id),
            route: Some(route),
            query: None,
        });
    }

    Err("unauthorized terminal host intent")
}

fn required_intent_text(
    value: Option<&str>,
    field_name: &'static str,
) -> Result<String, &'static str> {
    optional_intent_text(value).ok_or(field_name)
}

fn optional_intent_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn terminal_host_intent_target_from_route(route: &str) -> Option<String> {
    let rest = route.trim().strip_prefix("/apps/")?;
    let target = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_utf8_decoder_keeps_split_multibyte_text_intact() {
        let mut decoder = HomeTerminalUtf8Decoder::default();

        assert_eq!(decoder.push(b"Home \xe2"), Some("Home ".to_string()));
        assert_eq!(decoder.push(b"\x80\xa2 ready"), Some("• ready".to_string()));
        assert_eq!(decoder.flush_lossy(), None);
    }

    #[test]
    fn terminal_utf8_decoder_flushes_incomplete_tail_lossily() {
        let mut decoder = HomeTerminalUtf8Decoder::default();

        assert_eq!(decoder.push(b"\xe2"), None);
        assert_eq!(decoder.flush_lossy(), Some("�".to_string()));
    }

    #[test]
    fn terminal_authority_env_preserves_home_launch_context() {
        let mut context = test_home_terminal_context("principal-a", "session-a", "grant-a");
        context.proof_binding_id = Some("proof:passkey:test".to_string());

        let env = home_terminal_authority_env(&context);

        assert!(env.contains(&(
            HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID_ENV,
            "principal-a".to_string()
        )));
        assert!(env.contains(&(
            HOME_CLI_AUTH_CONTEXT_SESSION_ID_ENV,
            "session-a".to_string()
        )));
        assert!(env.contains(&(HOME_CLI_AUTH_CONTEXT_GRANT_ID_ENV, "grant-a".to_string())));
        assert!(env.contains(&(
            HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID_ENV,
            "proof:passkey:test".to_string()
        )));
    }

    #[test]
    fn terminal_host_intent_authorizes_only_explicit_home_actions() {
        let open =
            authorize_home_terminal_host_intent(test_terminal_host_intent(serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "open-gui:browser",
                "target": "browser"
            })))
            .expect("explicit open-gui action should be authorized");
        assert_eq!(open.action, "open-target");
        assert_eq!(open.action_id, "open-gui:browser");
        assert_eq!(open.target, "browser");
        assert!(open.query.is_none());

        let shell =
            authorize_home_terminal_host_intent(test_terminal_host_intent(serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "active-shell",
                "action_id": "shell-switch:home-gui",
                "target": "home-gui"
            })))
            .expect("explicit shell-switch action should be authorized");
        assert_eq!(shell.action, "active-shell");
        assert_eq!(shell.target, "home-gui");

        let sign_out =
            authorize_home_terminal_host_intent(test_terminal_host_intent(serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "sign-out",
                "action_id": "auth-sign-out",
                "target": "home"
            })))
            .expect("explicit sign-out action should be authorized");
        assert_eq!(sign_out.action, "sign-out");
        assert_eq!(sign_out.action_id, "auth-sign-out");
        assert_eq!(sign_out.target, "home");

        let people =
            authorize_home_terminal_host_intent(test_terminal_host_intent(serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "people-message:contact-alice",
                "target": "chat-room",
                "source": "people-contact",
                "contact_id": "contact-alice",
                "route": "/apps/chat-room/"
            })))
            .expect("people contact route should be authorized");
        assert_eq!(people.action_id, "people-message:contact-alice");
        assert_eq!(people.source.as_deref(), Some("people-contact"));
        assert_eq!(people.contact_id.as_deref(), Some("contact-alice"));
    }

    #[test]
    fn terminal_host_intent_rejects_implicit_or_mismatched_launches() {
        let cases = [
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "target": "browser"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "capsule-browser",
                "target": "browser"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "open-gui:wallet",
                "target": "browser"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "open-gui:home-cli",
                "target": "home-cli"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "active-shell",
                "action_id": "shell-switch:browser",
                "target": "browser"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "sign-out",
                "action_id": "auth-sign-out",
                "target": "browser"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "people-message:contact-alice",
                "target": "wallet",
                "source": "people-contact",
                "contact_id": "contact-alice",
                "route": "/apps/chat-room/"
            }),
            serde_json::json!({
                "schema": HOME_TERMINAL_HOST_INTENT_SCHEMA,
                "action": "open-target",
                "action_id": "open-gui:browser",
                "target": "browser",
                "query": { "debug": "1" }
            }),
        ];

        for case in cases {
            assert!(
                authorize_home_terminal_host_intent(test_terminal_host_intent(case)).is_err(),
                "case should be rejected"
            );
        }
    }

    #[test]
    fn terminal_start_plan_replaces_existing_exact_launch_context() {
        let context = test_home_terminal_context("principal-a", "session-a", "grant-a");
        let sessions = vec![
            test_terminal_lifecycle("old", "principal-a", "session-a", "grant-a", 900, 1),
            test_terminal_lifecycle("other", "principal-a", "session-b", "grant-b", 950, 1),
        ];

        let plan = home_terminal_start_plan(
            &sessions,
            &context,
            1_000,
            HomeTerminalSessionLimits {
                total: 2,
                per_principal: 2,
                per_auth_session: 1,
            },
        )
        .expect("same launch context should be replaced before counting capacity");

        assert_eq!(plan.stale_session_ids, Vec::<String>::new());
        assert_eq!(plan.replaced_session_ids, vec!["old"]);
    }

    #[test]
    fn terminal_start_plan_reaps_stale_unattached_sessions_before_capacity() {
        let context = test_home_terminal_context("principal-a", "session-a", "grant-a");
        let old = 1_000;
        let now = old + HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS * 1_000;
        let sessions = vec![test_terminal_lifecycle(
            "stale",
            "principal-b",
            "session-b",
            "grant-b",
            old,
            0,
        )];

        let plan = home_terminal_start_plan(
            &sessions,
            &context,
            now,
            HomeTerminalSessionLimits {
                total: 1,
                per_principal: 1,
                per_auth_session: 1,
            },
        )
        .expect("stale pending session should not consume capacity");

        assert_eq!(plan.stale_session_ids, vec!["stale"]);
        assert_eq!(plan.replaced_session_ids, Vec::<String>::new());
    }

    #[test]
    fn terminal_start_plan_keeps_attached_sessions_in_capacity() {
        let context = test_home_terminal_context("principal-a", "session-a", "grant-a");
        let old = 1_000;
        let now = old + HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS * 1_000;
        let sessions = vec![test_terminal_lifecycle(
            "attached",
            "principal-b",
            "session-b",
            "grant-b",
            old,
            1,
        )];

        let err = home_terminal_start_plan(
            &sessions,
            &context,
            now,
            HomeTerminalSessionLimits {
                total: 1,
                per_principal: 1,
                per_auth_session: 1,
            },
        )
        .expect_err("attached session should still consume capacity");

        assert!(err.message.contains("capacity unavailable"));
    }

    #[test]
    fn terminal_start_plan_enforces_per_principal_limit() {
        let context = test_home_terminal_context("principal-a", "session-c", "grant-c");
        let sessions = vec![
            test_terminal_lifecycle("one", "principal-a", "session-a", "grant-a", 900, 1),
            test_terminal_lifecycle("two", "principal-a", "session-b", "grant-b", 950, 1),
        ];

        let err = home_terminal_start_plan(
            &sessions,
            &context,
            1_000,
            HomeTerminalSessionLimits {
                total: 8,
                per_principal: 2,
                per_auth_session: 2,
            },
        )
        .expect_err("principal limit should reject another terminal");

        assert!(err.message.contains("principal limit"));
    }

    #[test]
    fn terminal_start_plan_enforces_per_auth_session_limit() {
        let context = test_home_terminal_context("principal-a", "session-a", "grant-b");
        let sessions = vec![test_terminal_lifecycle(
            "one",
            "principal-a",
            "session-a",
            "grant-a",
            900,
            1,
        )];

        let err = home_terminal_start_plan(
            &sessions,
            &context,
            1_000,
            HomeTerminalSessionLimits {
                total: 8,
                per_principal: 8,
                per_auth_session: 1,
            },
        )
        .expect_err("auth session limit should reject another terminal");

        assert!(err.message.contains("session limit"));
    }

    #[test]
    fn terminal_session_context_match_requires_principal_session_and_grant() {
        let context = test_home_terminal_context("principal-a", "session-a", "grant-a");
        let session = test_terminal_lifecycle("term", "principal-a", "session-a", "grant-a", 0, 1);
        assert!(home_terminal_session_matches_context(&session, &context));

        for (principal, session_id, grant) in [
            ("principal-b", "session-a", "grant-a"),
            ("principal-a", "session-b", "grant-a"),
            ("principal-a", "session-a", "grant-b"),
        ] {
            let other = test_terminal_lifecycle("term", principal, session_id, grant, 0, 1);
            assert!(!home_terminal_session_matches_context(&other, &context));
        }
    }

    #[test]
    fn terminal_replay_log_stays_bounded() {
        let mut replay = HomeTerminalReplayLog::default();
        for index in 0..(HOME_TERMINAL_REPLAY_MAX_EVENTS + 8) {
            record_home_terminal_replay(
                &mut replay,
                &HomeTerminalBroadcastEvent {
                    id: index as u64 + 1,
                    payload: test_terminal_event("term", "stdout", Some("x".repeat(2_048)), None),
                },
            );
        }

        assert!(replay.events.len() <= HOME_TERMINAL_REPLAY_MAX_EVENTS);
        assert!(replay.total_bytes <= HOME_TERMINAL_REPLAY_MAX_BYTES);
    }

    #[test]
    fn archived_terminal_replay_store_stays_bounded_and_evicts_oldest() {
        let mut store = HomeTerminalArchivedReplayStore::default();
        let now_ms = 5_000;
        let entry_bytes = (HOME_TERMINAL_ARCHIVED_REPLAY_MAX_BYTES
            / HOME_TERMINAL_ARCHIVED_REPLAY_MAX_SESSIONS)
            .max(1);

        for index in 0..=HOME_TERMINAL_ARCHIVED_REPLAY_MAX_SESSIONS {
            let session_id = format!("archived-term-{index}");
            store.insert(
                session_id.clone(),
                HomeTerminalArchivedReplay {
                    stream_ticket: format!("ticket-{index}"),
                    expires_at_ms: now_ms
                        + HOME_TERMINAL_REPLAY_RETENTION_SECS.saturating_mul(1_000),
                    replay: VecDeque::from([HomeTerminalReplayEntry {
                        id: 1,
                        payload: test_terminal_event(
                            &session_id,
                            "stdout",
                            Some("x".repeat(entry_bytes.saturating_sub(64).max(1))),
                            None,
                        ),
                        bytes: entry_bytes,
                    }]),
                    total_bytes: entry_bytes,
                },
                now_ms,
            );
        }

        assert!(store.replays.len() <= HOME_TERMINAL_ARCHIVED_REPLAY_MAX_SESSIONS);
        assert!(store.total_bytes <= HOME_TERMINAL_ARCHIVED_REPLAY_MAX_BYTES);
        assert!(!store.replays.contains_key("archived-term-0"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_replay_skips_duplicate_live_event_after_reconnect() {
        let replay = StdMutex::new(HomeTerminalReplayLog::default());
        let (events, _) = broadcast::channel(8);

        emit_home_terminal_broadcast_event(
            &replay,
            &events,
            test_terminal_event("term", "stdout", Some("first".to_string()), None),
        );
        let mut receiver = events.subscribe();
        emit_home_terminal_broadcast_event(
            &replay,
            &events,
            test_terminal_event("term", "stdout", Some("second".to_string()), None),
        );
        let mut replay_events = {
            let replay = replay.lock().unwrap_or_else(|poison| poison.into_inner());
            replay
                .events
                .iter()
                .filter(|entry| entry.id > 1)
                .map(|entry| HomeTerminalBroadcastEvent {
                    id: entry.id,
                    payload: entry.payload.clone(),
                })
                .collect::<VecDeque<_>>()
        };
        emit_home_terminal_broadcast_event(
            &replay,
            &events,
            test_terminal_event("term", "stdout", Some("third".to_string()), None),
        );

        let mut last_delivered_event_id = 1;
        let replayed = next_home_terminal_stream_event(
            &mut replay_events,
            &mut last_delivered_event_id,
            &mut receiver,
        )
        .await
        .expect("replayed terminal event");
        let live = next_home_terminal_stream_event(
            &mut replay_events,
            &mut last_delivered_event_id,
            &mut receiver,
        )
        .await
        .expect("next live terminal event");

        assert_eq!(replayed.id, 2);
        assert_eq!(replayed.payload.data.as_deref(), Some("second"));
        assert_eq!(live.id, 3);
        assert_eq!(live.payload.data.as_deref(), Some("third"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_publish_order_stays_monotonic_under_concurrent_producers() {
        let replay = std::sync::Arc::new(StdMutex::new(HomeTerminalReplayLog::default()));
        let (events, _) = broadcast::channel(32);
        let mut receiver = events.subscribe();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(9));
        let mut workers = Vec::new();
        for index in 0..8 {
            let replay = replay.clone();
            let events = events.clone();
            let gate = gate.clone();
            workers.push(std::thread::spawn(move || {
                gate.wait();
                emit_home_terminal_broadcast_event(
                    &replay,
                    &events,
                    test_terminal_event("term", "stdout", Some(format!("event-{index}")), None),
                );
            }));
        }
        gate.wait();
        for worker in workers {
            worker.join().expect("concurrent terminal event producer");
        }

        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(receiver.recv().await.expect("terminal broadcast event").id);
        }
        let replay_ids = {
            let replay = replay.lock().unwrap_or_else(|poison| poison.into_inner());
            replay
                .events
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
        };

        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(ids, replay_ids);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archived_terminal_replay_remains_available_after_live_session_grace() {
        let session = test_terminal_session_with_quick_exit()
            .await
            .expect("test terminal session");
        let session_id = session.session_id.clone();
        let stream_ticket = session.stream_ticket.clone();
        insert_home_terminal_session(session.clone()).await;

        emit_home_terminal_event(
            &session,
            test_terminal_event(&session_id, "lifecycle", None, Some("started".to_string())),
        );
        emit_home_terminal_event(
            &session,
            test_terminal_event(
                &session_id,
                "stdout",
                Some("fatal configuration mismatch".to_string()),
                None,
            ),
        );
        spawn_home_terminal_waiter(session.clone());

        let replay = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(replay) = archived_home_terminal_replay(
                    &session_id,
                    Some(stream_ticket.as_str()),
                    Some(1),
                    now_unix_ms(),
                ) {
                    break replay;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("archived replay before timeout");

        assert!(home_terminal_session(&session_id).await.is_none());
        assert_eq!(replay.front().map(|event| event.id), Some(2));
        assert_eq!(
            replay
                .front()
                .and_then(|event| event.payload.data.as_deref()),
            Some("fatal configuration mismatch")
        );
        assert!(
            archived_home_terminal_replay(&session_id, Some("wrong"), None, now_unix_ms(),)
                .is_none()
        );
        assert!(archived_home_terminal_replay(
            &session_id,
            Some(stream_ticket.as_str()),
            None,
            now_unix_ms() + HOME_TERMINAL_REPLAY_RETENTION_SECS.saturating_mul(1_000) + 1,
        )
        .is_none());

        test_cleanup_terminal_artifacts(&session_id).await;
    }

    fn test_home_terminal_context(
        principal_id: &str,
        session_id: &str,
        grant_id: &str,
    ) -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: principal_id.to_string(),
            session_id: session_id.to_string(),
            proof_binding_id: None,
            grant_id: grant_id.to_string(),
        }
    }

    fn test_terminal_lifecycle(
        session_id: &str,
        principal_id: &str,
        auth_session_id: &str,
        grant_id: &str,
        created_at_ms: u64,
        event_stream_generation: u64,
    ) -> HomeTerminalSessionLifecycle {
        HomeTerminalSessionLifecycle {
            session_id: session_id.to_string(),
            principal_id: principal_id.to_string(),
            auth_session_id: auth_session_id.to_string(),
            grant_id: grant_id.to_string(),
            created_at_ms,
            event_stream_generation,
        }
    }

    fn test_terminal_host_intent(value: serde_json::Value) -> HomeTerminalHostIntentRequest {
        serde_json::from_value(value).expect("test terminal host intent")
    }

    fn test_terminal_event(
        session_id: &str,
        stream: &'static str,
        data: Option<String>,
        message: Option<String>,
    ) -> HomeTerminalEvent {
        HomeTerminalEvent {
            schema: HOME_TERMINAL_EVENT_SCHEMA,
            session_id: session_id.to_string(),
            stream,
            data,
            exit_code: None,
            message,
        }
    }

    async fn test_cleanup_terminal_artifacts(session_id: &str) {
        let _ = remove_home_terminal_session(session_id).await;
        home_terminal_archived_replays()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(session_id);
    }

    async fn test_terminal_session_with_quick_exit() -> anyhow::Result<Arc<HomeTerminalSession>> {
        let child = Command::new(std::env::current_exe()?)
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let child_pid = child.id();
        let (events, _receiver) = broadcast::channel(32);
        Ok(Arc::new(HomeTerminalSession {
            session_id: format!("term-test-{}", random_hex_token()),
            stream_ticket: format!("ticket-test-{}", random_hex_token()),
            input_ticket: format!("input-test-{}", random_hex_token()),
            principal_id: "principal-test".to_string(),
            auth_session_id: "session-test".to_string(),
            grant_id: "grant-test".to_string(),
            child_pid,
            created_at_ms: now_unix_ms(),
            input: Mutex::new(None),
            child: Mutex::new(child),
            events,
            replay: StdMutex::new(HomeTerminalReplayLog::default()),
            event_stream_generation: AtomicU64::new(0),
            input_stream_generation: AtomicU64::new(0),
            pty_reader_drained: AtomicBool::new(true),
            pty_reader_drained_notify: Notify::new(),
        }))
    }
}
