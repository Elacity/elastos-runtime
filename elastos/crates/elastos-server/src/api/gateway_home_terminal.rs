use super::*;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rand::RngCore;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

const HOME_CLI_CAPSULE_ID: &str = "home-cli";
const HOME_TERMINAL_CONTRACT_SCHEMA: &str = "elastos.home-cli.terminal-contract/v1";
const HOME_TERMINAL_START_SCHEMA: &str = "elastos.home-cli.terminal-start/v1";
const HOME_TERMINAL_SESSION_SCHEMA: &str = "elastos.home-cli.terminal-session/v1";
const HOME_TERMINAL_INPUT_SCHEMA: &str = "elastos.home-cli.terminal-input/v1";
const HOME_TERMINAL_RESIZE_SCHEMA: &str = "elastos.home-cli.terminal-resize/v1";
const HOME_TERMINAL_CLOSE_SCHEMA: &str = "elastos.home-cli.terminal-close/v1";
const HOME_TERMINAL_EVENT_SCHEMA: &str = "elastos.home-cli.terminal-event/v1";
const HOME_TERMINAL_EVENT_KEEPALIVE_SECS: u64 = 15;
const HOME_TERMINAL_EVENT_DISCONNECT_GRACE_SECS: u64 = 3;
const HOME_TERMINAL_PENDING_ATTACH_TIMEOUT_SECS: u64 = 20;
const HOME_TERMINAL_MAX_ACTIVE_SESSIONS: usize = 8;
const HOME_TERMINAL_MAX_SESSIONS_PER_PRINCIPAL: usize = 4;
const HOME_TERMINAL_MAX_SESSIONS_PER_AUTH_SESSION: usize = 1;
pub(super) const HOME_TERMINAL_INPUT_MAX_BYTES: usize = 16 * 1024;
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

#[derive(Debug, Deserialize)]
pub(super) struct HomeTerminalEventsQuery {
    ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeTerminalInputRequest {
    schema: Option<String>,
    data: String,
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
    principal_id: String,
    auth_session_id: String,
    grant_id: String,
    child_pid: Option<u32>,
    created_at_ms: u64,
    input: Mutex<Option<HomeTerminalInput>>,
    child: Mutex<Child>,
    events: broadcast::Sender<HomeTerminalEvent>,
    event_stream_generation: AtomicU64,
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

pub(super) async fn home_cli_terminal_contract() -> Response {
    Json(HomeTerminalContract {
        schema: HOME_TERMINAL_CONTRACT_SCHEMA.to_string(),
        renderer_contract: "capsule-local xterm.js terminal over a Runtime-owned byte-stream contract".to_string(),
        transport: "runtime_pty_stream".to_string(),
        transport_scope: "local_runtime_adapter".to_string(),
        pty: "Runtime-owned PTY; xterm sends input bytes and renders PTY output without direct host process authority".to_string(),
        protocol: "SSE PTY output + HTTP input/resize".to_string(),
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
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/input",
            auth: "same home-cli launch token context that created the session",
        },
        resize: HomeTerminalEndpoint {
            method: "POST",
            route: "/api/apps/home-cli/terminal/sessions/:session_id/resize",
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
    match start_home_terminal_session(context, size).await {
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
                "input_url": format!("/api/apps/home-cli/terminal/sessions/{}/input", session.session_id),
                "resize_url": format!("/api/apps/home-cli/terminal/sessions/{}/resize", session.session_id),
                "close_url": format!("/api/apps/home-cli/terminal/sessions/{}/close", session.session_id),
                "input_schema": HOME_TERMINAL_INPUT_SCHEMA,
                "resize_schema": HOME_TERMINAL_RESIZE_SCHEMA,
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
    Query(query): Query<HomeTerminalEventsQuery>,
) -> Response {
    cleanup_stale_home_terminal_sessions(now_unix_ms()).await;
    let session = match home_terminal_session(&session_id).await {
        Some(session) => session,
        None => return (StatusCode::NOT_FOUND, "terminal session not found").into_response(),
    };
    if query.ticket.as_deref() != Some(session.stream_ticket.as_str()) {
        return (StatusCode::FORBIDDEN, "invalid terminal stream ticket").into_response();
    }
    let receiver = session.events.subscribe();
    let generation = session
        .event_stream_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    let state = HomeTerminalEventStreamState {
        receiver,
        _guard: HomeTerminalEventStreamGuard {
            session_id: session_id.clone(),
            generation,
        },
    };
    let stream = futures_lite::stream::unfold(state, |mut state| async move {
        loop {
            match state.receiver.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_else(|err| {
                        serde_json::json!({
                            "schema": HOME_TERMINAL_EVENT_SCHEMA,
                            "stream": "error",
                            "message": err.to_string()
                        })
                        .to_string()
                    });
                    let event = SseEvent::default().event("terminal").data(data);
                    return Some((Ok::<SseEvent, Infallible>(event), state));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(HOME_TERMINAL_EVENT_KEEPALIVE_SECS))
                .text("keepalive"),
        )
        .into_response();
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

pub(super) async fn home_cli_terminal_input(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<HomeTerminalInputRequest>,
) -> Response {
    if let Some(schema) = input.schema.as_deref() {
        if schema != HOME_TERMINAL_INPUT_SCHEMA {
            return (StatusCode::BAD_REQUEST, "unsupported terminal input schema").into_response();
        }
    }
    if input.data.len() > HOME_TERMINAL_INPUT_MAX_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "terminal input is too large").into_response();
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
    let mut input_handle = session.input.lock().await;
    let Some(input_handle) = input_handle.as_mut() else {
        return (StatusCode::GONE, "terminal input is closed").into_response();
    };
    if let Err(err) = input_handle.write_all(input.data.as_bytes()).await {
        return home_error_response(anyhow::anyhow!("terminal input write failed: {err}"));
    }
    Json(serde_json::json!({
        "schema": HOME_TERMINAL_INPUT_SCHEMA,
        "session_id": session_id,
        "written_bytes": input.data.len()
    }))
    .into_response()
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
) -> Result<Arc<HomeTerminalSession>, HomeTerminalStartError> {
    Err(HomeTerminalStartError::Runtime(anyhow::anyhow!(
        "Runtime PTY terminal is not supported on this platform"
    )))
}

#[cfg(unix)]
async fn start_home_terminal_session(
    context: HomeLaunchTokenContext,
    size: HomeTerminalSize,
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
        principal_id: context.principal_id,
        auth_session_id: context.session_id,
        grant_id: context.grant_id,
        child_pid,
        created_at_ms: now_unix_ms(),
        input: Mutex::new(Some(pty.input)),
        child: Mutex::new(child),
        events,
        event_stream_generation: AtomicU64::new(0),
    });
    insert_home_terminal_session(session.clone()).await;
    spawn_home_terminal_pty_reader(session.clone(), pty.reader);
    spawn_home_terminal_waiter(session.clone());
    spawn_home_terminal_attach_watchdog(session.clone());
    let _ = session.events.send(HomeTerminalEvent {
        schema: HOME_TERMINAL_EVENT_SCHEMA,
        session_id: session.session_id.clone(),
        stream: "lifecycle",
        data: None,
        exit_code: None,
        message: Some("started".to_string()),
    });
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
    let mut winsize = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
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
                    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
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
                    let _ = session.events.send(HomeTerminalEvent {
                        schema: HOME_TERMINAL_EVENT_SCHEMA,
                        session_id: session.session_id.clone(),
                        stream: "error",
                        data: None,
                        exit_code: None,
                        message: Some(format!("PTY read failed: {err}")),
                    });
                    break;
                }
            }
        }
    });
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
    let _ = session.events.send(HomeTerminalEvent {
        schema: HOME_TERMINAL_EVENT_SCHEMA,
        session_id: session.session_id.clone(),
        stream: "stdout",
        data: Some(data),
        exit_code: None,
        message: None,
    });
}

fn spawn_home_terminal_waiter(session: Arc<HomeTerminalSession>) {
    tokio::spawn(async move {
        let status = session.child.lock().await.wait().await;
        {
            let mut input_handle = session.input.lock().await;
            input_handle.take();
        }
        let (exit_code, message) = match status {
            Ok(status) => (status.code(), "exited".to_string()),
            Err(err) => (None, format!("wait failed: {err}")),
        };
        let _ = session.events.send(HomeTerminalEvent {
            schema: HOME_TERMINAL_EVENT_SCHEMA,
            session_id: session.session_id.clone(),
            stream: "lifecycle",
            data: None,
            exit_code,
            message: Some(message),
        });
        remove_home_terminal_session(&session.session_id).await;
    });
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
    receiver: broadcast::Receiver<HomeTerminalEvent>,
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
    let _ = session.events.send(HomeTerminalEvent {
        schema: HOME_TERMINAL_EVENT_SCHEMA,
        session_id,
        stream: "lifecycle",
        data: None,
        exit_code: None,
        message: Some(message.to_string()),
    });
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
}
