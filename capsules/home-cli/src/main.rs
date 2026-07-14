use anyhow::{anyhow, Result};
use elastos_guest::runtime::RuntimeClient;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

const DASHBOARD_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
thread_local! {
    static CLIENT: RefCell<RuntimeClient> = RefCell::new(RuntimeClient::new());
}

const STARTUP_ENTER_SETTLE_WINDOW: Duration = Duration::from_millis(350);
const ESCAPE_SEQUENCE_SETTLE_WINDOW: Duration = Duration::from_millis(25);
const ESCAPE_SEQUENCE_BYTE_TIMEOUT_MS: i32 = 10;
const ESCAPE_SEQUENCE_MAX_BYTES: usize = 64;
const LIVE_REFRESH_POLL_MS: i32 = 300;
const COMMAND_CONTRACT_JSON: &str = include_str!("../browser/commands.json");
const TUI_TAB_ROW: u16 = 1;
const TUI_FOOTER_TEXT: &str =
    " Keys: Up/Down select  Left/Right/Tab sections  Enter open  r refresh  q/Esc home-gui  ? help";
const TUI_HELP_FOOTER_TEXT: &str = " Keys: ? close help  q/Esc home-gui  Left/Right/Tab sections";
const TUI_HELP_LINES: &[(&str, &str)] = &[
    ("Up/Down", "select the previous or next visible item"),
    ("Left/Right", "switch sections"),
    ("Tab", "switch to the next section"),
    ("Shift+Tab", "switch to the previous section"),
    ("Enter", "open the selected action"),
    ("1-9", "quick-launch visible Home actions"),
    ("r", "refresh Home facts"),
    ("q or Esc", "return to home-gui"),
    ("?", "close help"),
    ("Inbox", "m marks read, d dismisses"),
    (
        "Mouse",
        "wheel selects items; tab-row clicks switch sections",
    ),
];

#[derive(Debug, Clone, Deserialize)]
struct HomeSnapshot {
    version: String,
    user: String,
    nickname: Option<String>,
    did: Option<String>,
    #[serde(default)]
    session: HomeCliSessionStatus,
    source: Option<SourceStatus>,
    runtime: RuntimeStatus,
    system_services: Vec<SystemServiceStatus>,
    #[serde(default)]
    services: Option<serde_json::Value>,
    site: SiteStatus,
    #[serde(default)]
    shares: ShareStatus,
    #[serde(default)]
    room: RoomStatus,
    #[serde(default)]
    people: PeopleStatus,
    #[serde(default)]
    notifications: NotificationStatus,
    roots: Vec<RootStatus>,
    actions: Vec<ActionInfo>,
    #[serde(default)]
    active_shell: ActiveShellStatus,
    #[serde(default)]
    cached_capsules: Vec<String>,
    #[serde(default)]
    capsule_catalog: Option<serde_json::Value>,
    #[serde(default)]
    capsule_interfaces: Option<serde_json::Value>,
    #[serde(default)]
    notice: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct HomeCliSessionStatus {
    #[serde(default)]
    mode: String,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ActiveShellStatus {
    #[serde(default)]
    active: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ShareStatus {
    #[serde(default)]
    channel_count: usize,
    #[serde(default)]
    active_count: usize,
    #[serde(default)]
    author_did: Option<String>,
    #[serde(default)]
    channels: Vec<ShareChannelStatus>,
}

#[derive(Debug, Clone, Deserialize)]
struct ShareChannelStatus {
    name: String,
    latest_cid: String,
    latest_version: u64,
    status: String,
    #[serde(default)]
    head_cid: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomStatus {
    #[serde(default)]
    room_slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    owner_did: Option<String>,
    #[serde(default)]
    current_key_epoch: u64,
    #[serde(default)]
    admin_count: usize,
    #[serde(default)]
    member_count: usize,
    #[serde(default)]
    active_member_count: usize,
    #[serde(default)]
    pending_invite_count: usize,
    #[serde(default)]
    allow_guest_invites: bool,
    #[serde(default)]
    allow_member_invites: bool,
    #[serde(default)]
    allow_members_to_host_guests: bool,
    #[serde(default)]
    local_runtime_role: Option<String>,
    #[serde(default)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    ephemeral_hosted_guest_url: Option<String>,
    #[serde(default)]
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    pending_count: usize,
    #[serde(default)]
    active_session_count: usize,
    #[serde(default)]
    active_participants: Vec<RoomParticipantStatus>,
    #[serde(default)]
    pending_requests: Vec<RoomPendingRequestStatus>,
    #[serde(default)]
    active_sessions: Vec<RoomSessionStatus>,
    #[serde(default)]
    members: Vec<RoomMemberStatus>,
    #[serde(default)]
    pending_invites: Vec<RoomInviteStatus>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomParticipantStatus {
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomPendingRequestStatus {
    #[allow(dead_code)]
    request_id: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomSessionStatus {
    #[allow(dead_code)]
    token: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomMemberStatus {
    member_did: String,
    role: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomInviteStatus {
    #[allow(dead_code)]
    invite_id: String,
    invited_did: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleStatus {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    contact_count: usize,
    #[serde(default)]
    contacts: Vec<PeopleContactStatus>,
    #[serde(default)]
    service_offer_count: usize,
    #[serde(default)]
    discovery: PeopleDiscoveryStatus,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleContactStatus {
    contact_id: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    relationship: String,
    #[serde(default)]
    route: String,
    #[serde(default)]
    can_message: bool,
    #[serde(default)]
    device_label: Option<String>,
    #[serde(default)]
    profile_card: Option<PeopleProfileCardStatus>,
    #[serde(default)]
    last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleProfileCardStatus {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    handle: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleDiscoveryStatus {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    remaining_seconds: Option<u64>,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    status_message: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    local_peer_id: Option<String>,
    #[serde(default)]
    discovered_count: usize,
    #[serde(default)]
    discovered_peers: Vec<PeopleDiscoveryPeerStatus>,
    #[serde(default)]
    request_count: usize,
    #[serde(default)]
    requests: Vec<PeopleDiscoveryRequestStatus>,
    #[serde(default)]
    next_refresh_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleDiscoveryPeerStatus {
    #[serde(default)]
    peer_id: String,
    #[serde(default)]
    did: Option<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    last_seen_at: u64,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PeopleDiscoveryRequestStatus {
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    peer_id: String,
    #[serde(default)]
    did: Option<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    status: String,
    #[serde(default)]
    invite_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotificationStatus {
    #[serde(default)]
    unread_count: usize,
    #[serde(default)]
    attention_count: usize,
    #[serde(default)]
    entries: Vec<NotificationEntryStatus>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotificationEntryStatus {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    source_app: String,
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    title: String,
    body: String,
    #[allow(dead_code)]
    action_ref: Option<NotificationActionRefStatus>,
    #[allow(dead_code)]
    read: bool,
    #[allow(dead_code)]
    severity: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotificationActionRefStatus {
    #[allow(dead_code)]
    app: String,
    action_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SourceStatus {
    name: String,
    #[serde(default)]
    channel: String,
    installed_version: String,
    gateway: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeStatus {
    running: bool,
    kind: Option<String>,
    peer_count: Option<usize>,
    ticket: Option<String>,
    #[serde(default)]
    running_capsules: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SystemServiceStatus {
    name: String,
    ready: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct SiteStatus {
    staged: bool,
    #[serde(default)]
    local_url: Option<String>,
    #[serde(default)]
    active_release: Option<String>,
    #[serde(default)]
    active_channel: Option<String>,
    #[serde(default)]
    active_bundle_cid: Option<String>,
    #[serde(default)]
    release_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct RootStatus {
    name: String,
    #[serde(default)]
    kind: String,
    uri: String,
    path: Option<String>,
    exists: bool,
    description: String,
    example: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionInfo {
    id: String,
    label: String,
    description: String,
    command: String,
    ready: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HomeIntent<'a> {
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    invoke: Option<HomeInvokeIntent>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct HomeInvokeIntent {
    capsule: String,
    #[serde(rename = "interface")]
    interface_id: String,
    method: String,
    input: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    Inbox,
    People,
    Apps,
    System,
}

const DEFAULT_TABS: &[Tab] = &[Tab::Home, Tab::Inbox, Tab::People, Tab::Apps, Tab::System];

#[derive(Debug, Clone)]
struct TuiState {
    tab: Tab,
    home_index: usize,
    inbox_index: usize,
    people_index: usize,
    app_index: usize,
    system_index: usize,
    show_help: bool,
    notice: Option<String>,
}

#[derive(Debug, Clone)]
struct AppEntry {
    name: String,
    action_id: String,
    label: String,
    category: &'static str,
    description: String,
    command: String,
    state: String,
    is_control: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeopleAction {
    id: String,
    label: String,
    description: String,
    command: String,
    ready: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemAction {
    id: String,
    label: String,
    description: String,
    command: String,
    ready: bool,
    reason: Option<String>,
}

#[derive(Clone, Copy)]
struct AppSurfaceSpec {
    name: &'static str,
    action_ids: &'static [&'static str],
    label: &'static str,
    category: &'static str,
    description: &'static str,
    command: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
struct CommandContract {
    schema: String,
    terminal: TerminalContract,
    commands: Vec<CommandSpec>,
    #[serde(default)]
    controls: Vec<ControlSpec>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TerminalContract {
    renderer: Option<String>,
    transport: Option<String>,
    transport_scope: Option<String>,
    input: Option<String>,
    pty: Option<String>,
    xterm: Option<String>,
    entrypoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CommandSpec {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    usage: String,
    summary: String,
    description: String,
    #[serde(default)]
    surface: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ControlSpec {
    key: String,
    description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    MarkRead,
    Dismiss,
    Refresh,
    Quit,
    Help,
    Digit(usize),
    Mouse(MouseEvent),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseEvent {
    button: u16,
    x: u16,
    y: u16,
    released: bool,
}

struct TerminalGuard {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    original_termios: Option<libc::termios>,
}

const APP_SURFACES: &[AppSurfaceSpec] = &[AppSurfaceSpec {
    name: "chat",
    action_ids: &["chat"],
    label: "Chat",
    category: "Communication",
    description: "Talk to people and connected ElastOS homes from this local world.",
    command: "elastos chat",
}];

fn command_contract() -> CommandContract {
    let contract: CommandContract =
        serde_json::from_str(COMMAND_CONTRACT_JSON).expect("valid Home CLI command contract");
    assert_eq!(
        contract.schema, "elastos.home-cli.command-contract/v1",
        "unexpected Home CLI command contract schema"
    );
    contract
}

fn normalize_contract_command(input: &str) -> String {
    let query = input.trim().to_lowercase();
    if query.is_empty() {
        return String::new();
    }
    for command in command_contract().commands {
        if command.name == query || command.aliases.iter().any(|alias| alias == &query) {
            return command.name;
        }
    }
    query
}

fn normalize_lookup(input: &str) -> String {
    input.trim().to_lowercase()
}

fn contract_commands_for(surface: &str) -> Vec<CommandSpec> {
    command_contract()
        .commands
        .into_iter()
        .filter(|command| {
            command.surface.is_empty() || command.surface.iter().any(|item| item == surface)
        })
        .collect()
}

fn with_client<F, R>(f: F) -> R
where
    F: FnOnce(&mut RuntimeClient) -> R,
{
    CLIENT.with(|client| f(&mut client.borrow_mut()))
}

fn request_capability(resource: &str, action: &str) -> Result<String> {
    with_client(|client| {
        client
            .request_capability(resource, action)
            .map_err(|e| anyhow!("Capability request failed: {}", e))
    })
}

fn carrier_invoke(
    token: &str,
    uri: &str,
    operation: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    with_client(
        |client| match client.carrier_invoke(uri, operation, body, token) {
            Ok(value) => Ok(value),
            Err(err) => {
                eprintln!("carrier invoke {operation} {uri} failed: {err}");
                Err(anyhow!(
                    "Carrier invoke {} {} failed: {}",
                    operation,
                    uri,
                    err
                ))
            }
        },
    )
}

fn storage_read_utf8(token: &str, path: &str) -> Result<Vec<u8>> {
    let body = serde_json::json!({
        "path": path,
        "encoding": "utf8",
    });
    let result = carrier_invoke(token, path, "read", &body)?;
    storage_read_bytes_from_result(&result)
}

fn storage_result_body(result: &serde_json::Value) -> Result<&serde_json::Value> {
    let response = result.get("response").unwrap_or(result);
    if response.get("type").and_then(|value| value.as_str()) == Some("carrier_result") {
        return response
            .get("result")
            .ok_or_else(|| anyhow!("carrier_result response missing result"));
    }
    Ok(response)
}

fn storage_read_bytes_from_result(result: &serde_json::Value) -> Result<Vec<u8>> {
    let body = storage_result_body(result)?;
    if body.get("status").and_then(|value| value.as_str()) == Some("error") {
        let code = body
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("read_failed");
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("localhost/read failed");
        return Err(anyhow!("localhost/read failed: {}: {}", code, message));
    }
    if body.get("type").and_then(|value| value.as_str()) == Some("error") {
        let code = body
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("read_failed");
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("localhost/read failed");
        return Err(anyhow!("localhost/read failed: {}: {}", code, message));
    }
    let data = body
        .get("data")
        .map(|value| {
            value
                .get("content")
                .or_else(|| value.get("data"))
                .unwrap_or(value)
        })
        .or_else(|| body.get("content"))
        .ok_or_else(|| anyhow!("localhost/read response missing data"))?;

    if let Some(bytes) = data.as_array() {
        return Ok(bytes
            .iter()
            .filter_map(|value| value.as_u64().map(|byte| byte as u8))
            .collect());
    }

    if let Some(text) = data.as_str() {
        return Ok(text.as_bytes().to_vec());
    }

    Err(anyhow!("localhost/read returned unsupported data shape"))
}

fn storage_write(token: &str, path: &str, content: Vec<u8>) -> Result<()> {
    carrier_invoke(
        token,
        path,
        "write",
        &serde_json::json!({
            "path": path,
            "content": content,
            "append": false,
        }),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let session_root = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("Home CLI capsule missing session root argument"))?;
    let session_scope = format!("{}/*", session_root.trim_end_matches('/'));
    let read_token = request_capability(&session_scope, "read")?;
    let write_token = request_capability(&session_scope, "write")?;
    let snapshot_path = format!("{}/snapshot.json", session_root.trim_end_matches('/'));
    let intent_path = format!("{}/intent.json", session_root.trim_end_matches('/'));
    let snapshot = load_snapshot(&read_token, &snapshot_path)?;

    dashboard_loop(
        &read_token,
        &snapshot_path,
        snapshot,
        &write_token,
        &intent_path,
    )
}

fn load_snapshot(read_token: &str, snapshot_path: &str) -> Result<HomeSnapshot> {
    Ok(serde_json::from_slice(&storage_read_utf8(
        read_token,
        snapshot_path,
    )?)?)
}

fn dashboard_loop(
    read_token: &str,
    snapshot_path: &str,
    snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    if should_use_tui() {
        dashboard_tui_loop(
            read_token,
            snapshot_path,
            snapshot,
            write_token,
            intent_path,
        )
    } else {
        dashboard_line_loop(
            read_token,
            snapshot_path,
            snapshot,
            write_token,
            intent_path,
        )
    }
}

fn should_use_tui() -> bool {
    if let Ok(mode) = std::env::var("ELASTOS_HOME_TUI") {
        return matches!(mode.as_str(), "1" | "true" | "yes");
    }

    if std::env::var("ELASTOS_TERM_COLS").is_ok() && std::env::var("ELASTOS_TERM_ROWS").is_ok() {
        return true;
    }

    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn dashboard_tui_loop(
    _read_token: &str,
    _snapshot_path: &str,
    snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    let mut state = TuiState::default();
    let _guard = TerminalGuard::enter()?;
    let mut startup_input_drained = false;
    let mut home_launch_armed = false;
    let mut home_launch_ready_at: Option<Instant> = None;
    let mut needs_render = true;

    loop {
        if needs_render {
            render_tui(&snapshot, &state)?;
            needs_render = false;
        }
        if !startup_input_drained {
            drain_startup_input()?;
            startup_input_drained = true;
        }

        let key = read_ui_key()?;
        if key == UiKey::None {
            continue;
        }
        match startup_home_enter_decision(
            &state,
            key,
            home_launch_armed,
            home_launch_ready_at,
            Instant::now(),
        ) {
            HomeLaunchDecision::Defer(ready_at) => {
                state.notice = Some(
                    "Press Enter again to launch Chat, or use arrows / Tab to pick something else."
                        .to_string(),
                );
                home_launch_armed = true;
                home_launch_ready_at = Some(ready_at);
                drain_startup_input()?;
                needs_render = true;
                continue;
            }
            HomeLaunchDecision::IgnoreDuplicate => {
                drain_startup_input()?;
                continue;
            }
            HomeLaunchDecision::Allow | HomeLaunchDecision::NotApplicable => {}
        }
        if !matches!(key, UiKey::None | UiKey::Enter) {
            home_launch_armed = true;
            home_launch_ready_at = None;
            if state.notice.take().is_some() {
                needs_render = true;
            }
        }

        match key {
            UiKey::Quit => {
                write_intent(write_token, intent_path, "quit")?;
                return Ok(());
            }
            UiKey::Refresh => {
                write_intent(write_token, intent_path, "refresh")?;
                return Ok(());
            }
            UiKey::Help => {
                state.show_help = !state.show_help;
                state.notice = None;
                needs_render = true;
            }
            UiKey::Left => {
                state.prev_tab();
                state.notice = None;
                needs_render = true;
            }
            UiKey::Right => {
                state.next_tab();
                state.notice = None;
                needs_render = true;
            }
            UiKey::Up => {
                state.move_prev(&snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Down => {
                state.move_next(&snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Enter => {
                state.notice = None;
                home_launch_ready_at = None;
                if let Some(action_id) = state.activate(&snapshot) {
                    write_intent(write_token, intent_path, &action_id)?;
                    return Ok(());
                }
            }
            UiKey::MarkRead => {
                if state.tab == Tab::Inbox {
                    if let Some(action_id) =
                        selected_notification_read_action(&snapshot, state.inbox_index)
                    {
                        write_intent(write_token, intent_path, &action_id)?;
                        return Ok(());
                    }
                }
            }
            UiKey::Dismiss => {
                if state.tab == Tab::Inbox {
                    if let Some(action_id) =
                        selected_notification_dismiss_action(&snapshot, state.inbox_index)
                    {
                        write_intent(write_token, intent_path, &action_id)?;
                        return Ok(());
                    }
                }
            }
            UiKey::Digit(index) => {
                let quick_actions = quick_launch_action_indices(&snapshot);
                if let Some(action_idx) = quick_actions.get(index.saturating_sub(1)).copied() {
                    state.tab = Tab::Home;
                    state.home_index = index.saturating_sub(1).min(quick_actions.len() - 1);
                    state.notice = None;
                    let action = &snapshot.actions[action_idx];
                    write_intent(write_token, intent_path, &action.id)?;
                    return Ok(());
                }
            }
            UiKey::Mouse(event) => {
                if state.handle_mouse(event, term_cols(), &snapshot) {
                    state.notice = None;
                    needs_render = true;
                }
            }
            UiKey::None => {}
        }
    }
}

fn drain_startup_input() -> Result<()> {
    loop {
        if !stdin_has_input(0)? {
            return Ok(());
        }
        let _ = read_stdin_byte()?;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeLaunchDecision {
    NotApplicable,
    Defer(Instant),
    IgnoreDuplicate,
    Allow,
}

fn startup_home_enter_decision(
    state: &TuiState,
    key: UiKey,
    home_launch_armed: bool,
    home_launch_ready_at: Option<Instant>,
    now: Instant,
) -> HomeLaunchDecision {
    if !matches!(key, UiKey::Enter)
        || state.tab != Tab::Home
        || state.home_index != 0
        || state.show_help
    {
        return HomeLaunchDecision::NotApplicable;
    }

    if !home_launch_armed {
        return HomeLaunchDecision::Defer(now + STARTUP_ENTER_SETTLE_WINDOW);
    }

    if home_launch_ready_at.is_some_and(|ready_at| now < ready_at) {
        return HomeLaunchDecision::IgnoreDuplicate;
    }

    HomeLaunchDecision::Allow
}

fn dashboard_line_loop(
    read_token: &str,
    snapshot_path: &str,
    mut snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    loop {
        render_line_dashboard(&snapshot)?;
        print!("Select action (number, r refresh, q exit, ? help): ");
        io::stdout().flush()?;

        if !stdin_has_input(LIVE_REFRESH_POLL_MS)? {
            if let Ok(next_snapshot) = load_snapshot(read_token, snapshot_path) {
                snapshot = next_snapshot;
            }
            continue;
        }

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            write_intent(write_token, intent_path, "quit")?;
            return Ok(());
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        match trimmed {
            "q" | "quit" | "/quit" | "/q" => {
                write_intent(write_token, intent_path, "quit")?;
                return Ok(());
            }
            "r" | "refresh" | "/refresh" => {
                write_intent(write_token, intent_path, "refresh")?;
                return Ok(());
            }
            "?" | "help" | "/help" => {
                print_line_help()?;
                continue;
            }
            _ => {}
        }

        if let Some(result) = cli_invoke_intent(trimmed, &snapshot) {
            match result {
                Ok(invoke) => {
                    write_invoke_intent(write_token, intent_path, invoke)?;
                    return Ok(());
                }
                Err(error) => {
                    println!();
                    println!("invoke: {}", error);
                    wait_for_enter()?;
                    continue;
                }
            }
        }

        match people_line_action(trimmed, &snapshot) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("people: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        match mywebsite_line_action(trimmed) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("mywebsite: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        match system_line_action(trimmed, &snapshot) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("system: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        if handle_shared_line_command(trimmed, &snapshot)? {
            wait_for_enter()?;
            continue;
        }

        let Ok(index) = trimmed.parse::<usize>() else {
            println!("Unknown command: {}. Type ? for help.", trimmed);
            wait_for_enter()?;
            continue;
        };

        let quick_actions = quick_launch_action_indices(&snapshot);
        let Some(action_idx) = quick_actions.get(index.saturating_sub(1)).copied() else {
            println!("No action {}. Pick 1-{}.", index, quick_actions.len());
            wait_for_enter()?;
            continue;
        };
        let action = &snapshot.actions[action_idx];

        if !action.ready {
            println!(
                "{} is not ready: {}",
                action.label,
                action.reason.as_deref().unwrap_or("missing prerequisites")
            );
            wait_for_enter()?;
            continue;
        }

        write_intent(write_token, intent_path, &action.id)?;
        return Ok(());
    }
}

fn write_intent(write_token: &str, intent_path: &str, action: &str) -> Result<()> {
    storage_write(write_token, intent_path, home_intent_payload(action, None)?)
}

fn write_invoke_intent(
    write_token: &str,
    intent_path: &str,
    invoke: HomeInvokeIntent,
) -> Result<()> {
    storage_write(
        write_token,
        intent_path,
        home_intent_payload("invoke", Some(invoke))?,
    )
}

fn home_intent_payload(action: &str, invoke: Option<HomeInvokeIntent>) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&HomeIntent { action, invoke })?)
}

fn render_line_dashboard(snapshot: &HomeSnapshot) -> Result<()> {
    print_cli_page_header(snapshot, "Home");
    println!("A compact Home shell for working CLI journeys.");
    println!(
        "Version: runtime {}  home {}  installed {}",
        snapshot.version,
        DASHBOARD_VERSION,
        snapshot
            .source
            .as_ref()
            .map(|source| source.installed_version.as_str())
            .unwrap_or("(none)")
    );

    println!();
    println!("Now");
    println!("  User:      {}", snapshot.user);
    println!("  Nick:      {}", display_name(snapshot));
    println!("  Identity:  {}", identity_summary(snapshot));
    println!("  Network:   {}", network_summary(snapshot));
    println!("  Shell:     {}", active_shell_label(snapshot));
    println!(
        "  Capsules:  {} installed / {} running",
        snapshot.cached_capsules.len(),
        snapshot.runtime.running_capsules.len()
    );

    println!();
    let quick_actions = quick_launch_action_indices(snapshot);
    if quick_actions.is_empty() {
        println!("Start Here");
        println!("  No CLI actions are ready in this snapshot.");
    } else {
        println!("Start Here");
        for (slot, action_idx) in quick_actions.iter().enumerate() {
            let action = &snapshot.actions[*action_idx];
            println!(
                "  {}. {} [{}]",
                slot + 1,
                action_display_label(action),
                if action.ready { "ready" } else { "blocked" }
            );
            println!("     {}", home_action_summary(action));
            if !action.command.trim().is_empty() {
                println!("     {}", action.command);
            }
            if let Some(reason) = &action.reason {
                println!("     setup: {}", reason);
            }
        }
    }

    let alerts = alerts_lines(snapshot, 80, snapshot.notice.as_deref());
    if !alerts.is_empty() {
        println!();
        println!("Needs Attention");
        for line in alerts {
            println!("  {}", line);
        }
    }

    println!();
    println!("Inbox");
    println!(
        "  Attention: {} waiting / {} unread",
        snapshot.notifications.attention_count, snapshot.notifications.unread_count
    );
    for entry in snapshot.notifications.entries.iter().take(3) {
        println!("  - {}", entry.body);
    }

    println!();
    println!("Apps");
    for line in apps_summary_lines(snapshot) {
        println!("  {}", line);
    }

    println!();
    println!("Other Commands");
    println!("  wallet       Wallet targets and approval hints");
    println!("  exits        Browser Exit offers");
    println!("  debug        Developer facts and projection details");

    if let Some(notice) = &snapshot.notice {
        println!();
        println!("Notice");
        println!("  {}", notice);
    }

    println!();
    println!("Choose an action number, `r` to refresh, `q` to return to home-gui, `?` for help.");
    io::stdout().flush()?;
    Ok(())
}

fn cli_page_header(snapshot: &HomeSnapshot, title: &str) -> String {
    format!(
        "\x1B[2J\x1B[HHome CLI / {title}\nuser {}  |  identity {}  |  network {}  |  shell {}\n\n",
        display_name(snapshot),
        identity_summary(snapshot),
        network_summary(snapshot),
        active_shell_label(snapshot)
    )
}

fn print_cli_page_header(snapshot: &HomeSnapshot, title: &str) {
    print!("{}", cli_page_header(snapshot, title));
}

fn print_line_help() -> Result<()> {
    print_cli_help_topic("");
    wait_for_enter()
}

fn print_cli_help_topic(topic: &str) {
    println!();
    let normalized = normalize_contract_command(topic);
    if !normalized.is_empty() {
        let contract = command_contract();
        if let Some(command) = contract
            .commands
            .into_iter()
            .find(|command| command.name == normalized)
        {
            println!("{}", command.usage);
            println!("  {}", command.description);
            println!("  surface: {}", command.surface.join(", "));
            return;
        }
    }
    println!("Home Commands");
    for command in contract_commands_for("home-cli") {
        println!("  {:<24} {}", command.usage, command.summary);
    }
    println!("  <number>                 Launch a quick action and return home afterward");
    println!();
    println!("Home CLI line mode uses the shared command vocabulary over the local");
    println!("Home snapshot. Low-risk invoke writes a signed Home intent; user/high-risk");
    println!("methods still fail closed.");
}

fn cli_invoke_intent(input: &str, snapshot: &HomeSnapshot) -> Option<Result<HomeInvokeIntent>> {
    let mut parts = input.split_whitespace();
    let raw_name = parts.next()?;
    if normalize_contract_command(raw_name) != "invoke" {
        return None;
    }
    let arg = parts.collect::<Vec<_>>().join(" ");
    Some(resolve_cli_invoke_intent(&arg, snapshot))
}

fn resolve_cli_invoke_intent(arg: &str, snapshot: &HomeSnapshot) -> Result<HomeInvokeIntent> {
    let raw = arg.trim();
    let Some((capsule_input, rest)) = raw.split_once(char::is_whitespace) else {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    };
    let rest = rest.trim();
    if rest.is_empty() {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    }
    let (method_input, input) = match rest.split_once(char::is_whitespace) {
        Some((method, input)) => (method, input.trim()),
        None => (rest, ""),
    };
    let capsule = find_capsule_fact(snapshot, capsule_input)
        .ok_or_else(|| anyhow!("capsule not found: {capsule_input}"))?;
    let capsule_name = json_text(capsule, "name");
    let (interface_id, method) = resolve_cli_method(snapshot, capsule_name, method_input)?;
    if let Some(reason) = cli_method_block_reason(method) {
        anyhow::bail!("blocked: {reason}");
    }
    Ok(HomeInvokeIntent {
        capsule: capsule_name.to_string(),
        interface_id: interface_id.to_string(),
        method: json_text(method, "id").to_string(),
        input: parse_cli_invoke_input(input, method)?,
    })
}

fn resolve_cli_method<'a>(
    snapshot: &'a HomeSnapshot,
    capsule_name: &str,
    method_input: &str,
) -> Result<(&'a str, &'a serde_json::Value)> {
    let method_query = normalize_lookup(method_input);
    if method_query.is_empty() {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    }
    let mut matches = Vec::new();
    for entry in cli_interface_entries_for(snapshot, Some(capsule_name)) {
        let descriptor = interface_descriptor(entry);
        let interface_id = json_text(descriptor, "id");
        if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
            for method in methods {
                if normalize_lookup(json_text(method, "id")) == method_query {
                    matches.push((interface_id, method));
                }
            }
        }
    }
    match matches.len() {
        1 => Ok(matches[0]),
        0 => anyhow::bail!("method not found: {method_input}"),
        _ => anyhow::bail!("ambiguous method: {method_input}"),
    }
}

fn cli_method_block_reason(method: &serde_json::Value) -> Option<String> {
    let approval = json_text(method, "approval");
    let approval = if approval.is_empty() {
        json_text(method, "approval_mode")
    } else {
        approval
    };
    let risk = json_text(method, "risk");
    let risk = if risk.is_empty() {
        json_text(method, "risk_level")
    } else {
        risk
    };
    if approval == "user" {
        return Some("user approval is required before invocation".to_string());
    }
    if ["payment", "rights", "actuator", "privileged"].contains(&risk) {
        return Some(format!("{risk} risk requires explicit user approval"));
    }
    None
}

fn parse_cli_invoke_input(input: &str, method: &serde_json::Value) -> Result<serde_json::Value> {
    let raw = input.trim();
    if raw.is_empty() {
        return Ok(serde_json::json!({}));
    }
    if raw.starts_with('{') || raw.starts_with('[') {
        return serde_json::from_str(raw).map_err(Into::into);
    }
    if json_text(method, "resource") == "elastos://capsules/*"
        && json_text(method, "operation") == "launch"
    {
        return Ok(serde_json::json!({ "target": raw }));
    }
    anyhow::bail!("input must be JSON for this affordance")
}

fn handle_shared_line_command(input: &str, snapshot: &HomeSnapshot) -> Result<bool> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(false);
    };
    let arg = parts.collect::<Vec<_>>().join(" ");
    let name = normalize_contract_command(raw_name);
    match name.as_str() {
        "home" => {
            render_line_dashboard(snapshot)?;
        }
        "apps" => {
            print_cli_section(snapshot, "Apps", &apps_summary_lines(snapshot));
        }
        "inbox" => {
            print_cli_inbox(snapshot);
        }
        "people" => {
            print_cli_people(snapshot);
        }
        "mywebsite" => {
            print_cli_mywebsite(snapshot);
        }
        "wallet" => {
            print_cli_wallet(snapshot);
        }
        "exits" => {
            print_cli_services(snapshot, "remote_exit");
        }
        "system" => {
            print_cli_system(snapshot, &arg);
        }
        "debug" => {
            print_cli_debug(snapshot, &arg);
        }
        "help" => {
            print_cli_help_topic(&arg);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn people_line_action(input: &str, snapshot: &HomeSnapshot) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    if normalize_contract_command(raw_name) != "people" {
        return Ok(None);
    }

    let mut args = parts.collect::<Vec<_>>();
    if normalize_lookup(raw_name) == "discovery" {
        args.insert(0, "discovery");
    }
    if args.is_empty() {
        return Ok(None);
    }

    let action_id = match normalize_lookup(args[0]).as_str() {
        "discovery" => match args.get(1).map(|value| normalize_lookup(value)).as_deref() {
            Some("on" | "enable" | "start") => "people-discovery-enable".to_string(),
            Some("off" | "disable" | "stop") => "people-discovery-disable".to_string(),
            Some("refresh" | "reload") => "people-discovery-refresh".to_string(),
            _ => anyhow::bail!("usage: people discovery on|off|refresh"),
        },
        "request" | "add" => {
            let Some(peer_id) = args
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                anyhow::bail!("usage: people request <peer-id>");
            };
            format!("people-request-peer:{peer_id}")
        }
        "accept" => {
            let Some(request_id) = args
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                anyhow::bail!("usage: people accept <request-id>");
            };
            format!("people-accept-request:{request_id}")
        }
        "remove" | "delete" => {
            let Some(contact_id) = args
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                anyhow::bail!("usage: people remove <contact-id>");
            };
            format!("people-remove-contact:{contact_id}")
        }
        "message" | "chat" => {
            let Some(contact_id) = args
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                anyhow::bail!("usage: people message <contact-id>");
            };
            format!("people-message:{contact_id}")
        }
        _ => return Ok(None),
    };

    people_actions(snapshot)
        .into_iter()
        .find(|action| action.id == action_id && action.ready)
        .map(|action| action.id)
        .ok_or_else(|| {
            anyhow!("People action is not available in the current Home snapshot: {action_id}")
        })
        .map(Some)
}

fn mywebsite_line_action(input: &str) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    if normalize_contract_command(raw_name) != "mywebsite" {
        return Ok(None);
    }

    let Some(raw_verb) = parts.next() else {
        return Ok(None);
    };
    let verb = normalize_lookup(raw_verb);
    match verb.as_str() {
        "status" => Ok(None),
        "stage" => {
            let path = parts.collect::<Vec<_>>().join(" ");
            let path = path.trim();
            if path.is_empty() {
                anyhow::bail!("usage: mywebsite stage <dir>");
            }
            Ok(Some(format!("site-stage:{path}")))
        }
        "preview" | "serve" => Ok(Some("site-local".to_string())),
        "publish" | "public" | "go-public" => Ok(Some("site-ephemeral".to_string())),
        "open" => Ok(Some("site-open".to_string())),
        _ => anyhow::bail!(
            "unknown MyWebSite command: {raw_verb}. Try status, stage <dir>, preview, publish, or open"
        ),
    }
}

fn system_line_action(input: &str, _snapshot: &HomeSnapshot) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    if normalize_contract_command(raw_name) != "system" {
        return Ok(None);
    }

    let Some(raw_topic) = parts.next() else {
        return Ok(None);
    };
    if normalize_system_topic(raw_topic) != "shell" {
        return Ok(None);
    }

    let Some(raw_target) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        anyhow::bail!("usage: system shell home-gui");
    }

    let target = normalize_shell_target(raw_target);
    if target != "home-gui" {
        anyhow::bail!("unsupported shell target `{raw_target}`; use `system shell home-gui`");
    }
    Ok(Some("shell-switch:home-gui".to_string()))
}

fn normalize_shell_target(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "gui" | "desktop" | "home" | "home-gui" => "home-gui".to_string(),
        "cli" | "terminal" | "home-cli" => "home-cli".to_string(),
        other => other.to_string(),
    }
}

fn print_cli_system(snapshot: &HomeSnapshot, arg: &str) {
    let mut parts = arg.split_whitespace();
    let topic = parts.next().map(normalize_system_topic).unwrap_or_default();
    match topic.as_str() {
        "" => print_cli_section(snapshot, "System", &system_settings_lines(snapshot)),
        "shell" => print_cli_section(snapshot, "System Shell", &system_shell_lines(snapshot)),
        "source" => print_cli_section(snapshot, "Trusted Source", &system_source_lines(snapshot)),
        "updates" => print_cli_section(snapshot, "Updates", &system_update_lines(snapshot)),
        "services" => print_cli_section(snapshot, "Services", &system_service_lines(snapshot)),
        "identity" => print_cli_section(snapshot, "Identity", &system_identity_lines(snapshot)),
        "diagnostics" => {
            print_cli_section(snapshot, "Diagnostics", &system_diagnostics_lines(snapshot))
        }
        _ => {
            print_cli_page_header(snapshot, "System");
            println!("Unknown System topic: {arg}");
            println!("  Try: system shell, source, updates, services, identity, diagnostics");
        }
    }
}

fn normalize_system_topic(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "shells" | "active-shell" | "home-shell" => "shell".to_string(),
        "source" | "sources" | "trusted-source" | "seed" => "source".to_string(),
        "update" | "updates" | "upgrade" | "release" => "updates".to_string(),
        "service" | "services" | "offers" => "services".to_string(),
        "id" | "identity" | "profile" | "auth" | "security" => "identity".to_string(),
        "diag" | "diagnostic" | "diagnostics" | "health" => "diagnostics".to_string(),
        other => other.to_string(),
    }
}

fn print_cli_debug(snapshot: &HomeSnapshot, arg: &str) {
    let mut parts = arg.split_whitespace();
    let Some(raw_name) = parts.next() else {
        print_cli_debug_help(snapshot);
        return;
    };
    let rest = parts.collect::<Vec<_>>().join(" ");
    match normalize_debug_command(raw_name).as_str() {
        "capsules" => print_cli_capsules(snapshot),
        "inspect" => print_cli_inspect(snapshot, &rest),
        "affordances" => print_cli_affordances(snapshot, &rest),
        "gates" => print_cli_gates(snapshot, &rest),
        "audit" => print_cli_audit(snapshot, &rest),
        "people" => {
            print_cli_section(snapshot, "Debug People", &people_debug_lines(snapshot));
            print_cli_contacts(snapshot);
        }
        "spaces" => print_cli_spaces(snapshot, raw_name, &rest),
        "services" => print_cli_services(snapshot, ""),
        "browser" => print_cli_browser(snapshot),
        "contract" => print_cli_contract(snapshot),
        "terminal" => print_cli_terminal_contract(snapshot),
        _ => {
            println!("Unknown debug topic: {raw_name}");
            print_cli_debug_help(snapshot);
        }
    }
}

fn print_cli_debug_help(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Debug");
    println!("Developer facts are hidden from the default Home CLI surface.");
    println!();
    println!("Debug Topics");
    println!("  debug capsules              installed capsule catalog");
    println!("  debug inspect <capsule>     catalog projection for one capsule");
    println!("  debug affordances [capsule] declared methods and interfaces");
    println!("  debug gates [capsule]       gate and consent descriptors");
    println!("  debug audit <capsule>       provenance and trust facts");
    println!("  debug people                contacts/discovery projection");
    println!("  debug spaces [root]         root and WebSpace projection");
    println!("  debug services              local and remote service offers");
    println!("  debug browser               Browser target and exit facts");
    println!("  debug terminal              Runtime PTY terminal contract");
    println!("  debug contract              shared capsule interface model");
}

fn normalize_debug_command(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "caps" | "catalog" => "capsules".to_string(),
        "affordance" | "interface" | "interfaces" | "ifaces" => "affordances".to_string(),
        "gate" => "gates".to_string(),
        "trust" | "provenance" => "audit".to_string(),
        "roots" | "places" | "mywebsite" | "public" | "local" | "webspaces" => "spaces".to_string(),
        "shortcuts" | "keys" => "terminal".to_string(),
        "model" | "interface-contract" => "contract".to_string(),
        other => other.to_string(),
    }
}

fn catalog_capsules(snapshot: &HomeSnapshot) -> &[serde_json::Value] {
    snapshot
        .capsule_catalog
        .as_ref()
        .and_then(|catalog| catalog.get("capsules"))
        .and_then(|capsules| capsules.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn interface_registry_entries(snapshot: &HomeSnapshot) -> &[serde_json::Value] {
    snapshot
        .capsule_interfaces
        .as_ref()
        .and_then(|registry| registry.get("interfaces"))
        .and_then(|interfaces| interfaces.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn json_text<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value.get(key).and_then(|item| item.as_str()).unwrap_or("")
}

fn json_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|item| item.as_bool())
        .unwrap_or(false)
}

fn json_array_len(value: &serde_json::Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(|item| item.as_array())
        .map(Vec::len)
        .unwrap_or_default()
}

fn projection_surface_state(capsule: &serde_json::Value, surface: &str) -> String {
    capsule
        .get("projection")
        .and_then(|projection| projection.get(surface))
        .and_then(|surface| surface.get("state"))
        .and_then(|state| state.as_str())
        .unwrap_or("unknown")
        .to_string()
}

fn projection_surface_note(capsule: &serde_json::Value, surface: &str) -> String {
    capsule
        .get("projection")
        .and_then(|projection| projection.get(surface))
        .and_then(|surface| surface.get("note"))
        .and_then(|note| note.as_str())
        .unwrap_or("")
        .to_string()
}

fn capsule_matches(capsule: &serde_json::Value, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    [
        json_text(capsule, "name"),
        json_text(capsule, "title"),
        json_text(capsule, "launch_target"),
    ]
    .iter()
    .any(|value| value.to_lowercase() == needle)
}

fn find_capsule_fact<'a>(snapshot: &'a HomeSnapshot, query: &str) -> Option<&'a serde_json::Value> {
    catalog_capsules(snapshot)
        .iter()
        .find(|capsule| capsule_matches(capsule, query))
}

fn require_capsule_arg<'a>(arg: &'a str, command: &str) -> Option<&'a str> {
    let query = arg.trim();
    if query.is_empty() {
        println!();
        println!("Usage: {command} <capsule>");
        return None;
    }
    Some(query)
}

fn print_cli_capsules(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Capsules");
    println!("Capsules");
    let capsules = catalog_capsules(snapshot);
    if capsules.is_empty() {
        println!("  Capsule catalog facts are not available in this snapshot.");
        return;
    }
    if let Some(counts) = snapshot
        .capsule_catalog
        .as_ref()
        .and_then(|catalog| catalog.get("counts"))
    {
        println!(
            "  total {} · installed {} · launchable {} · interfaces {} · methods {}",
            counts
                .get("total")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("installed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("launchable")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("interfaces")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("methods")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        );
    }
    for capsule in capsules.iter().take(18) {
        println!(
            "  {:<24} {:<9} cli={} gates={} {}",
            json_text(capsule, "name"),
            json_text(capsule, "role"),
            projection_surface_state(capsule, "cli"),
            projection_surface_state(capsule, "gates"),
            if json_bool(capsule, "launchable") {
                "launchable"
            } else {
                "facts"
            }
        );
    }
    if capsules.len() > 18 {
        println!("  ... {} more", capsules.len() - 18);
    }
}

fn print_cli_inspect(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Inspect");
    let Some(query) = require_capsule_arg(arg, "inspect") else {
        return;
    };
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("inspect: capsule not found: {query}");
        return;
    };
    println!("Capsule {}", json_text(capsule, "name"));
    println!("  title       {}", json_text(capsule, "title"));
    println!(
        "  role/type   {}/{}",
        json_text(capsule, "role"),
        json_text(capsule, "type")
    );
    println!("  state       {}", json_text(capsule, "state"));
    println!("  trust       {}", json_text(capsule, "trust_state"));
    if !json_text(capsule, "route").is_empty() {
        println!("  route       {}", json_text(capsule, "route"));
    }
    for surface in [
        "web",
        "cli",
        "facts",
        "affordances",
        "gates",
        "audit_mirror",
        "carrier",
    ] {
        println!(
            "  {:<12} {}",
            surface,
            projection_surface_state(capsule, surface)
        );
    }
}

fn cli_interface_entries_for<'a>(
    snapshot: &'a HomeSnapshot,
    capsule_name: Option<&str>,
) -> Vec<&'a serde_json::Value> {
    let entries = interface_registry_entries(snapshot);
    if entries.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|entry| {
            capsule_name
                .map(|name| json_text(entry, "capsule") == name)
                .unwrap_or(true)
        })
        .collect()
}

fn interface_descriptor(entry: &serde_json::Value) -> &serde_json::Value {
    entry.get("interface").unwrap_or(entry)
}

fn print_cli_affordances(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Affordances");
    let capsule = if arg.trim().is_empty() {
        None
    } else {
        match find_capsule_fact(snapshot, arg.trim()) {
            Some(capsule) => Some(capsule),
            None => {
                println!("affordances: capsule not found: {}", arg.trim());
                return;
            }
        }
    };
    let capsule_name = capsule.map(|capsule| json_text(capsule, "name"));
    let entries = cli_interface_entries_for(snapshot, capsule_name);
    println!("Affordances");
    if entries.is_empty() {
        println!("  No declared affordances in this snapshot.");
        return;
    }
    for entry in entries.iter().take(16) {
        let descriptor = interface_descriptor(entry);
        println!(
            "  {} :: {}",
            json_text(entry, "capsule"),
            json_text(descriptor, "id")
        );
        if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
            for method in methods.iter().take(8) {
                println!(
                    "    - {:<24} risk={} approval={}",
                    json_text(method, "id"),
                    json_text(method, "risk"),
                    json_text(method, "approval")
                );
            }
        }
    }
    if entries.len() > 16 {
        println!("  ... {} more interfaces", entries.len() - 16);
    }
}

fn print_cli_gates(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Gates");
    let query = arg.trim();
    if query.is_empty() {
        let entries = cli_interface_entries_for(snapshot, None);
        println!("Gates");
        if entries.is_empty() {
            println!("  No declared method gates in this snapshot.");
            return;
        }
        for entry in entries.iter().take(16) {
            print_cli_gate_entry(entry, 8);
        }
        if entries.len() > 16 {
            println!("  ... {} more interfaces", entries.len() - 16);
        }
        return;
    }
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("gates: capsule not found: {query}");
        return;
    };
    let capsule_name = json_text(capsule, "name");
    println!("Gates {capsule_name}");
    println!(
        "  projection {}",
        projection_surface_state(capsule, "gates")
    );
    let note = projection_surface_note(capsule, "gates");
    if !note.is_empty() {
        println!("  note       {note}");
    }
    let entries = cli_interface_entries_for(snapshot, Some(capsule_name));
    if entries.is_empty() {
        println!("  No declared method gates.");
        return;
    }
    for entry in entries {
        print_cli_gate_entry(entry, usize::MAX);
    }
}

fn print_cli_gate_entry(entry: &serde_json::Value, method_limit: usize) {
    let descriptor = interface_descriptor(entry);
    println!(
        "  {} :: {}",
        json_text(entry, "capsule"),
        json_text(descriptor, "id")
    );
    if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
        for method in methods.iter().take(method_limit) {
            println!(
                "    - {:<24} risk={} approval={}",
                json_text(method, "id"),
                json_text(method, "risk"),
                json_text(method, "approval")
            );
        }
    }
}

fn print_cli_audit(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Audit");
    let Some(query) = require_capsule_arg(arg, "audit") else {
        return;
    };
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("audit: capsule not found: {query}");
        return;
    };
    println!("Audit {}", json_text(capsule, "name"));
    println!("  trust      {}", json_text(capsule, "trust_state"));
    println!("  signature  {}", json_text(capsule, "signature_state"));
    println!("  cid        {}", json_text(capsule, "cid_state"));
    println!("  payment    {}", json_text(capsule, "payment_state"));
    println!("  drm        {}", json_text(capsule, "drm_state"));
    println!("  source     {}", json_text(capsule, "source"));
    println!("  interfaces {}", json_array_len(capsule, "interfaces"));
    let note = projection_surface_note(capsule, "audit_mirror");
    if !note.is_empty() {
        println!("  mirror     {note}");
    }
}

fn print_cli_section(snapshot: &HomeSnapshot, title: &str, lines: &[String]) {
    print_cli_page_header(snapshot, title);
    println!("{title}");
    if lines.is_empty() {
        println!("  (none)");
        return;
    }
    for line in lines {
        println!("  {line}");
    }
}

fn print_cli_spaces(snapshot: &HomeSnapshot, raw_name: &str, arg: &str) {
    let query = space_query_for_command(raw_name, arg);
    if query.is_empty() {
        print_cli_section(snapshot, "Spaces", &spaces_summary_lines(snapshot));
        return;
    }

    let normalized = normalize_lookup(&query);
    let Some(root) = snapshot
        .roots
        .iter()
        .find(|root| normalize_lookup(&root.name) == normalized)
    else {
        print_cli_page_header(snapshot, "Spaces");
        println!("Spaces");
        println!("  Unknown root: {}", query.trim());
        println!("  Try: MyWebSite, Public, Local, WebSpaces");
        return;
    };

    let title = format!("Spaces / {}", root.name);
    print_cli_section(snapshot, &title, &space_detail_lines(root, snapshot, 80));
}

fn space_query_for_command(raw_name: &str, arg: &str) -> String {
    let trimmed = arg.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    let normalized = normalize_lookup(raw_name);
    match normalized.as_str() {
        "mywebsite" => "MyWebSite".to_string(),
        "public" => "Public".to_string(),
        "local" => "Local".to_string(),
        "webspaces" => "WebSpaces".to_string(),
        _ => String::new(),
    }
}

fn print_cli_mywebsite(snapshot: &HomeSnapshot) {
    print_cli_section(snapshot, "MyWebSite", &mywebsite_task_lines(snapshot));
}

fn mywebsite_task_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("Status   {}", website_summary(snapshot)),
        "Stage    mywebsite stage <dir>".to_string(),
        format!(
            "Preview  mywebsite preview ({})",
            action_state_label(action_by_id(snapshot, "site-local"))
        ),
        format!(
            "Publish  mywebsite publish ({})",
            action_state_label(action_by_id(snapshot, "site-ephemeral"))
        ),
        format!(
            "Open     mywebsite open ({})",
            action_state_label(action_by_id(snapshot, "site-open"))
        ),
    ];

    if let Some(url) = snapshot.site.local_url.as_deref() {
        lines.push(format!("Preview  {}", url.trim_end_matches('/')));
    }
    if let Some(release) = snapshot.site.active_release.as_deref() {
        let live = snapshot
            .site
            .active_channel
            .as_deref()
            .map(|channel| format!("{} on {}", release, channel))
            .unwrap_or_else(|| release.to_string());
        lines.push(format!("Live     {live}"));
    } else if snapshot.site.release_count > 0 {
        lines.push(format!("Releases {}", snapshot.site.release_count));
    }
    if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
        lines.push(format!("Bundle   elastos://{}", cid));
    }
    if !snapshot.site.staged {
        lines.push("Next     stage a directory containing index.html".to_string());
    }
    lines
}

fn print_cli_inbox(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Inbox");
    println!("Inbox");
    println!(
        "  Attention: {} waiting / {} unread",
        snapshot.notifications.attention_count, snapshot.notifications.unread_count
    );
    let entries = notification_entries(snapshot);
    if entries.is_empty() {
        println!("  No inbox entries waiting.");
        return;
    }
    for entry in entries.iter().take(8) {
        println!(
            "  - [{}{}] {}",
            entry.severity,
            if entry.read { "" } else { ", new" },
            entry.title
        );
        if !entry.body.trim().is_empty() {
            println!("    {}", entry.body);
        }
    }
}

fn print_cli_people(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "People");
    println!("People");
    for line in people_overview_lines(snapshot, 80) {
        println!("  {line}");
    }
    println!();
    println!("Actions");
    let actions = people_actions(snapshot);
    if actions.is_empty() {
        println!("  No People actions are available right now.");
    } else {
        for action in actions.iter().take(12) {
            println!(
                "  - {} [{}]",
                action.label,
                if action.ready { "ready" } else { "setup" }
            );
            println!("    {}", action.command);
            if let Some(reason) = action.reason.as_deref() {
                println!("    {reason}");
            }
        }
    }
}

fn print_cli_wallet(snapshot: &HomeSnapshot) {
    let wallet_capsules = catalog_capsules(snapshot)
        .iter()
        .filter(|capsule| json_text(capsule, "name").contains("wallet"))
        .map(|capsule| json_text(capsule, "name"))
        .collect::<Vec<_>>();
    let wallet_entries = notification_entries(snapshot)
        .into_iter()
        .filter(|entry| {
            [
                entry.source_app.as_str(),
                entry.kind.as_str(),
                entry.title.as_str(),
                entry.body.as_str(),
                entry
                    .action_ref
                    .as_ref()
                    .map(|action| action.app.as_str())
                    .unwrap_or(""),
            ]
            .join(" ")
            .to_lowercase()
            .contains("wallet")
                || [
                    entry.kind.as_str(),
                    entry.title.as_str(),
                    entry.body.as_str(),
                ]
                .join(" ")
                .to_lowercase()
                .contains("approval")
                || entry.body.to_lowercase().contains("sign")
        })
        .collect::<Vec<_>>();

    print_cli_page_header(snapshot, "Wallet");
    println!("Wallet");
    println!(
        "  capsules  {}",
        if wallet_capsules.is_empty() {
            "(none)".to_string()
        } else {
            wallet_capsules.join(", ")
        }
    );
    println!("  requests  {}", wallet_entries.len());
    for entry in wallet_entries.iter().take(8) {
        let action = entry
            .action_ref
            .as_ref()
            .map(|action| format!(" -> {}:{}", action.app, action.action_id))
            .unwrap_or_default();
        println!("  - {}{}", entry.title, action);
    }
}

fn print_cli_services(snapshot: &HomeSnapshot, kind_filter: &str) {
    let offers = cli_service_offers(snapshot, kind_filter);
    let title = if kind_filter == "remote_exit" {
        "Browser Exits"
    } else {
        "Services"
    };
    print_cli_page_header(snapshot, title);
    println!("{title}");
    if offers.is_empty() {
        if kind_filter == "remote_exit" {
            println!("  No Browser Exit offers visible in this snapshot.");
        } else {
            for line in compact_system_lines(snapshot) {
                println!("  {line}");
            }
            println!("  No service offers visible in this snapshot.");
        }
        return;
    }
    for offer in offers.iter().take(12) {
        let id = first_json_text(offer, &["offer_id", "id"]);
        let kind = first_json_text(offer, &["service_kind", "kind"]);
        let name = first_json_text(offer, &["service_display_name", "display_name"]);
        let status = first_json_text(offer, &["status"]);
        let route = first_json_text(offer, &["route"]);
        println!(
            "  {:<30} {:<12} {:<12} {}{}",
            if id.is_empty() { "offer" } else { id },
            if kind.is_empty() { "service" } else { kind },
            if status.is_empty() {
                "available"
            } else {
                status
            },
            if name.is_empty() { id } else { name },
            if route.is_empty() {
                String::new()
            } else {
                format!(" -> {route}")
            }
        );
    }
    if offers.len() > 12 {
        println!("  ... {} more offers", offers.len() - 12);
    }
}

fn print_cli_browser(snapshot: &HomeSnapshot) {
    let browser = find_capsule_fact(snapshot, "browser");
    let engine = cli_service_offers(snapshot, "browser_engine")
        .into_iter()
        .next();
    let exits = cli_service_offers(snapshot, "remote_exit");
    print_cli_page_header(snapshot, "Browser");
    println!("Browser");
    println!(
        "  target    {}",
        browser
            .map(|capsule| json_text(capsule, "state"))
            .filter(|state| !state.is_empty())
            .unwrap_or("missing")
    );
    println!(
        "  route     {}",
        browser
            .map(|capsule| json_text(capsule, "route"))
            .filter(|route| !route.is_empty())
            .unwrap_or("/apps/browser/")
    );
    println!(
        "  engine    {}",
        engine
            .map(|offer| first_json_text(offer, &["status", "display_name", "offer_id"]))
            .filter(|text| !text.is_empty())
            .unwrap_or("unknown")
    );
    println!("  exits     {}", exits.len());
    for exit in exits.iter().take(8) {
        let name = first_json_text(exit, &["display_name", "offer_id"]);
        let status = first_json_text(exit, &["status"]);
        println!(
            "  - {} ({})",
            if name.is_empty() {
                "Browser Exit"
            } else {
                name
            },
            if status.is_empty() { "unknown" } else { status }
        );
    }
}

fn cli_service_offers<'a>(
    snapshot: &'a HomeSnapshot,
    kind_filter: &str,
) -> Vec<&'a serde_json::Value> {
    let Some(services) = snapshot.services.as_ref() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut offers = Vec::new();
    for key in [
        "local_offers",
        "remote_offers",
        "available_local_offers",
        "available_remote_offers",
        "service_offers",
    ] {
        if let Some(items) = services.get(key).and_then(|value| value.as_array()) {
            for offer in items {
                let kind = first_json_text(offer, &["service_kind", "kind"]);
                if !kind_filter.is_empty() && kind != kind_filter {
                    continue;
                }
                let id = first_json_text(offer, &["offer_id", "id", "service_uri"]);
                let dedupe = if id.is_empty() {
                    offer.to_string()
                } else {
                    id.to_string()
                };
                if seen.insert(dedupe) {
                    offers.push(offer);
                }
            }
        }
    }
    offers
}

fn first_json_text<'a>(value: &'a serde_json::Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| {
            let text = json_text(value, key);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .unwrap_or("")
}

fn print_cli_contacts(snapshot: &HomeSnapshot) {
    if snapshot.people.contacts.is_empty() {
        println!("  Contacts   No accepted ElastOS contacts yet.");
        return;
    }
    println!("  Contacts");
    for contact in snapshot.people.contacts.iter().take(8) {
        let device = contact
            .device_label
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!(" on {value}"))
            .unwrap_or_default();
        println!(
            "  - {}{} · {}",
            people_contact_display_name(contact, "Person"),
            device,
            if contact.relationship.trim().is_empty() {
                "connected"
            } else {
                contact.relationship.as_str()
            }
        );
    }
}

fn print_cli_contract(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Contract");
    println!("Capsule Interface Contract");
    println!("  Home:       Runtime-owned facts, gates, active-shell state, and host routing");
    println!("  home-gui:   GUI shell surface over the same Home facts");
    println!("  home-cli:   terminal shell surface over the same Home facts");
    println!("  entrypoint: `elastos home` runs this home-cli capsule over local Home state");
    println!("  facts:      Runtime catalog/interface streams are the shared truth");
    println!("  affordance: descriptors are not grants");
    println!("  gates:      Runtime/provider/Inbox gates still decide access");
    println!("  carrier:    capsule-to-capsule actions stay provider/Carrier intents");
}

fn print_cli_terminal_contract(snapshot: &HomeSnapshot) {
    let contract = command_contract();
    let terminal = contract.terminal;
    print_cli_page_header(snapshot, "Terminal");
    println!("Home CLI terminal contract");
    println!(
        "  renderer  {}",
        terminal
            .renderer
            .as_deref()
            .unwrap_or("Runtime-owned PTY terminal projection")
    );
    println!(
        "  entrypoint {}",
        terminal
            .entrypoint
            .as_deref()
            .unwrap_or("snapshot dashboard with shared high-level command vocabulary")
    );
    println!(
        "  transport {} ({})",
        terminal.transport.as_deref().unwrap_or("runtime snapshot"),
        terminal
            .transport_scope
            .as_deref()
            .unwrap_or("local_runtime_adapter")
    );
    println!(
        "  input     {}",
        terminal
            .input
            .as_deref()
            .unwrap_or("keyboard, paste, mouse, and resize events -> Runtime-owned PTY stream")
    );
    println!(
        "  PTY       {}",
        terminal.pty.as_deref().unwrap_or("not attached")
    );
    println!(
        "  xterm     {}",
        terminal
            .xterm
            .as_deref()
            .unwrap_or("capsule-local xterm.js renderer over Runtime PTY stream")
    );
    if !contract.controls.is_empty() {
        println!();
        println!("Controls");
        for control in contract.controls {
            println!("  {:<9} {}", control.key, control.description);
        }
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            tab: Tab::Home,
            home_index: 0,
            inbox_index: 0,
            people_index: 0,
            app_index: 0,
            system_index: 0,
            show_help: false,
            notice: None,
        }
    }
}

impl TuiState {
    fn next_tab(&mut self) {
        let current = DEFAULT_TABS
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        self.tab = DEFAULT_TABS[(current + 1) % DEFAULT_TABS.len()];
    }

    fn prev_tab(&mut self) {
        let current = DEFAULT_TABS
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        self.tab = DEFAULT_TABS[(current + DEFAULT_TABS.len() - 1) % DEFAULT_TABS.len()];
    }

    fn move_prev(&mut self, snapshot: &HomeSnapshot) {
        match self.tab {
            Tab::Home => {
                if !home_action_indices(snapshot).is_empty() {
                    self.home_index = self.home_index.saturating_sub(1);
                }
            }
            Tab::Inbox => {
                if !notification_indices(snapshot).is_empty() {
                    self.inbox_index = self.inbox_index.saturating_sub(1);
                }
            }
            Tab::People => {
                if !people_actions(snapshot).is_empty() {
                    self.people_index = self.people_index.saturating_sub(1);
                }
            }
            Tab::Apps => {
                if !app_entries(snapshot).is_empty() {
                    self.app_index = self.app_index.saturating_sub(1);
                }
            }
            Tab::System => {
                if !system_actions(snapshot).is_empty() {
                    self.system_index = self.system_index.saturating_sub(1);
                }
            }
        }
    }

    fn move_next(&mut self, snapshot: &HomeSnapshot) {
        match self.tab {
            Tab::Home => {
                let items = home_action_indices(snapshot);
                if !items.is_empty() {
                    self.home_index = (self.home_index + 1).min(items.len() - 1);
                }
            }
            Tab::Inbox => {
                let items = notification_indices(snapshot);
                if !items.is_empty() {
                    self.inbox_index = (self.inbox_index + 1).min(items.len() - 1);
                }
            }
            Tab::People => {
                let items = people_actions(snapshot);
                if !items.is_empty() {
                    self.people_index = (self.people_index + 1).min(items.len() - 1);
                }
            }
            Tab::Apps => {
                let items = app_entries(snapshot);
                if !items.is_empty() {
                    self.app_index = (self.app_index + 1).min(items.len() - 1);
                }
            }
            Tab::System => {
                let items = system_actions(snapshot);
                if !items.is_empty() {
                    self.system_index = (self.system_index + 1).min(items.len() - 1);
                }
            }
        }
    }

    fn activate(&self, snapshot: &HomeSnapshot) -> Option<String> {
        match self.tab {
            Tab::Home => selected_action(snapshot, &home_action_indices(snapshot), self.home_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::Inbox => selected_notification_action(snapshot, self.inbox_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::People => selected_people_action(snapshot, self.people_index)
                .filter(|action| action.ready)
                .map(|action| action.id),
            Tab::Apps => selected_app_action(snapshot, self.app_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::System => selected_system_action(snapshot, self.system_index)
                .filter(|action| action.ready)
                .map(|action| action.id),
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, cols: usize, snapshot: &HomeSnapshot) -> bool {
        if event.released {
            return false;
        }

        match event.button {
            64 => {
                self.move_prev(snapshot);
                true
            }
            65 => {
                self.move_next(snapshot);
                true
            }
            0 if event.y == TUI_TAB_ROW => {
                let tab_count = DEFAULT_TABS.len() as u16;
                let cols = cols.max(tab_count as usize) as u16;
                let slot = event.x.saturating_sub(1).saturating_mul(tab_count) / cols;
                self.tab = DEFAULT_TABS[slot.min(tab_count - 1) as usize];
                true
            }
            _ => false,
        }
    }
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let guard = Self::new()?;
        print!("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H");
        io::stdout().flush()?;
        Ok(guard)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn new() -> Result<Self> {
        Ok(Self {
            original_termios: enable_terminal_raw_mode()?,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn new() -> Result<Self> {
        Ok(Self {})
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(original) = self.original_termios.as_ref() {
            let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original) };
        }
        let _ = write!(io::stdout(), "\x1b[?1006l\x1b[?1000l\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn enable_terminal_raw_mode() -> Result<Option<libc::termios>> {
    if !io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    let rc = unsafe { libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) };
    if rc != 0 {
        return Err(anyhow!(
            "failed to read terminal attributes: {}",
            io::Error::last_os_error()
        ));
    }

    let original = unsafe { termios.assume_init() };
    let mut raw = original;
    unsafe {
        libc::cfmakeraw(&mut raw);
    }
    let rc = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };
    if rc != 0 {
        return Err(anyhow!(
            "failed to set terminal raw mode: {}",
            io::Error::last_os_error()
        ));
    }

    Ok(Some(original))
}

fn read_ui_key() -> Result<UiKey> {
    if !stdin_has_input(LIVE_REFRESH_POLL_MS)? {
        return Ok(UiKey::None);
    }
    let byte = read_stdin_byte()?;
    let key = match byte {
        b'q' | b'Q' => UiKey::Quit,
        b'r' | b'R' => UiKey::Refresh,
        b'm' | b'M' => UiKey::MarkRead,
        b'd' | b'D' => UiKey::Dismiss,
        b'?' => UiKey::Help,
        b'\n' | b'\r' => UiKey::Enter,
        b'\t' | b'l' | b'L' => UiKey::Right,
        b'h' | b'H' => UiKey::Left,
        b'j' | b'J' => UiKey::Down,
        b'k' | b'K' => UiKey::Up,
        b'1'..=b'9' => UiKey::Digit((byte - b'0') as usize),
        27 => read_escape_sequence()?,
        _ => UiKey::None,
    };

    if home_debug_keys() {
        eprintln!("[home-keys] byte={byte} parsed={key:?}");
    }

    Ok(key)
}

fn read_escape_sequence() -> Result<UiKey> {
    std::thread::sleep(ESCAPE_SEQUENCE_SETTLE_WINDOW);
    let mut seq = Vec::with_capacity(ESCAPE_SEQUENCE_MAX_BYTES);
    while seq.len() < ESCAPE_SEQUENCE_MAX_BYTES {
        if !stdin_has_input(ESCAPE_SEQUENCE_BYTE_TIMEOUT_MS)? {
            break;
        }
        let byte = read_stdin_byte()?;
        seq.push(byte);
        if is_escape_sequence_complete(&seq) {
            break;
        }
    }

    let key = escape_sequence_key(&seq);
    if home_debug_keys() {
        eprintln!("[home-keys] esc-seq={seq:?} parsed={key:?}");
    }
    Ok(key)
}

fn parse_escape_sequence_bytes(seq: &[u8]) -> UiKey {
    if let Some(key) = parse_sgr_mouse_sequence(seq) {
        return key;
    }
    if let Some(key) = parse_legacy_mouse_sequence(seq) {
        return key;
    }

    let Some((&prefix, rest)) = seq.split_first() else {
        return UiKey::None;
    };
    let Some(&last) = rest.last() else {
        return UiKey::None;
    };

    match (prefix, last) {
        (b'[', b'A') | (b'O', b'A') => UiKey::Up,
        (b'[', b'B') | (b'O', b'B') => UiKey::Down,
        (b'[', b'C') | (b'O', b'C') => UiKey::Right,
        (b'[', b'D') | (b'O', b'D') => UiKey::Left,
        (b'[', b'Z') => UiKey::Left,
        _ => UiKey::None,
    }
}

fn escape_sequence_key(seq: &[u8]) -> UiKey {
    if seq.is_empty() {
        UiKey::Quit
    } else {
        parse_escape_sequence_bytes(seq)
    }
}

fn parse_legacy_mouse_sequence(seq: &[u8]) -> Option<UiKey> {
    if seq.len() < 5 || !seq.starts_with(b"[M") {
        return None;
    }
    let button = seq[2].checked_sub(32)? as u16;
    let x = seq[3].checked_sub(32)? as u16;
    let y = seq[4].checked_sub(32)? as u16;
    Some(UiKey::Mouse(MouseEvent {
        button,
        x,
        y,
        released: button == 3,
    }))
}

fn is_escape_sequence_complete(seq: &[u8]) -> bool {
    if seq.starts_with(b"[M") {
        return seq.len() >= 5;
    }
    seq.last()
        .copied()
        .is_some_and(is_escape_sequence_terminator)
}

fn parse_sgr_mouse_sequence(seq: &[u8]) -> Option<UiKey> {
    if seq.len() < 6 || !seq.starts_with(b"[<") {
        return None;
    }
    let released = match seq.last().copied()? {
        b'M' => false,
        b'm' => true,
        _ => return None,
    };
    let payload = std::str::from_utf8(&seq[2..seq.len().saturating_sub(1)]).ok()?;
    let mut parts = payload.split(';');
    let button = parts.next()?.parse::<u16>().ok()?;
    let x = parts.next()?.parse::<u16>().ok()?;
    let y = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(UiKey::Mouse(MouseEvent {
        button,
        x,
        y,
        released,
    }))
}

fn is_escape_sequence_terminator(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'~')
}

fn render_tui(snapshot: &HomeSnapshot, state: &TuiState) -> Result<()> {
    let cols = term_cols();
    let rows = term_rows();
    let screen = build_tui_screen(snapshot, state, cols, rows);

    print!("{}", screen);
    io::stdout().flush()?;
    Ok(())
}

fn build_tui_screen(snapshot: &HomeSnapshot, state: &TuiState, cols: usize, rows: usize) -> String {
    let cols = terminal_paint_cols(cols);
    let body_width = cols.saturating_sub(4);
    let mut screen = String::new();
    let mut body = String::new();
    // Steady-state redraws repaint from the home position and clear to the end of the
    // alternate screen. This avoids old tail lines surviving shorter frames without
    // bringing back the heavier full-screen clear on every keypress.
    screen.push_str("\x1b[H\x1b[J");
    push_screen_line(&mut screen, &render_tabs(state.tab, cols));
    push_screen_line(&mut screen, &rule(cols));

    if state.show_help {
        render_help_tab(&mut body, body_width);
    } else {
        match state.tab {
            Tab::Home => render_home_tab(&mut body, snapshot, state, body_width),
            Tab::Inbox => render_inbox_tab(&mut body, snapshot, state, body_width),
            Tab::People => render_people_tab(&mut body, snapshot, state, body_width),
            Tab::Apps => render_apps_tab(&mut body, snapshot, state, body_width),
            Tab::System => render_system_tab(&mut body, snapshot, state, body_width),
        }
    }

    if let Some(notice) = state
        .notice
        .as_deref()
        .or(snapshot.notice.as_deref())
        .filter(|notice| should_render_notice(notice))
    {
        push_screen_blank(&mut body);
        push_screen_line(&mut body, &section_title("Notice", cols));
        for line in wrap_text(notice, body_width) {
            push_screen_line(&mut body, &format!("  {}", line));
        }
    }

    let header_lines = 2usize;
    let footer_lines = 3usize;
    let body_rows = rows.saturating_sub(header_lines + footer_lines);
    let body_lines = push_bounded_screen_body(&mut screen, &body, body_rows, cols);
    if body_lines < body_rows {
        for _ in 0..(body_rows - body_lines) {
            push_screen_blank(&mut screen);
        }
    }

    push_screen_blank(&mut screen);
    push_screen_line(&mut screen, &rule(cols));
    push_screen_line(&mut screen, &fit_line(tui_footer_text(state), cols));
    trim_trailing_screen_newline(&mut screen);
    screen
}

fn tui_footer_text(state: &TuiState) -> &'static str {
    if state.show_help {
        TUI_HELP_FOOTER_TEXT
    } else {
        TUI_FOOTER_TEXT
    }
}

fn terminal_paint_cols(cols: usize) -> usize {
    // Leave the final terminal column untouched. xterm-compatible terminals can
    // enter autowrap after a full-width line, and the following CRLF may scroll
    // the first row off the viewport.
    cols.saturating_sub(1).max(20)
}

fn push_bounded_screen_body(
    screen: &mut String,
    body: &str,
    max_rows: usize,
    cols: usize,
) -> usize {
    if max_rows == 0 {
        return 0;
    }
    let lines = body.split_terminator("\r\n").collect::<Vec<_>>();
    if lines.len() <= max_rows {
        let rendered = lines.len();
        for line in lines {
            push_screen_line(screen, line);
        }
        return rendered;
    }

    let visible_rows = max_rows.saturating_sub(1);
    for line in lines.iter().take(visible_rows) {
        push_screen_line(screen, line);
    }
    push_screen_line(screen, &fit_line("  ...", cols));
    max_rows
}

fn trim_trailing_screen_newline(screen: &mut String) {
    if screen.ends_with("\r\n") {
        screen.truncate(screen.len().saturating_sub(2));
    }
}

fn render_help_tab(buf: &mut String, width: usize) {
    push_screen_line(buf, "  Home CLI Controls");
    push_screen_blank(buf);
    for (key, description) in TUI_HELP_LINES {
        let line = format!("  {:<12} {}", key, description);
        for wrapped in wrap_text(&line, width) {
            push_screen_line(buf, &wrapped);
        }
    }
}

fn render_home_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let text_width = total_width.saturating_sub(2);
    let primary_actions = quick_launch_action_indices(snapshot);
    let active_notice = current_notice(state, snapshot);
    for line in render_home_actions(snapshot, &primary_actions, state.home_index, text_width) {
        push_screen_line(buf, &format!("  {}", fit_line(&line, total_width)));
    }

    let alerts = alerts_lines(snapshot, text_width, active_notice);
    if !alerts.is_empty() {
        push_screen_blank(buf);
        push_screen_line(
            buf,
            &format!("  {}", fit_line("Needs attention", total_width)),
        );
        for line in alerts {
            push_screen_line(buf, &format!("  {}", fit_line(&line, total_width)));
        }
    }
}

fn render_inbox_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let entries = notification_entries(snapshot);
    let list = if entries.is_empty() {
        vec!["No inbox entries waiting.".to_string()]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                format!(
                    "{} {} [{}{}]",
                    selected_marker(idx == state.inbox_index),
                    entry.title,
                    entry.severity,
                    if entry.read { "" } else { ", new" }
                )
            })
            .collect::<Vec<_>>()
    };
    push_section_lines(&mut left, "Inbox", &list);

    let overview = vec![
        format!("Unread     {}", snapshot.notifications.unread_count),
        format!("Attention  {}", snapshot.notifications.attention_count),
        format!("Entries    {}", entries.len()),
    ];
    push_section_lines(&mut left, "Overview", &overview);

    if let Some(entry) = selected_notification(snapshot, state.inbox_index) {
        let mut details = vec![
            format!("Title      {}", entry.title),
            format!("Severity   {}", entry.severity),
            format!("Source     {}", entry.source_app),
            format!("State      {}", if entry.read { "read" } else { "unread" }),
        ];
        details.extend(wrap_with_label("Body", &entry.body, column_width));
        if let Some(action) = selected_notification_action(snapshot, state.inbox_index) {
            details.push(format!("Action     {}", action.label));
            details.push(format!(
                "ActionUse  {}",
                if action.ready { "ready" } else { "blocked" }
            ));
            details.push("Enter      run this inbox action and return here".to_string());
            if let Some(reason) = &action.reason {
                details.extend(wrap_with_label("Setup", reason, column_width));
            }
        } else if entry.action_ref.is_some() {
            details.push("Action     no longer available".to_string());
        } else {
            details.push("Action     informational only".to_string());
        }
        details.push("m          mark this inbox entry read".to_string());
        details.push("d          dismiss this inbox entry".to_string());
        push_section_lines(&mut right, "Selected", &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_people_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    push_section_lines(
        &mut left,
        "My Profile",
        &people_profile_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut left,
        "People",
        &people_contact_lines(snapshot, column_width),
    );

    push_section_lines(
        &mut right,
        "Discovery",
        &people_discovery_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut right,
        "Visible People",
        &people_visible_peer_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut right,
        "Requests",
        &people_request_lines(snapshot, column_width),
    );

    let people_actions = people_actions(snapshot);
    if !people_actions.is_empty() {
        let actions = people_actions
            .iter()
            .enumerate()
            .map(|(slot, action)| {
                format!(
                    "{} {} [{}]",
                    selected_marker(slot == state.people_index),
                    action.label,
                    if action.ready { "ready" } else { "setup" }
                )
            })
            .collect::<Vec<_>>();
        push_section_lines(&mut left, "Actions", &actions);
    }

    if let Some(action) = selected_people_action(snapshot, state.people_index) {
        let mut profile = vec![
            format!("Action     {}", action.label),
            format!(
                "State      {}",
                if action.ready { "ready" } else { "setup" }
            ),
            format!("Command    {}", action.command),
        ];
        if let Some(reason) = &action.reason {
            profile.extend(wrap_with_label("Prep", reason, column_width));
        } else {
            profile.push("Enter      run this People action and return home".to_string());
        }
        profile.extend(wrap_with_label("What", &action.description, column_width));
        push_section_lines(&mut right, "Selected Action", &profile);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_apps_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let entries = app_entries(snapshot);
    let list = render_app_list(&entries, state.app_index);
    push_section_lines(&mut left, "Apps", &list);

    if let Some(entry) = entries.get(state.app_index.min(entries.len().saturating_sub(1))) {
        let mut details = if entry.action_id == "chat-room" {
            chat_room_app_detail_lines(snapshot, entry, column_width)
        } else {
            let mut details = vec![
                format!("Surface    {}", entry.name),
                format!("State      {}", entry.state),
                format!("Category   {}", entry.category),
            ];
            details.extend(wrap_with_label(
                "What it does",
                &entry.description,
                column_width,
            ));
            details.extend(wrap_with_label("Command", &entry.command, column_width));
            details
        };
        if let Some(action) = selected_app_action(snapshot, state.app_index) {
            if action.ready {
                details.push(if entry.is_control {
                    "Enter      run this room action and return here".to_string()
                } else {
                    "Enter      launch from Home".to_string()
                });
            } else {
                details.push(if entry.is_control {
                    "Enter      room action not ready yet".to_string()
                } else {
                    "Enter      not ready from Home yet".to_string()
                });
                if let Some(reason) = &action.reason {
                    details.extend(wrap_with_label("Setup", reason, column_width));
                }
            }
        } else if entry.action_id == "chat-room" {
            details.push(
                "Enter      no direct launch; review the room controls listed below in Apps"
                    .to_string(),
            );
        } else {
            details.push("Enter      no direct launch from Home yet".to_string());
        }
        push_section_lines(&mut right, &entry.label, &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_system_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let actions = system_actions(snapshot);
    let action_lines = actions
        .iter()
        .enumerate()
        .map(|(slot, action)| {
            format!(
                "{} {} [{}]",
                selected_marker(slot == state.system_index),
                action.label,
                if action.ready { "ready" } else { "current" }
            )
        })
        .collect::<Vec<_>>();
    push_section_lines(&mut left, "Settings", &action_lines);
    push_section_lines(&mut left, "Trusted Source", &system_source_lines(snapshot));
    push_section_lines(&mut left, "Identity", &system_identity_lines(snapshot));

    if let Some(action) = selected_system_action(snapshot, state.system_index) {
        let mut details = vec![
            format!("Action     {}", action.label),
            format!(
                "State      {}",
                if action.ready { "ready" } else { "current" }
            ),
            format!("Command    {}", action.command),
        ];
        if let Some(reason) = &action.reason {
            details.extend(wrap_with_label("Info", reason, column_width));
        } else {
            details.push("Enter      run this System setting through Home".to_string());
        }
        details.extend(wrap_with_label("What", &action.description, column_width));
        push_section_lines(&mut right, "Selected Setting", &details);
    }
    push_section_lines(&mut right, "Services", &system_service_lines(snapshot));
    push_section_lines(
        &mut right,
        "Diagnostics",
        &system_diagnostics_lines(snapshot),
    );

    render_two_columns(buf, &left, &right, total_width);
}

fn push_screen_line(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push_str("\r\n");
}

fn push_screen_blank(buf: &mut String) {
    buf.push_str("\r\n");
}

fn render_tabs(active: Tab, cols: usize) -> String {
    let tabs = DEFAULT_TABS
        .iter()
        .map(|tab| render_tab(active == *tab, tab.label()))
        .collect::<Vec<_>>()
        .join("  ");
    pad_ansi_line(&tabs, cols)
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Inbox => "Inbox",
            Tab::People => "People",
            Tab::Apps => "Apps",
            Tab::System => "System",
        }
    }
}

fn render_tab(active: bool, label: &str) -> String {
    if active {
        format!("\x1b[30;46;1m {} \x1b[0m", label)
    } else {
        format!("\x1b[2m{}\x1b[0m", label)
    }
}

fn render_two_columns(buf: &mut String, left: &[String], right: &[String], total_width: usize) {
    let total_width = total_width.max(60);
    if total_width < 90 {
        for line in left {
            push_screen_line(buf, &format!("  {}", fit_line(line, total_width)));
        }
        if !left.is_empty() && !right.is_empty() {
            push_screen_blank(buf);
        }
        for line in right {
            push_screen_line(buf, &format!("  {}", fit_line(line, total_width)));
        }
        return;
    }

    let gutter = 3usize;
    let left_width = (total_width - gutter) / 2;
    let right_width = total_width - gutter - left_width;
    let rows = left.len().max(right.len());

    for idx in 0..rows {
        let left_line = left
            .get(idx)
            .map(|line| fit_line(line, left_width))
            .unwrap_or_else(|| " ".repeat(left_width));
        let right_line = right
            .get(idx)
            .map(|line| fit_line(line, right_width))
            .unwrap_or_else(|| " ".repeat(right_width));
        push_screen_line(
            buf,
            &format!("  {}{}{}", left_line, " ".repeat(gutter), right_line),
        );
    }
}

fn push_section_lines(target: &mut Vec<String>, title: &str, lines: &[String]) {
    target.push(title.to_string());
    target.extend(lines.iter().cloned());
}

fn wrap_with_label(label: &str, text: &str, width: usize) -> Vec<String> {
    let first_width = width.saturating_sub(label.len() + 2).max(12);
    let rest_width = width.max(20);
    let wrapped = wrap_text(text, first_width);
    let mut lines = Vec::new();
    if let Some(first) = wrapped.first() {
        lines.push(format!("{:<10} {}", label, first));
        for line in wrapped.iter().skip(1) {
            lines.push(format!(
                "{:<10} {}",
                "",
                fit_line(line, rest_width.saturating_sub(11))
            ));
        }
    }
    lines
}

fn column_width(total_width: usize) -> usize {
    if total_width < 90 {
        total_width.max(20)
    } else {
        ((total_width - 3) / 2).max(20)
    }
}

fn selected_marker(selected: bool) -> &'static str {
    if selected {
        ">"
    } else {
        " "
    }
}

fn people_overview_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut lines = people_profile_lines(snapshot, width);
    lines.push(format!("Contacts   {}", snapshot.people.contact_count));
    lines.push(format!(
        "Discovery  {}",
        people_discovery_state_label(&snapshot.people.discovery)
    ));
    let peers = people_visible_peers(snapshot);
    lines.push(format!("Visible    {}", peers.len()));
    let requests = people_visible_requests(snapshot);
    lines.push(format!("Requests   {}", requests.len()));
    lines
}

fn people_profile_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut lines = vec![
        format!("Name       {}", display_name(snapshot)),
        format!("User       {}", snapshot.user),
        format!("Identity   {}", identity_summary(snapshot)),
    ];
    if !snapshot.people.schema.trim().is_empty() {
        lines.push(format!("Model      {}", snapshot.people.schema));
    }
    if snapshot.people.service_offer_count > 0 {
        lines.push(format!(
            "Services   {}",
            snapshot.people.service_offer_count
        ));
    }
    if let Some(source) = snapshot.source.as_ref() {
        lines.extend(wrap_with_label("Source", &source.name, width));
    }
    lines
}

fn people_contact_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    if snapshot.people.contacts.is_empty() {
        return vec!["No people yet. Turn on Discovery to find another ElastOS home.".to_string()];
    }
    snapshot
        .people
        .contacts
        .iter()
        .take(8)
        .flat_map(|contact| {
            let mut lines = vec![format!(
                "{} · {}",
                people_contact_display_name(contact, "Person"),
                if contact.relationship.trim().is_empty() {
                    "connected"
                } else {
                    contact.relationship.as_str()
                }
            )];
            let mut details = Vec::new();
            if let Some(handle) = contact
                .handle
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                details.push(handle.to_string());
            }
            if let Some(device) = contact
                .device_label
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                details.push(device.to_string());
            }
            if contact.can_message {
                details.push("message ready".to_string());
            }
            if !details.is_empty() {
                lines.extend(wrap_with_label(" ", &details.join(" · "), width));
            }
            lines
        })
        .collect()
}

fn people_discovery_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let discovery = &snapshot.people.discovery;
    let mut lines = vec![
        format!("State      {}", people_discovery_state_label(discovery)),
        format!(
            "Status     {}",
            if discovery.status_message.trim().is_empty() {
                discovery.status.as_str()
            } else {
                discovery.status_message.as_str()
            }
        ),
    ];
    if discovery.enabled {
        lines.push(format!(
            "Remaining  {}",
            people_discovery_remaining_text(discovery.remaining_seconds.unwrap_or(0))
        ));
    }
    if discovery.discovered_count > 0 {
        lines.push(format!("Visible    {}", discovery.discovered_count));
    }
    if discovery.request_count > 0 {
        lines.push(format!("Requests   {}", discovery.request_count));
    }
    lines
}

fn people_visible_peer_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let peers = people_visible_peers(snapshot);
    if peers.is_empty() {
        return vec![
            "No visible people yet.".to_string(),
            "Use Turn On or Refresh while another ElastOS home is discoverable.".to_string(),
        ];
    }
    peers
        .into_iter()
        .take(8)
        .map(|peer| {
            format!(
                "{} · {}",
                people_peer_display_name(peer, "Visible person"),
                if peer.status.trim().is_empty() {
                    "visible"
                } else {
                    peer.status.as_str()
                }
            )
        })
        .collect()
}

fn people_request_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let requests = people_visible_requests(snapshot);
    if requests.is_empty() {
        return vec!["No People requests waiting.".to_string()];
    }
    requests
        .into_iter()
        .take(8)
        .map(|request| {
            format!(
                "{} · {}",
                people_request_display_name(request, "Person"),
                if request.status.trim().is_empty() {
                    "requested"
                } else {
                    request.status.as_str()
                }
            )
        })
        .collect()
}

fn people_debug_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("You        {}", snapshot.user),
        format!("Nick       {}", display_name(snapshot)),
        format!("Identity   {}", identity_summary(snapshot)),
        format!("Contacts   {}", snapshot.people.contact_count),
        format!(
            "Discovery  {}",
            people_discovery_state_label(&snapshot.people.discovery)
        ),
        format!("Model      {}", snapshot.people.discovery.schema),
        format!("Topic      {}", snapshot.people.discovery.topic),
        format!(
            "LocalPeer  {}",
            snapshot
                .people
                .discovery
                .local_peer_id
                .as_deref()
                .unwrap_or("not advertised")
        ),
        format!("Network    {}", network_summary(snapshot)),
        format!(
            "Profile    {}",
            action_state_label(action_by_id(snapshot, "identity-nickname-set"))
        ),
        format!(
            "Chat       {}",
            action_state_label(action_by_id(snapshot, "chat"))
        ),
        format!(
            "Peers      {}",
            format!(
                "{} endpoints reachable",
                snapshot.runtime.peer_count.unwrap_or_default()
            )
        ),
    ];
    if let Some(ticket) = &snapshot.runtime.ticket {
        lines.push(format!("Ticket     {}", truncate(ticket, 42)));
    } else {
        lines.push("Ticket     waiting for runtime".to_string());
    }
    if let Some(delay) = snapshot.people.discovery.next_refresh_after_ms {
        lines.push(format!("RefreshMs  {delay}"));
    }
    for contact in snapshot.people.contacts.iter().take(3) {
        if let Some(last_seen_at) = contact.last_seen_at {
            lines.push(format!(
                "ContactSeen {} {}",
                people_contact_display_name(contact, "Person"),
                last_seen_at
            ));
        }
    }
    for peer in snapshot.people.discovery.discovered_peers.iter().take(3) {
        if peer.last_seen_at > 0 {
            lines.push(format!(
                "PeerSeen   {} {}",
                people_peer_display_name(peer, "Visible person"),
                peer.last_seen_at
            ));
        }
    }
    for request in snapshot.people.discovery.requests.iter().take(3) {
        if request.created_at > 0 {
            lines.push(format!(
                "ReqCreated {} {}",
                people_request_display_name(request, "Person"),
                request.created_at
            ));
        }
        if let Some(invite_id) = request
            .invite_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            lines.push(format!("ReqInvite  {}", truncate(invite_id, 42)));
        }
    }
    lines.push(format!(
        "RoomGuests {}",
        if snapshot.room.allow_guest_invites {
            "public join requests enabled"
        } else {
            "public join requests disabled"
        }
    ));
    lines.push(format!(
        "RoomUsers  {}",
        if snapshot.room.allow_member_invites {
            "ElastOS user invites enabled"
        } else {
            "ElastOS user invites disabled"
        }
    ));
    lines.push(format!("RoomReqs   {}", snapshot.room.pending_count));
    lines.push(format!("RoomWeb    {}", snapshot.room.active_session_count));
    lines.push("Manage     elastos identity nickname set".to_string());
    lines
}

fn spaces_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("MyWebSite  {}", website_summary(snapshot)),
        format!(
            "Public     {} shared channel{} ready to open",
            snapshot.shares.channel_count,
            if snapshot.shares.channel_count == 1 {
                ""
            } else {
                "s"
            }
        ),
        "Local      scratch space for temporary work and session state".to_string(),
        "WebSpaces  named handles into content, peers, identity, and AI".to_string(),
    ]
}

fn apps_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let entries = app_entries(snapshot)
        .into_iter()
        .filter(|entry| !entry.is_control)
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut last_category = "";
    for entry in entries.into_iter().take(8) {
        if entry.category != last_category {
            lines.push(format!("{}:", entry.category));
            last_category = entry.category;
        }
        lines.push(format!("  {} [{}]", entry.label, entry.state));
    }
    lines
}

fn system_settings_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = compact_system_summary_lines(snapshot);
    lines.push(format!(
        "Shell      active {} · run `system shell home-gui` to return to Home GUI",
        active_shell_label(snapshot)
    ));
    lines.push("Source     system source".to_string());
    lines.push("Updates    system updates".to_string());
    lines.push("Services   system services".to_string());
    lines.push("Identity   system identity".to_string());
    lines.push("Health     system diagnostics".to_string());
    lines
}

fn system_shell_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let active = active_shell_label(snapshot);
    let mut lines = vec![
        format!("Active     {active}"),
        "Home GUI   system shell home-gui".to_string(),
        "Home CLI   current terminal shell".to_string(),
        "Authority  Runtime active-shell state, settled by the Home host".to_string(),
    ];
    if active == "home-gui" {
        lines.push("Status     home-gui is already active".to_string());
    } else {
        lines.push(
            "Enter      select Return to Home GUI, or run `system shell home-gui`".to_string(),
        );
    }
    lines
}

fn system_source_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let Some(source) = snapshot.source.as_ref() else {
        return vec![
            "Trusted source not configured".to_string(),
            "Updates disabled until a trusted Runtime source is configured".to_string(),
        ];
    };
    vec![
        format!("Name       {}", source.name),
        format!(
            "Gateway    {}",
            source.gateway.as_deref().unwrap_or("not configured")
        ),
        format!("Channel    {}", source_channel_label(snapshot)),
        format!("Installed  {}", source_installed_label(snapshot)),
        format!("Status     {}", source_status_label(snapshot)),
    ]
}

fn system_update_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("Policy     {}", source_update_policy_label(snapshot)),
        format!("Source     {}", source_label(snapshot)),
        format!("Channel    {}", source_channel_label(snapshot)),
        format!("Installed  {}", source_installed_label(snapshot)),
        "Apply      no automatic update is run from Home CLI".to_string(),
    ]
}

fn system_service_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let ready = snapshot
        .system_services
        .iter()
        .filter(|service| service.ready)
        .count();
    let mut lines = vec![format!(
        "Runtime    {} / {} core services ready",
        ready,
        snapshot.system_services.len()
    )];
    for service in snapshot.system_services.iter().take(8) {
        lines.push(format!(
            "{:<10} {}",
            if service.ready { "ready" } else { "blocked" },
            service.name
        ));
    }
    let offers = cli_service_offers(snapshot, "");
    lines.push(format!(
        "Offers     {} visible service offers",
        offers.len()
    ));
    for offer in offers.iter().take(5) {
        lines.push(format!("Offer      {}", service_offer_line(offer)));
    }
    lines
}

fn system_identity_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("User       {}", snapshot.user),
        format!("Display    {}", display_name(snapshot)),
        format!("DID        {}", identity_summary(snapshot)),
        format!("Network    {}", network_summary(snapshot)),
        format!("Session    {}", home_cli_session_mode_label(snapshot)),
        format!("Auth       {}", home_cli_auth_state_label(snapshot)),
    ]
}

fn home_cli_session_mode_label(snapshot: &HomeSnapshot) -> String {
    match snapshot.session.mode.trim() {
        "browser_pty" => "browser Runtime PTY".to_string(),
        "native_terminal" => "native terminal".to_string(),
        "" => "home-cli".to_string(),
        other => other.to_string(),
    }
}

fn home_cli_auth_state_label(snapshot: &HomeSnapshot) -> String {
    let state = snapshot
        .session
        .extra
        .get(&session_auth_state_key())
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if state.is_empty() {
        "not reported by this Home snapshot".to_string()
    } else {
        state.to_string()
    }
}

fn session_auth_state_key() -> String {
    ["pass", "key_state"].concat()
}

fn system_diagnostics_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let running_capsules = snapshot.runtime.running_capsules.len();
    let installed_capsules = catalog_capsules(snapshot)
        .len()
        .max(snapshot.cached_capsules.len());
    let service_ready = snapshot
        .system_services
        .iter()
        .filter(|service| service.ready)
        .count();
    vec![
        format!("Runtime    {}", runtime_state_label(snapshot)),
        format!(
            "Kind       {}",
            snapshot.runtime.kind.as_deref().unwrap_or("unknown")
        ),
        format!("Peers      {}", snapshot.runtime.peer_count.unwrap_or(0)),
        format!("Capsules   {running_capsules} running / {installed_capsules} installed"),
        format!(
            "Services   {service_ready} / {} ready",
            snapshot.system_services.len()
        ),
        format!("Roots      {} configured", snapshot.roots.len()),
        format!(
            "Inbox      {} attention",
            snapshot.notifications.attention_count
        ),
    ]
}

fn service_offer_line(offer: &serde_json::Value) -> String {
    let id = first_json_text(offer, &["offer_id", "id", "service_uri"]);
    let kind = first_json_text(offer, &["service_kind", "kind"]);
    let status = first_json_text(offer, &["status"]);
    let name = first_json_text(offer, &["service_display_name", "display_name"]);
    format!(
        "{} [{}] {}",
        if name.is_empty() { id } else { name },
        if kind.is_empty() { "service" } else { kind },
        if status.is_empty() {
            "available"
        } else {
            status
        }
    )
}

fn compact_system_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let ready = snapshot
        .system_services
        .iter()
        .filter(|service| service.ready)
        .count();
    let identity = if snapshot.did.is_some() {
        "ready"
    } else {
        "needs setup"
    };
    vec![
        format!("Runtime    {}", runtime_state_label(snapshot)),
        format!("Identity   {}", identity),
        format!("Source     {}", source_status_label(snapshot)),
        format!("Updates    {}", source_update_policy_label(snapshot)),
        format!(
            "Inbox      {} attention · {} unread",
            snapshot.notifications.attention_count, snapshot.notifications.unread_count
        ),
        format!(
            "Services   {} / {} ready",
            ready,
            snapshot.system_services.len()
        ),
        "Root       ElastOS".to_string(),
    ]
}

fn compact_system_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = compact_system_summary_lines(snapshot);
    let not_ready: Vec<&SystemServiceStatus> = snapshot
        .system_services
        .iter()
        .filter(|service| !service.ready)
        .collect();
    if !not_ready.is_empty() {
        let missing = not_ready
            .iter()
            .take(3)
            .map(|service| service.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Attention  {}", missing));
    }
    lines.push(format!(
        "ElastOS    {}",
        root_example(snapshot, "ElastOS", "localhost://ElastOS/SystemRegistry")
    ));
    if active_shell_label(snapshot) == "home-gui" {
        lines.push("Shell      home-gui is already active".to_string());
    } else {
        lines.push("Switch     system shell home-gui".to_string());
    }
    lines.push("Next       system source, services, identity, diagnostics".to_string());
    lines
}

fn active_shell_label(snapshot: &HomeSnapshot) -> String {
    snapshot
        .active_shell
        .active
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("home-cli")
        .to_string()
}

fn root_example(snapshot: &HomeSnapshot, name: &str, default_example: &str) -> String {
    snapshot
        .roots
        .iter()
        .find(|root| root.name == name)
        .map(|root| root.example.as_str())
        .filter(|example| !example.is_empty())
        .unwrap_or(default_example)
        .to_string()
}

fn root_group_name(root: &str) -> &'static str {
    match root {
        "Users" | "UsersAI" => "People",
        "Local" | "Public" | "MyWebSite" | "WebSpaces" => "Spaces",
        "AppCapsules" => "Apps",
        "ElastOS" => "System",
        _ => "World",
    }
}

fn truncate_did(did: &str) -> String {
    truncate(did, 36)
}

fn network_summary(snapshot: &HomeSnapshot) -> String {
    if !snapshot.runtime.running {
        return "home session not running yet".to_string();
    }

    let peers = snapshot.runtime.peer_count.unwrap_or(0);
    if peers == 0 {
        if snapshot.runtime.ticket.is_some() {
            "Carrier bootstrap ready; waiting for another participant".to_string()
        } else {
            "starting up".to_string()
        }
    } else if peers == 1 {
        "1 Carrier endpoint reachable".to_string()
    } else {
        format!("{} Carrier endpoints reachable", peers)
    }
}

fn identity_summary(snapshot: &HomeSnapshot) -> String {
    snapshot
        .did
        .as_deref()
        .map(truncate_did)
        .unwrap_or_else(|| "not initialized yet".to_string())
}

fn display_name(snapshot: &HomeSnapshot) -> String {
    snapshot
        .nickname
        .as_deref()
        .filter(|nick| !nick.is_empty())
        .unwrap_or(&snapshot.user)
        .to_string()
}

fn website_summary(snapshot: &HomeSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(url) = snapshot.site.local_url.as_deref() {
        parts.push(format!("preview at {}", url.trim_end_matches('/')));
    } else if snapshot.site.staged {
        parts.push("staged at localhost://MyWebSite".to_string());
    } else {
        parts.push("not staged locally".to_string());
    }

    if let Some(release) = snapshot.site.active_release.as_deref() {
        if let Some(channel) = snapshot.site.active_channel.as_deref() {
            parts.push(format!("live {} on {}", release, channel));
        } else {
            parts.push(format!("release {}", release));
        }
    } else if snapshot.site.release_count > 0 {
        let suffix = if snapshot.site.release_count == 1 {
            ""
        } else {
            "s"
        };
        parts.push(format!(
            "{} saved release{}",
            snapshot.site.release_count, suffix
        ));
    }

    if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
        parts.push(format!("elastos://{}", truncate(cid, 18)));
    }

    parts.join(" · ")
}

fn source_label(snapshot: &HomeSnapshot) -> String {
    match &snapshot.source {
        Some(source) => {
            let name = if source.name == "default" {
                "default".to_string()
            } else {
                source.name.clone()
            };
            match &source.gateway {
                Some(gateway) => {
                    let host = gateway
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/');
                    if name == host {
                        host.to_string()
                    } else {
                        format!("{} via {}", name, host)
                    }
                }
                None => name,
            }
        }
        None => "no trusted source configured".to_string(),
    }
}

fn source_status_label(snapshot: &HomeSnapshot) -> String {
    match snapshot.source.as_ref() {
        Some(_) => format!(
            "{} · {} · installed {}",
            source_label(snapshot),
            source_channel_label(snapshot),
            source_installed_label(snapshot)
        ),
        None => "not configured".to_string(),
    }
}

fn source_channel_label(snapshot: &HomeSnapshot) -> String {
    snapshot
        .source
        .as_ref()
        .map(|source| {
            let channel = source.channel.trim();
            if channel.is_empty() {
                "stable"
            } else {
                channel
            }
        })
        .unwrap_or("not configured")
        .to_string()
}

fn source_installed_label(snapshot: &HomeSnapshot) -> String {
    snapshot
        .source
        .as_ref()
        .map(|source| {
            let version = source.installed_version.trim();
            if version.is_empty() {
                "unknown"
            } else {
                version
            }
        })
        .unwrap_or("not configured")
        .to_string()
}

fn source_update_policy_label(snapshot: &HomeSnapshot) -> String {
    let Some(source) = snapshot.source.as_ref() else {
        return "disabled (no trusted source)".to_string();
    };
    if snapshot.version.contains("dev") {
        return "disabled in dev builds; use explicit source/update commands".to_string();
    }
    let channel = if source.channel.trim().is_empty() {
        "stable"
    } else {
        source.channel.trim()
    };
    format!("allowed on {channel} via Carrier-first trusted source")
}

fn section_title(title: &str, cols: usize) -> String {
    fit_line(title, cols)
}

fn home_action_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    prioritized_action_indices(
        snapshot,
        &["chat", "room-approve", "room-deny", "room-revoke-all"],
    )
}

fn people_actions(snapshot: &HomeSnapshot) -> Vec<PeopleAction> {
    let mut actions = Vec::new();
    let discovery = &snapshot.people.discovery;
    if discovery.enabled && discovery.remaining_seconds.unwrap_or(0) > 0 {
        actions.push(PeopleAction {
            id: "people-discovery-disable".to_string(),
            label: "Stop discovery".to_string(),
            description: "Stop advertising this Home as discoverable to nearby ElastOS homes."
                .to_string(),
            command: "people discovery off".to_string(),
            ready: true,
            reason: None,
        });
    } else {
        actions.push(PeopleAction {
            id: "people-discovery-enable".to_string(),
            label: "Turn on discovery".to_string(),
            description: "Make this Home discoverable for a short window so another ElastOS home can request contact."
                .to_string(),
            command: "people discovery on".to_string(),
            ready: true,
            reason: None,
        });
    }
    actions.push(PeopleAction {
        id: "people-discovery-refresh".to_string(),
        label: "Refresh discovery".to_string(),
        description:
            "Refresh visible people and pending People requests through the Runtime People route."
                .to_string(),
        command: "people discovery refresh".to_string(),
        ready: true,
        reason: None,
    });
    for request in people_visible_requests(snapshot)
        .into_iter()
        .filter(|request| request.status == "incoming")
    {
        if request.request_id.trim().is_empty() {
            continue;
        }
        let name = people_request_display_name(request, "Person");
        actions.push(PeopleAction {
            id: format!("people-accept-request:{}", request.request_id),
            label: format!("Accept {name}"),
            description: "Accept this incoming People request and add the person to People."
                .to_string(),
            command: format!("people accept {}", request.request_id),
            ready: true,
            reason: None,
        });
    }
    for peer in people_visible_peers(snapshot) {
        if peer.peer_id.trim().is_empty() {
            continue;
        }
        let name = people_peer_display_name(peer, "Visible person");
        actions.push(PeopleAction {
            id: format!("people-request-peer:{}", peer.peer_id),
            label: format!("Request {name}"),
            description: "Send a People request to this visible ElastOS home.".to_string(),
            command: format!("people request {}", peer.peer_id),
            ready: true,
            reason: None,
        });
    }
    for contact in &snapshot.people.contacts {
        let name = people_contact_display_name(contact, "Person");
        if contact.can_message && !contact.contact_id.trim().is_empty() {
            actions.push(PeopleAction {
                id: format!("people-message:{}", contact.contact_id),
                label: format!("Chat with {name}"),
                description: "Open the Home CLI Chat flow for this contact.".to_string(),
                command: format!("people message {}", contact.contact_id),
                ready: true,
                reason: None,
            });
        }
        if !contact.contact_id.trim().is_empty() {
            actions.push(PeopleAction {
                id: format!("people-remove-contact:{}", contact.contact_id),
                label: format!("Remove {name}"),
                description: "Remove this person from People through the Runtime People route."
                    .to_string(),
                command: format!("people remove {}", contact.contact_id),
                ready: true,
                reason: None,
            });
        }
    }
    actions
}

fn selected_people_action(snapshot: &HomeSnapshot, selected: usize) -> Option<PeopleAction> {
    let actions = people_actions(snapshot);
    actions
        .get(selected.min(actions.len().saturating_sub(1)))
        .cloned()
}

fn system_actions(_snapshot: &HomeSnapshot) -> Vec<SystemAction> {
    vec![SystemAction {
        id: "shell-switch:home-gui".to_string(),
        label: "Return to Home GUI".to_string(),
        description:
            "Switch the active Home shell back to the graphical desktop through Runtime state."
                .to_string(),
        command: "system shell home-gui".to_string(),
        ready: true,
        reason: None,
    }]
}

fn selected_system_action(snapshot: &HomeSnapshot, selected: usize) -> Option<SystemAction> {
    let actions = system_actions(snapshot);
    actions
        .get(selected.min(actions.len().saturating_sub(1)))
        .cloned()
}

fn people_visible_peers(snapshot: &HomeSnapshot) -> Vec<&PeopleDiscoveryPeerStatus> {
    let mut contact_peer_ids = std::collections::BTreeSet::new();
    let mut contact_dids = std::collections::BTreeSet::new();
    for contact in &snapshot.people.contacts {
        if let Some(device) = contact
            .device_label
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            contact_peer_ids.insert(device.to_string());
        }
        if contact.route.starts_with("elastos://peer/") {
            contact_peer_ids.insert(
                contact
                    .route
                    .trim_start_matches("elastos://peer/")
                    .to_string(),
            );
        }
        if let Some(handle) = contact
            .handle
            .as_deref()
            .filter(|value| value.starts_with("did:"))
        {
            contact_dids.insert(handle.to_string());
        }
    }
    snapshot
        .people
        .discovery
        .discovered_peers
        .iter()
        .filter(|peer| {
            let peer_id = peer.peer_id.trim();
            let did = peer.did.as_deref().unwrap_or("").trim();
            (peer_id.is_empty() || !contact_peer_ids.contains(peer_id))
                && (did.is_empty() || !contact_dids.contains(did))
        })
        .collect()
}

fn people_visible_requests(snapshot: &HomeSnapshot) -> Vec<&PeopleDiscoveryRequestStatus> {
    snapshot
        .people
        .discovery
        .requests
        .iter()
        .filter(|request| matches!(request.status.as_str(), "incoming" | "requested"))
        .collect()
}

fn people_discovery_state_label(discovery: &PeopleDiscoveryStatus) -> String {
    if discovery.enabled && discovery.remaining_seconds.unwrap_or(0) > 0 {
        format!(
            "on for {}",
            people_discovery_remaining_text(discovery.remaining_seconds.unwrap_or(0))
        )
    } else if !discovery.visibility.trim().is_empty() && discovery.visibility != "off" {
        discovery.visibility.clone()
    } else if !discovery.status.trim().is_empty() {
        discovery.status.clone()
    } else {
        "off".to_string()
    }
}

fn people_discovery_remaining_text(seconds: u64) -> String {
    if seconds == 0 {
        "0 sec".to_string()
    } else if seconds >= 60 {
        format!("{} min", seconds.div_ceil(60))
    } else {
        format!("{seconds} sec")
    }
}

fn people_contact_display_name(contact: &PeopleContactStatus, fallback: &str) -> String {
    let profile = contact.profile_card.as_ref();
    let display_name = profile
        .map(|profile| profile.display_name.as_str())
        .unwrap_or("")
        .trim();
    if !display_name.is_empty() && display_name != "ElastOS user" {
        return display_name.to_string();
    }
    let direct = contact.display_name.trim();
    if !direct.is_empty() && direct != "ElastOS user" {
        return direct.to_string();
    }
    profile
        .and_then(|profile| profile.handle.as_deref())
        .or(contact.handle.as_deref())
        .or(contact.device_label.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn people_peer_display_name(peer: &PeopleDiscoveryPeerStatus, fallback: &str) -> String {
    let display_name = peer.display_name.trim();
    if !display_name.is_empty() && display_name != "ElastOS user" {
        return display_name.to_string();
    }
    peer.handle
        .as_deref()
        .or(peer.did.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if peer.peer_id.trim().is_empty() {
                fallback
            } else {
                peer.peer_id.as_str()
            }
        })
        .to_string()
}

fn people_request_display_name(request: &PeopleDiscoveryRequestStatus, fallback: &str) -> String {
    let display_name = request.display_name.trim();
    if !display_name.is_empty() && display_name != "ElastOS user" {
        return display_name.to_string();
    }
    request
        .handle
        .as_deref()
        .or(request.did.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if request.peer_id.trim().is_empty() {
                fallback
            } else {
                request.peer_id.as_str()
            }
        })
        .to_string()
}

fn notification_entries(snapshot: &HomeSnapshot) -> &[NotificationEntryStatus] {
    &snapshot.notifications.entries
}

fn notification_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    (0..notification_entries(snapshot).len()).collect()
}

fn quick_launch_action_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    home_action_indices(snapshot)
}

fn prioritized_action_indices(snapshot: &HomeSnapshot, ids: &[&str]) -> Vec<usize> {
    let mut indices = Vec::new();
    for id in ids {
        if let Some(idx) = snapshot.actions.iter().position(|action| action.id == *id) {
            if !indices.contains(&idx) {
                indices.push(idx);
            }
        }
    }
    indices
}

fn selected_action<'a>(
    snapshot: &'a HomeSnapshot,
    indices: &[usize],
    selected: usize,
) -> Option<&'a ActionInfo> {
    let idx = indices.get(selected.min(indices.len().saturating_sub(1)))?;
    snapshot.actions.get(*idx)
}

fn selected_notification<'a>(
    snapshot: &'a HomeSnapshot,
    selected: usize,
) -> Option<&'a NotificationEntryStatus> {
    let entries = notification_entries(snapshot);
    entries.get(selected.min(entries.len().saturating_sub(1)))
}

fn selected_notification_read_action(snapshot: &HomeSnapshot, selected: usize) -> Option<String> {
    let entry = selected_notification(snapshot, selected)?;
    Some(format!("notification-read:{}", entry.id))
}

fn selected_notification_dismiss_action(
    snapshot: &HomeSnapshot,
    selected: usize,
) -> Option<String> {
    let entry = selected_notification(snapshot, selected)?;
    Some(format!("notification-dismiss:{}", entry.id))
}

fn selected_notification_action<'a>(
    snapshot: &'a HomeSnapshot,
    selected: usize,
) -> Option<&'a ActionInfo> {
    let entry = selected_notification(snapshot, selected)?;
    let action_id = entry
        .action_ref
        .as_ref()
        .map(|action_ref| action_ref.action_id.as_str())?;
    action_by_id(snapshot, action_id)
}

fn selected_app_action<'a>(snapshot: &'a HomeSnapshot, selected: usize) -> Option<&'a ActionInfo> {
    let entries = app_entries(snapshot);
    let entry = entries.get(selected.min(entries.len().saturating_sub(1)))?;
    action_by_id(snapshot, &entry.action_id)
}

fn action_by_id<'a>(snapshot: &'a HomeSnapshot, id: &str) -> Option<&'a ActionInfo> {
    snapshot.actions.iter().find(|action| action.id == id)
}

fn first_action_by_id<'a>(
    snapshot: &'a HomeSnapshot,
    action_ids: &[&str],
) -> Option<&'a ActionInfo> {
    action_ids
        .iter()
        .find_map(|action_id| action_by_id(snapshot, action_id))
}

fn action_state_label(action: Option<&ActionInfo>) -> String {
    match action {
        Some(action) if action.ready => "ready".to_string(),
        Some(action) => format!(
            "blocked ({})",
            action.reason.as_deref().unwrap_or("setup needed")
        ),
        None => "not available".to_string(),
    }
}

fn next_step_command(reason: &str) -> Option<&str> {
    reason
        .split_once("run: ")
        .map(|(_, command)| command.trim())
}

fn render_home_actions(
    snapshot: &HomeSnapshot,
    indices: &[usize],
    selected: usize,
    width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (slot, action_idx) in indices.iter().take(5).enumerate() {
        let action = &snapshot.actions[*action_idx];
        let state = home_action_state(action, snapshot);
        let summary = home_action_summary(action);
        let label = action_display_label(action);
        lines.push(format!(
            "{} {} {} [{}]  {}",
            selected_marker(slot == selected),
            slot + 1,
            label,
            state,
            truncate(summary, width.saturating_sub(label.len() + 18).max(16))
        ));
        if let Some(reason) = &action.reason {
            lines.push(format!(
                "    setup: {}",
                truncate(reason, width.saturating_sub(11).max(16))
            ));
        }
    }
    lines
}

fn home_action_state<'a>(action: &'a ActionInfo, snapshot: &HomeSnapshot) -> &'a str {
    match action.id.as_str() {
        "site-local" => {
            if snapshot.site.local_url.is_some() {
                "preview"
            } else if snapshot.site.staged && !action.ready {
                "staged"
            } else if !snapshot.site.staged {
                "empty"
            } else if action.ready {
                "ready"
            } else {
                "setup"
            }
        }
        _ => {
            if action.ready {
                "ready"
            } else {
                "setup"
            }
        }
    }
}

fn home_action_summary(action: &ActionInfo) -> &str {
    match action.id.as_str() {
        "chat" => "Send a message and return home",
        "room-approve" => "Approve the next pending Chat web guest request",
        "room-deny" => "Deny the next pending Chat web guest request",
        "room-revoke-all" => "Disconnect active Chat web guest sessions",
        "site-local" => "Start or reuse the local MyWebSite preview",
        "site-ephemeral" => "Publish a temporary public HTTPS URL for MyWebSite",
        "site-open" => "Open the MyWebSite preview in a browser",
        "shares-list" => "Review shared channels, open links, and next steps",
        _ => action.description.as_str(),
    }
}

fn action_display_label<'a>(action: &'a ActionInfo) -> &'a str {
    match action.id.as_str() {
        "chat" => "Chat",
        "room-approve" => "Approve access",
        "room-deny" => "Deny access",
        "room-revoke-all" => "Disconnect browsers",
        "site-local" => "Preview",
        "site-ephemeral" => "Publish",
        "site-open" => "Open",
        "shares-list" => "Shared",
        _ => action.label.as_str(),
    }
}

fn current_notice<'a>(state: &'a TuiState, snapshot: &'a HomeSnapshot) -> Option<&'a str> {
    state.notice.as_deref().or(snapshot.notice.as_deref())
}

fn alerts_lines(snapshot: &HomeSnapshot, width: usize, notice: Option<&str>) -> Vec<String> {
    let mut alerts = Vec::new();
    if snapshot.did.is_none() {
        alerts.push(
            "Identity is not initialized yet. Run elastos setup to create the local DID."
                .to_string(),
        );
    }
    if !snapshot.site.staged {
        alerts.push(
            "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`."
                .to_string(),
        );
    }
    for entry in snapshot.notifications.entries.iter().take(3) {
        alerts.push(entry.body.clone());
    }
    if snapshot.notifications.entries.len() > 3 {
        alerts.push(format!(
            "{} more inbox notification(s) waiting.",
            snapshot.notifications.entries.len() - 3
        ));
    }
    if snapshot.room.active_session_count > 0 {
        alerts.push(format!(
            "Chat has {} active web guest session(s): {}.",
            snapshot.room.active_session_count,
            format_room_participants(&snapshot.room.active_participants)
        ));
    }
    if snapshot.source.is_none() {
        alerts.push(
            "No trusted release source is configured yet, so update flows stay manual.".to_string(),
        );
    }
    let notice = notice.unwrap_or("").trim().to_string();
    alerts
        .into_iter()
        .filter(|item| !notice_covers_alert(&notice, item))
        .flat_map(|item| wrap_text(&item, width))
        .collect()
}

fn notice_covers_alert(notice: &str, alert: &str) -> bool {
    if notice.is_empty() {
        return false;
    }

    let notice = notice.trim();
    let alert = alert.trim();

    notice == alert
        || notice.starts_with(alert)
        || alert.starts_with(notice)
        || (notice.contains("MyWebSite is empty.") && alert.contains("MyWebSite is empty."))
}

fn format_room_participants(participants: &[RoomParticipantStatus]) -> String {
    if participants.is_empty() {
        return "browser room active".to_string();
    }
    participants
        .iter()
        .take(3)
        .map(|participant| {
            format!(
                "{} on {}",
                participant.display_name, participant.device_label
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn runtime_state_label(snapshot: &HomeSnapshot) -> String {
    if !snapshot.runtime.running {
        return "offline".to_string();
    }
    snapshot
        .runtime
        .kind
        .clone()
        .unwrap_or_else(|| "running".to_string())
}

fn should_render_notice(notice: &str) -> bool {
    let trimmed = notice.trim();
    !trimmed.is_empty()
        && trimmed != "Home is live. Launch an app and you return here automatically when it exits."
        && trimmed != "Snapshot refreshed from live local state."
        && !trimmed.starts_with("Returned home from ")
}

fn space_detail_lines(root: &RootStatus, snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut details = vec![
        format!("Group      {}", root_group_name(&root.name)),
        format!("Kind       {}", root.kind),
        format!("URI        {}", root.uri),
        format!("Exists     {}", if root.exists { "yes" } else { "no" }),
    ];
    if let Some(path) = &root.path {
        details.push(format!("Path       {}", path));
    }
    details.extend(wrap_with_label("Meaning", &root.description, width));
    details.extend(wrap_with_label("Example", &root.example, width));
    match root.name.as_str() {
        "MyWebSite" => {
            details.push(format!("State      {}", website_summary(snapshot)));
            if let Some(url) = snapshot.site.local_url.as_deref() {
                details.push(format!("Preview    {}", url.trim_end_matches('/')));
            } else if let Some(action) = action_by_id(snapshot, "site-local") {
                if action.ready {
                    details.push("Preview    mywebsite preview".to_string());
                } else if let Some(reason) = action.reason.as_deref() {
                    if let Some(command) = next_step_command(reason) {
                        details.push(format!("Next       {}", command));
                    } else {
                        details.extend(wrap_with_label("Setup", reason, width));
                    }
                }
            } else if snapshot.site.staged {
                details.push("Next       elastos site stage <dir>".to_string());
            } else {
                details.push("Next       elastos site stage <dir>".to_string());
            }
            if let Some(release) = snapshot.site.active_release.as_deref() {
                let live = snapshot
                    .site
                    .active_channel
                    .as_deref()
                    .map(|channel| format!("{} on {}", release, channel))
                    .unwrap_or_else(|| release.to_string());
                details.push(format!("Live       {}", live));
            } else if snapshot.site.release_count > 0 {
                details.push(format!("Releases   {}", snapshot.site.release_count));
            }
            if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
                details.push(format!("Bundle     elastos://{}", cid));
            }
            details.push("Public     mywebsite publish gives a temporary HTTPS URL".to_string());
            details.extend(wrap_with_label(
                "Commands",
                "mywebsite stage <dir> · mywebsite preview · mywebsite publish · mywebsite open · elastos site publish --release <name> · elastos site activate --channel live · elastos site rollback --target publisher",
                width,
            ));
        }
        "Public" => {
            details.push(format!(
                "Channels   {} total · {} active",
                snapshot.shares.channel_count, snapshot.shares.active_count
            ));
            if let Some(author_did) = snapshot.shares.author_did.as_deref() {
                details.push(format!(
                    "Signer     {}",
                    truncate(author_did, width.saturating_sub(13).max(16))
                ));
            }
            if let Some(channel) = snapshot.shares.channels.first() {
                details.push(format!(
                    "Latest     {} v{} {}",
                    channel.name, channel.latest_version, channel.status
                ));
                details.push(format!(
                    "Open       elastos://{}",
                    truncate(&channel.latest_cid, width.saturating_sub(16).max(16))
                ));
                if let Some(head_cid) = channel.head_cid.as_deref() {
                    details.push(format!(
                        "Head       elastos://{}",
                        truncate(head_cid, width.saturating_sub(16).max(16))
                    ));
                }
            } else {
                details.push("Latest     none yet".to_string());
            }
            details.extend(wrap_with_label(
                "Commands",
                "elastos share <path> · elastos shares list · elastos attest <cid> · elastos open elastos://<cid>",
                width,
            ));
        }
        "Local" => {
            details.extend(wrap_with_label(
                "Commands",
                "Use Local for temporary working state, session roots, and transient data.",
                width,
            ));
        }
        "WebSpaces" => {
            details.extend(wrap_with_label(
                "Commands",
                "elastos webspace ... resolves named monikers into dynamic typed handles.",
                width,
            ));
        }
        _ => {}
    }
    details
}

fn app_entries(snapshot: &HomeSnapshot) -> Vec<AppEntry> {
    let mut entries = Vec::new();

    for spec in APP_SURFACES {
        let Some(action) = first_action_by_id(snapshot, spec.action_ids) else {
            continue;
        };
        let active = app_surface_active(snapshot, spec, action);
        if action.id == "shares-list" && !active && snapshot.shares.channel_count == 0 {
            continue;
        }
        if !(action.ready || active) {
            continue;
        }
        let state = if active { "active" } else { "ready" }.to_string();
        entries.push(AppEntry {
            name: spec.name.to_string(),
            action_id: action.id.clone(),
            label: spec.label.to_string(),
            category: spec.category,
            description: spec.description.to_string(),
            command: if action.command.is_empty() {
                spec.command.to_string()
            } else {
                action.command.clone()
            },
            state,
            is_control: false,
        });
    }
    entries
}

fn chat_room_app_entry(snapshot: &HomeSnapshot) -> Option<AppEntry> {
    if snapshot.room.room_slug.is_empty()
        && snapshot.room.pending_count == 0
        && snapshot.room.active_session_count == 0
        && snapshot.room.member_count == 0
        && snapshot.room.local_runtime_role.is_none()
    {
        return None;
    }

    let state = if snapshot.room.pending_count > 0 {
        "attention"
    } else if snapshot.room.active_session_count > 0 {
        "active"
    } else if !snapshot.room.browser_access_allowed {
        "restricted"
    } else if snapshot.room.member_count > 0 || snapshot.room.local_runtime_role.is_some() {
        "ready"
    } else {
        "idle"
    };

    Some(AppEntry {
        name: "chat-room".to_string(),
        action_id: "chat-room".to_string(),
        label: "Shared Conversation".to_string(),
        category: "Communication",
        description:
            "Chat with other ElastOS users and approved web guests, with attachments opening as ElastOS documents."
                .to_string(),
        command: "Conversation access stays local to this Home.".to_string(),
        state: state.to_string(),
        is_control: false,
    })
}

fn room_control_entries(snapshot: &HomeSnapshot) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    for action in &snapshot.actions {
        let is_room_control = action.id.starts_with("room-approve-request:")
            || action.id.starts_with("room-deny-request:")
            || action.id.starts_with("room-revoke-session:")
            || action.id.starts_with("room-accept-invite:")
            || action.id.starts_with("room-revoke-invite:")
            || action.id.starts_with("room-remove-member:")
            || matches!(
                action.id.as_str(),
                "room-policy-toggle-guests"
                    | "room-policy-toggle-members"
                    | "room-policy-toggle-member-hosts"
            );
        if !is_room_control {
            continue;
        }
        entries.push(AppEntry {
            name: "chat-room".to_string(),
            action_id: action.id.clone(),
            label: action.label.clone(),
            category: "Communication",
            description: action.description.clone(),
            command: action.command.clone(),
            state: if action.ready {
                "ready".to_string()
            } else {
                "blocked".to_string()
            },
            is_control: true,
        });
    }
    entries
}

fn chat_room_app_detail_lines(
    snapshot: &HomeSnapshot,
    entry: &AppEntry,
    width: usize,
) -> Vec<String> {
    let mut details = vec![
        format!("Surface    {}", entry.name),
        format!("State      {}", entry.state),
        format!("Category   {}", entry.category),
    ];
    details.extend(wrap_with_label("What it does", &entry.description, width));

    if !snapshot.room.title.is_empty() {
        details.push(format!("Title      {}", snapshot.room.title));
    }
    if !snapshot.room.room_slug.is_empty() {
        details.push(format!("Channel    {}", snapshot.room.room_slug));
    }
    if let Some(role) = snapshot.room.local_runtime_role.as_deref() {
        details.push(format!("Access     {}", conversation_role_label(role)));
    } else {
        details.push("Access     this device is not connected to this conversation".to_string());
    }
    details.push(format!(
        "People     {} trusted · {} admins · {} active",
        snapshot.room.member_count, snapshot.room.admin_count, snapshot.room.active_member_count
    ));
    details.push(format!("Key epoch  {}", snapshot.room.current_key_epoch));
    details.push(format!(
        "Web guests {}",
        if snapshot.room.allow_guest_invites {
            "public join requests enabled"
        } else {
            "public join requests disabled"
        }
    ));
    details.push(format!(
        "ElastOS    {}",
        if snapshot.room.allow_member_invites {
            "user invites enabled"
        } else {
            "user invites disabled"
        }
    ));
    details.push(format!(
        "Approvals  {}",
        if snapshot.room.allow_members_to_host_guests {
            "trusted users may approve web guests"
        } else {
            "conversation managers approve web guests"
        }
    ));
    if let Some(url) = snapshot.room.canonical_hosted_guest_url.as_deref() {
        details.push(format!(
            "Public URL {}",
            truncate(url, width.saturating_sub(12).max(28))
        ));
    }
    if let Some(url) = snapshot.room.ephemeral_hosted_guest_url.as_deref() {
        details.push(format!(
            "Quick URL  {}",
            truncate(url, width.saturating_sub(12).max(28))
        ));
    }
    if snapshot.room.pending_invite_count > 0 {
        details.push(format!(
            "Invites    {} pending",
            snapshot.room.pending_invite_count
        ));
        for invite in snapshot.room.pending_invites.iter().take(3) {
            details.push(format!(
                "Invite     {} pending",
                truncate(&invite.invited_did, width.saturating_sub(18).max(16)),
            ));
        }
    } else {
        details.push("Invites    no ElastOS user invites pending".to_string());
    }
    if snapshot.room.owner_did.is_none() {
        details.push("Advanced   elastos room seed --title \"Chat\"".to_string());
    } else if matches!(
        snapshot.room.local_runtime_role.as_deref(),
        Some("owner") | Some("admin")
    ) {
        details.push("Advanced   elastos room invite <did:key:...>".to_string());
    }

    if snapshot.room.members.is_empty() {
        details.push("People     no trusted ElastOS users yet".to_string());
    } else {
        for member in snapshot.room.members.iter().take(4) {
            details.push(format!(
                "Person     {} ({})",
                truncate(&member.member_did, width.saturating_sub(18).max(16)),
                conversation_role_label(&member.role)
            ));
        }
    }

    if !snapshot.room.browser_access_allowed {
        if let Some(reason) = snapshot.room.browser_access_block_reason.as_deref() {
            details.extend(wrap_with_label("Web link", reason, width));
        } else {
            details.push("Web link   access blocked on this device".to_string());
        }
    } else {
        details.push("Web link   access allowed from this device".to_string());
    }

    if snapshot.room.pending_requests.is_empty() {
        details.push("Pending    no web guest join requests".to_string());
    } else {
        details.push(format!(
            "Pending    {} web guest request(s)",
            snapshot.room.pending_requests.len()
        ));
        for request in snapshot.room.pending_requests.iter().take(3) {
            details.push(format!(
                "Request    {} on {}",
                request.display_name, request.device_label
            ));
        }
    }

    if snapshot.room.active_sessions.is_empty() {
        details.push("Web guests no active web guest sessions".to_string());
    } else {
        details.push(format!(
            "Web guests {} active session(s)",
            snapshot.room.active_sessions.len()
        ));
        for session in snapshot.room.active_sessions.iter().take(3) {
            details.push(format!(
                "Web guest  {} on {}",
                session.display_name, session.device_label
            ));
        }
    }

    let available_controls = room_control_entries(snapshot);
    if available_controls.is_empty() {
        details.push("Control    No conversation actions are waiting right now.".to_string());
    } else {
        details.push(format!(
            "Control    {} targeted conversation action(s) are available below this entry in Apps.",
            available_controls.len()
        ));
        for control in available_controls.iter().take(3) {
            details.push(format!("Next       {}", control.label));
        }
    }
    details
}

fn app_surface_active(snapshot: &HomeSnapshot, spec: &AppSurfaceSpec, action: &ActionInfo) -> bool {
    if action.id == "site-local" {
        return snapshot.site.local_url.is_some();
    }
    let runtime_name = action.id.strip_prefix("capsule-").unwrap_or(spec.name);
    snapshot.runtime.running_capsules.iter().any(|item| {
        item == runtime_name
            || item.starts_with(&format!("{} ", runtime_name))
            || item.starts_with(&format!("{}(", runtime_name))
    })
}

fn render_app_list(entries: &[AppEntry], selected: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut last_category = "";
    for (idx, entry) in entries.iter().enumerate() {
        if entry.category != last_category {
            lines.push(format!("{}:", entry.category));
            last_category = entry.category;
        }
        lines.push(format!(
            "{} {} [{}]",
            selected_marker(idx == selected),
            if entry.is_control {
                format!("  {}", entry.label)
            } else {
                entry.label.clone()
            },
            entry.state
        ));
    }
    lines
}

fn fit_line(text: &str, cols: usize) -> String {
    let max = cols.max(20);
    let trimmed = truncate(text, max);
    format!("{:<width$}", trimmed, width = max)
}

fn pad_ansi_line(text: &str, cols: usize) -> String {
    let max = cols.max(20);
    let visible = visible_text_width(text);
    if visible >= max {
        return text.to_string();
    }
    format!("{}{}", text, " ".repeat(max - visible))
}

fn rule(cols: usize) -> String {
    "─".repeat(cols.max(20))
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let proposed_len = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };

        if proposed_len > width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn term_cols() -> usize {
    std::env::var("ELASTOS_TERM_COLS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 40)
        .unwrap_or(100)
}

fn term_rows() -> usize {
    std::env::var("ELASTOS_TERM_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 20)
        .unwrap_or(32)
}

fn home_debug_keys() -> bool {
    std::env::var("ELASTOS_HOME_DEBUG_KEYS")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

fn stdin_has_input(timeout_ms: i32) -> Result<bool> {
    let mut pollfd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }

        return Ok(ready != 0 && (pollfd.revents & libc::POLLIN) != 0);
    }
}

fn read_stdin_byte() -> Result<u8> {
    let mut byte = [0u8; 1];

    loop {
        let read = unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) };
        if read == 1 {
            return Ok(byte[0]);
        }
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed").into());
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err.into());
    }
}

fn wait_for_enter() -> Result<()> {
    print!("Press Enter to continue...");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(())
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let keep = max.saturating_sub(1) / 2;
    let suffix = max.saturating_sub(keep + 1);
    let start: String = value.chars().take(keep).collect();
    let end: String = value
        .chars()
        .rev()
        .take(suffix)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}…{}", start, end)
}

fn conversation_role_label(role: &str) -> String {
    match role {
        "owner" | "admin" => "conversation manager".to_string(),
        "member" => "trusted participant".to_string(),
        _ => role.to_string(),
    }
}

fn visible_text_width(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut idx = 0;
    let mut width = 0;

    while idx < bytes.len() {
        if bytes[idx] == 0x1b {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'[' {
                idx += 1;
                while idx < bytes.len() && bytes[idx] != b'm' {
                    idx += 1;
                }
                if idx < bytes.len() {
                    idx += 1;
                }
                continue;
            }
        }

        let ch = text[idx..].chars().next().unwrap();
        width += 1;
        idx += ch.len_utf8();
    }

    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_read_bytes_accepts_direct_provider_body() {
        let body = serde_json::json!({
            "status": "ok",
            "data": {
                "content": [104, 105],
                "size": 2
            }
        });

        assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
    }

    #[test]
    fn storage_read_bytes_accepts_utf8_provider_body() {
        let body = serde_json::json!({
            "status": "ok",
            "data": {
                "content": "{\"ok\":true}",
                "encoding": "utf8",
                "size": 11
            }
        });

        assert_eq!(
            storage_read_bytes_from_result(&body).unwrap(),
            br#"{"ok":true}"#
        );
    }

    #[test]
    fn storage_read_bytes_accepts_runtime_carrier_result_body() {
        let body = serde_json::json!({
            "type": "carrier_result",
            "result": {
                "status": "ok",
                "data": {
                    "content": [104, 105],
                    "size": 2
                }
            }
        });

        assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
    }

    #[test]
    fn storage_read_bytes_accepts_wrapped_runtime_response() {
        let body = serde_json::json!({
            "response": {
                "type": "carrier_result",
                "result": {
                    "status": "ok",
                    "data": "hi"
                }
            }
        });

        assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
    }

    #[test]
    fn storage_read_bytes_reports_provider_error() {
        let body = serde_json::json!({
            "type": "carrier_result",
            "result": {
                "status": "error",
                "code": "read_failed",
                "message": "no such object"
            }
        });

        let error = storage_read_bytes_from_result(&body)
            .unwrap_err()
            .to_string();
        assert!(error.contains("read_failed"));
        assert!(error.contains("no such object"));
    }

    #[test]
    fn command_contract_is_home_cli_and_terminal_scoped() {
        let contract = command_contract();
        let home_cli_commands: Vec<String> = contract_commands_for("home-cli")
            .into_iter()
            .map(|command| command.name)
            .collect();
        let drift_surfaces: Vec<String> = contract
            .commands
            .iter()
            .flat_map(|command| command.surface.iter())
            .filter(|surface| surface.as_str() == "native" || surface.as_str() == "browser")
            .cloned()
            .collect();

        assert!(
            drift_surfaces.is_empty(),
            "Home CLI command contract must not split entrypoint-specific command vocabularies"
        );
        assert_eq!(
            home_cli_commands,
            vec![
                "home".to_string(),
                "apps".to_string(),
                "invoke".to_string(),
                "inbox".to_string(),
                "people".to_string(),
                "mywebsite".to_string(),
                "wallet".to_string(),
                "exits".to_string(),
                "system".to_string(),
                "debug".to_string(),
                "refresh".to_string(),
                "help".to_string(),
                "exit".to_string(),
            ]
        );
        assert_eq!(normalize_contract_command("whoami"), "home");
        assert_eq!(normalize_contract_command("approvals"), "inbox");
        assert_eq!(normalize_contract_command("approve"), "inbox");
        assert_eq!(normalize_contract_command("contacts"), "people");
        assert_eq!(normalize_contract_command("spaces"), "mywebsite");
        assert_eq!(normalize_contract_command("exit-nodes"), "exits");
        assert_eq!(normalize_contract_command("settings"), "system");
        assert_eq!(normalize_contract_command("dev"), "debug");
        assert_eq!(normalize_debug_command("webspaces"), "spaces");
        assert_eq!(normalize_debug_command("shortcuts"), "terminal");
        assert_eq!(
            contract.terminal.pty.as_deref(),
            Some("Runtime-owned PTY; xterm sends input bytes and renders PTY output without direct host process authority"),
            "Home CLI must describe the Runtime-owned PTY boundary honestly",
        );
        assert_eq!(
            contract.terminal.entrypoint.as_deref(),
            Some("the same home-cli binary runs inside the Runtime PTY and through `elastos home` over local Home state"),
            "`elastos home` and browser home-cli must share the same home-cli capsule binary",
        );
        assert!(
            contract
                .controls
                .iter()
                .any(|control| control.key == "q / Esc"
                    && control.description.contains("home-gui")),
            "shell-switch copy must name the sibling shell as home-gui",
        );
        assert!(
            !COMMAND_CONTRACT_JSON.contains("Home GUI"),
            "command contract should not use a third prose name for home-gui",
        );
    }

    #[test]
    fn home_cli_line_mode_accepts_shared_snapshot_backed_commands() {
        let snapshot = sample_snapshot();
        for command in [
            "home",
            "apps",
            "inbox",
            "people",
            "mywebsite",
            "mywebsite status",
            "site",
            "website",
            "spaces",
            "wallet",
            "exits",
            "debug",
            "debug capsules",
            "debug inspect browser",
            "debug affordances browser",
            "debug gates",
            "debug gates browser",
            "debug audit browser",
            "debug people",
            "debug spaces",
            "debug spaces webspaces",
            "debug webspaces",
            "debug services",
            "debug browser",
            "debug terminal",
            "debug contract",
            "system",
            "settings",
            "system shell",
            "system source",
            "system updates",
            "system services",
            "system identity",
            "system auth",
            "system diagnostics",
            "approvals",
            "approve",
        ] {
            assert!(
                handle_shared_line_command(command, &snapshot).unwrap(),
                "Home CLI line mode did not accept shared command: {command}",
            );
        }
        for command in [
            "capsules",
            "inspect browser",
            "affordances browser",
            "gates browser",
            "audit browser",
            "services",
            "browser",
            "terminal",
            "contract",
        ] {
            assert!(
                !handle_shared_line_command(command, &snapshot).unwrap(),
                "developer command should require explicit debug prefix: {command}",
            );
        }
        assert_eq!(normalize_contract_command("call"), "invoke");
    }

    #[test]
    fn system_line_mode_emits_home_gui_shell_switch() {
        let mut snapshot = sample_snapshot();
        snapshot.active_shell.active = Some("home-cli".to_string());

        assert_eq!(
            system_line_action("system shell home-gui", &snapshot).unwrap(),
            Some("shell-switch:home-gui".to_string())
        );
        assert_eq!(
            system_line_action("settings shell gui", &snapshot).unwrap(),
            Some("shell-switch:home-gui".to_string())
        );
        assert!(system_line_action("system shell browser", &snapshot).is_err());

        snapshot.active_shell.active = Some("home-gui".to_string());
        assert_eq!(
            system_line_action("system shell home-gui", &snapshot).unwrap(),
            Some("shell-switch:home-gui".to_string())
        );
    }

    #[test]
    fn system_cli_exposes_settings_not_passive_status_only() {
        let snapshot = sample_snapshot();
        let lines = system_settings_lines(&snapshot).join("\n");
        assert!(lines.contains("system shell home-gui"));
        assert!(lines.contains("system source"));
        assert!(lines.contains("system updates"));
        assert!(lines.contains("system services"));
        assert!(lines.contains("system identity"));
        assert!(lines.contains("system diagnostics"));
        assert!(system_identity_lines(&snapshot)
            .join("\n")
            .contains("launch-token authorized browser Home session"));

        let mut state = TuiState::default();
        state.tab = Tab::System;
        let screen = build_tui_screen(&snapshot, &state, 100, 30);
        assert!(screen.contains("System"));
        assert!(screen.contains("Return to Home GUI"));
        assert!(screen.contains("system shell home-gui"));
    }

    #[test]
    fn people_line_mode_emits_snapshot_backed_people_actions() {
        let mut snapshot = sample_snapshot();
        snapshot.people = PeopleStatus {
            contact_count: 1,
            contacts: vec![PeopleContactStatus {
                contact_id: "contact-alice".to_string(),
                display_name: "Alice".to_string(),
                relationship: "connected".to_string(),
                can_message: true,
                ..PeopleContactStatus::default()
            }],
            discovery: PeopleDiscoveryStatus {
                enabled: true,
                remaining_seconds: Some(60),
                discovered_peers: vec![PeopleDiscoveryPeerStatus {
                    peer_id: "peer-bob".to_string(),
                    display_name: "Bob".to_string(),
                    status: "visible".to_string(),
                    ..PeopleDiscoveryPeerStatus::default()
                }],
                requests: vec![PeopleDiscoveryRequestStatus {
                    request_id: "request-carol".to_string(),
                    peer_id: "peer-carol".to_string(),
                    display_name: "Carol".to_string(),
                    status: "incoming".to_string(),
                    ..PeopleDiscoveryRequestStatus::default()
                }],
                ..PeopleDiscoveryStatus::default()
            },
            ..PeopleStatus::default()
        };

        assert_eq!(
            people_line_action("people discovery off", &snapshot).unwrap(),
            Some("people-discovery-disable".to_string())
        );
        assert_eq!(
            people_line_action("discovery refresh", &snapshot).unwrap(),
            Some("people-discovery-refresh".to_string())
        );
        assert_eq!(
            people_line_action("people request peer-bob", &snapshot).unwrap(),
            Some("people-request-peer:peer-bob".to_string())
        );
        assert_eq!(
            people_line_action("people accept request-carol", &snapshot).unwrap(),
            Some("people-accept-request:request-carol".to_string())
        );
        assert_eq!(
            people_line_action("people message contact-alice", &snapshot).unwrap(),
            Some("people-message:contact-alice".to_string())
        );
        assert_eq!(
            people_line_action("people remove contact-alice", &snapshot).unwrap(),
            Some("people-remove-contact:contact-alice".to_string())
        );
        assert!(people_line_action("people request missing", &snapshot)
            .unwrap_err()
            .to_string()
            .contains("not available"));
        assert_eq!(people_line_action("people", &snapshot).unwrap(), None);
    }

    #[test]
    fn mywebsite_line_mode_emits_explicit_site_actions() {
        assert_eq!(mywebsite_line_action("mywebsite").unwrap(), None);
        assert_eq!(mywebsite_line_action("spaces status").unwrap(), None);
        assert_eq!(
            mywebsite_line_action("mywebsite stage /tmp/my site").unwrap(),
            Some("site-stage:/tmp/my site".to_string())
        );
        assert_eq!(
            mywebsite_line_action("site preview").unwrap(),
            Some("site-local".to_string())
        );
        assert_eq!(
            mywebsite_line_action("website publish").unwrap(),
            Some("site-ephemeral".to_string())
        );
        assert_eq!(
            mywebsite_line_action("spaces open").unwrap(),
            Some("site-open".to_string())
        );
        assert!(mywebsite_line_action("mywebsite stage")
            .unwrap_err()
            .to_string()
            .contains("stage <dir>"));
    }

    #[test]
    fn home_cli_pages_keep_context_header() {
        let snapshot = sample_snapshot();
        let header = cli_page_header(&snapshot, "People");

        assert!(header.starts_with("\x1B[2J\x1B[HHome CLI / People\n"));
        assert!(header.contains("user anders"));
        assert!(header.contains("identity did:key:z6M"));
        assert!(header.contains("shell home-cli"));
        assert!(!header.contains("ElastOS Home"));
    }

    #[test]
    fn debug_spaces_aliases_resolve_to_selected_roots() {
        assert_eq!(space_query_for_command("webspaces", ""), "WebSpaces");
        assert_eq!(space_query_for_command("mywebsite", ""), "MyWebSite");
        assert_eq!(space_query_for_command("spaces", "public"), "public");

        let snapshot = sample_snapshot();
        let webspaces = snapshot
            .roots
            .iter()
            .find(|root| root.name == "WebSpaces")
            .expect("sample WebSpaces root missing");
        let lines = space_detail_lines(webspaces, &snapshot, 80);

        assert!(lines.iter().any(|line| line == "Group      Spaces"));
        assert!(lines
            .iter()
            .any(|line| line.contains("localhost://WebSpaces/Elastos")));
        assert!(lines.iter().any(|line| line.contains("elastos webspace")));
    }

    #[test]
    fn mywebsite_page_is_task_oriented_and_hides_space_roots() {
        let snapshot = sample_snapshot();
        let lines = mywebsite_task_lines(&snapshot);
        let text = lines.join("\n");

        assert!(text.contains("Stage    mywebsite stage <dir>"));
        assert!(text.contains("Preview  mywebsite preview"));
        assert!(text.contains("Publish  mywebsite publish"));
        assert!(text.contains("Open     mywebsite open"));
        assert!(!text.contains("WebSpaces"));
        assert!(!text.contains("scratch space"));
        assert!(!text.contains("localhost://Local"));
    }

    #[test]
    fn home_cli_line_mode_reads_browser_exit_service_offers() {
        let snapshot = sample_snapshot();
        let exits = cli_service_offers(&snapshot, "remote_exit");
        let names = exits
            .iter()
            .map(|offer| first_json_text(offer, &["display_name", "offer_id"]))
            .collect::<Vec<_>>();

        assert_eq!(exits.len(), 2);
        assert!(names.contains(&"Browser Exit node"));
        assert!(names.contains(&"Seed Node Browser Exit"));
    }

    #[test]
    fn home_cli_line_mode_builds_low_risk_invoke_intent() {
        let snapshot = sample_snapshot();
        let intent = resolve_cli_invoke_intent("browser page_status", &snapshot).unwrap();
        assert_eq!(intent.capsule, "browser");
        assert_eq!(intent.interface_id, "elastos.browser.page");
        assert_eq!(intent.method, "page_status");
        assert_eq!(intent.input, serde_json::json!({}));
    }

    #[test]
    fn home_cli_line_mode_serializes_structured_invoke_home_intent() {
        let snapshot = sample_snapshot();
        let intent =
            resolve_cli_invoke_intent("browser page_status {\"page_id\":\"default\"}", &snapshot)
                .unwrap();
        let payload = home_intent_payload("invoke", Some(intent)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "action": "invoke",
                "invoke": {
                    "capsule": "browser",
                    "interface": "elastos.browser.page",
                    "method": "page_status",
                    "input": {
                        "page_id": "default"
                    }
                }
            })
        );
    }

    #[test]
    fn home_cli_line_mode_blocks_high_risk_invoke_intent() {
        let mut snapshot = sample_snapshot();
        let methods = snapshot
            .capsule_interfaces
            .as_mut()
            .and_then(|registry| registry.get_mut("interfaces"))
            .and_then(|interfaces| interfaces.as_array_mut())
            .and_then(|interfaces| interfaces.first_mut())
            .and_then(|entry| entry.get_mut("interface"))
            .and_then(|interface| interface.get_mut("methods"))
            .and_then(|methods| methods.as_array_mut())
            .expect("sample interface methods");
        methods.push(serde_json::json!({
            "id": "payment.send",
            "risk": "payment",
            "approval": "runtime_policy"
        }));
        let error = resolve_cli_invoke_intent("browser payment.send", &snapshot)
            .unwrap_err()
            .to_string();
        assert!(error.contains("payment risk requires explicit user approval"));
    }

    fn sample_snapshot() -> HomeSnapshot {
        HomeSnapshot {
            version: "0.1.0".to_string(),
            user: "anders".to_string(),
            nickname: Some("anders".to_string()),
            did: Some("did:key:z6MkhExample".to_string()),
            session: HomeCliSessionStatus {
                mode: "browser_pty".to_string(),
                extra: BTreeMap::from([(
                    session_auth_state_key(),
                    serde_json::json!("launch-token authorized browser Home session"),
                )]),
            },
            source: Some(SourceStatus {
                name: "elastos.elacitylabs.com".to_string(),
                channel: "stable".to_string(),
                installed_version: "0.1.0".to_string(),
                gateway: Some("https://elastos.elacitylabs.com".to_string()),
            }),
            runtime: RuntimeStatus {
                running: true,
                kind: Some("managed".to_string()),
                peer_count: Some(2),
                ticket: Some("ticket:example".to_string()),
                running_capsules: vec!["chat".to_string()],
            },
            system_services: vec![],
            services: Some(serde_json::json!({
                "schema": "elastos.runtime.services/v1",
                "local_offers": [{
                    "offer_id": "local:provider:browser-exit",
                    "service_kind": "remote_exit",
                    "display_name": "Browser Exit node",
                    "status": "configured",
                    "route": "/apps/browser/"
                }],
                "remote_offers": [{
                    "offer_id": "remote:seed:browser-exit",
                    "service_kind": "remote_exit",
                    "display_name": "Seed Node Browser Exit",
                    "status": "available"
                }]
            })),
            site: SiteStatus {
                staged: true,
                local_url: None,
                active_release: None,
                active_channel: None,
                active_bundle_cid: None,
                release_count: 0,
            },
            shares: ShareStatus::default(),
            room: RoomStatus {
                room_slug: "chat-room".to_string(),
                title: "Room".to_string(),
                owner_did: Some("did:key:z6Mkowner".to_string()),
                current_key_epoch: 1,
                admin_count: 1,
                member_count: 3,
                active_member_count: 1,
                pending_invite_count: 0,
                allow_guest_invites: true,
                allow_member_invites: true,
                allow_members_to_host_guests: true,
                local_runtime_role: Some("owner".to_string()),
                canonical_hosted_guest_url: Some(
                    "https://elastos.elacitylabs.com/apps/chat-room/".to_string(),
                ),
                ephemeral_hosted_guest_url: None,
                browser_access_allowed: true,
                browser_access_block_reason: None,
                pending_count: 0,
                active_session_count: 0,
                active_participants: Vec::new(),
                pending_requests: Vec::new(),
                active_sessions: Vec::new(),
                members: vec![RoomMemberStatus {
                    member_did: "did:key:z6MkhExample".to_string(),
                    role: "owner".to_string(),
                }],
                pending_invites: Vec::new(),
            },
            people: PeopleStatus::default(),
            notifications: NotificationStatus::default(),
            roots: vec![
                RootStatus {
                    name: "Users".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://Users".to_string(),
                    path: Some("/tmp/Users".to_string()),
                    exists: true,
                    description: "People root".to_string(),
                    example: "localhost://Users/<principal-root>".to_string(),
                },
                RootStatus {
                    name: "UsersAI".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://UsersAI".to_string(),
                    path: Some("/tmp/UsersAI".to_string()),
                    exists: true,
                    description: "AI root".to_string(),
                    example: "localhost://UsersAI/self".to_string(),
                },
                RootStatus {
                    name: "MyWebSite".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://MyWebSite".to_string(),
                    path: Some("/tmp/MyWebSite".to_string()),
                    exists: true,
                    description: "Site root".to_string(),
                    example: "localhost://MyWebSite/index.html".to_string(),
                },
                RootStatus {
                    name: "Public".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://Public".to_string(),
                    path: Some("/tmp/Public".to_string()),
                    exists: true,
                    description: "Shared root".to_string(),
                    example: "localhost://Public/manual.pdf".to_string(),
                },
                RootStatus {
                    name: "Local".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://Local".to_string(),
                    path: Some("/tmp/Local".to_string()),
                    exists: true,
                    description: "Local root".to_string(),
                    example: "localhost://Local/Shared".to_string(),
                },
                RootStatus {
                    name: "WebSpaces".to_string(),
                    kind: "dynamic".to_string(),
                    uri: "localhost://WebSpaces".to_string(),
                    path: None,
                    exists: false,
                    description: "Dynamic root".to_string(),
                    example: "localhost://WebSpaces/Elastos".to_string(),
                },
                RootStatus {
                    name: "ElastOS".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://ElastOS".to_string(),
                    path: Some("/tmp/ElastOS".to_string()),
                    exists: true,
                    description: "System root".to_string(),
                    example: "localhost://ElastOS/SystemRegistry".to_string(),
                },
            ],
            actions: vec![
                ActionInfo {
                    id: "chat".to_string(),
                    label: "Chat".to_string(),
                    description: String::new(),
                    command: "home: open Chat".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "site-local".to_string(),
                    label: "Preview".to_string(),
                    description: String::new(),
                    command: "home: start MyWebSite local preview".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "site-ephemeral".to_string(),
                    label: "Publish".to_string(),
                    description: String::new(),
                    command: "home: publish a temporary HTTPS URL for MyWebSite".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "site-open".to_string(),
                    label: "Open".to_string(),
                    description: String::new(),
                    command: "home: open MyWebSite preview in browser".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "shares-list".to_string(),
                    label: "Shared".to_string(),
                    description: String::new(),
                    command: "elastos shares list".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "capsule-chat-wasm".to_string(),
                    label: "chat-wasm".to_string(),
                    description: "Packaged WASM chat bundle".to_string(),
                    command: "elastos capsule chat-wasm --lifecycle interactive --interactive"
                        .to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "capsule-gba-ucity".to_string(),
                    label: "gba-ucity".to_string(),
                    description: "Bundled uCity demo cartridge".to_string(),
                    command: "elastos capsule gba-ucity --lifecycle interactive --interactive"
                        .to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "capsule-gba-emulator".to_string(),
                    label: "gba-emulator".to_string(),
                    description: "Browser GBA viewer bundle".to_string(),
                    command: "elastos capsule gba-emulator --lifecycle interactive --interactive"
                        .to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "capsule-browser".to_string(),
                    label: "browser".to_string(),
                    description: "Open web sites through the ElastOS Browser boundary.".to_string(),
                    command: "elastos capsule browser --lifecycle interactive --interactive"
                        .to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "capsule-mystery-capsule".to_string(),
                    label: "mystery-capsule".to_string(),
                    description: "Unknown capsule".to_string(),
                    command:
                        "elastos capsule mystery-capsule --lifecycle interactive --interactive"
                            .to_string(),
                    ready: true,
                    reason: None,
                },
            ],
            active_shell: ActiveShellStatus {
                active: Some("home-cli".to_string()),
            },
            cached_capsules: vec![
                "chat".to_string(),
                "agent".to_string(),
                "mystery-capsule".to_string(),
            ],
            capsule_catalog: Some(serde_json::json!({
                "schema": "elastos.capsules.catalog/v1",
                "counts": {
                    "total": 2,
                    "installed": 2,
                    "launchable": 2,
                    "interfaces": 2,
                    "methods": 3
                },
                "capsules": [
                    {
                        "name": "browser",
                        "version": "0.1.0",
                        "title": "Browser",
                        "role": "app",
                        "type": "wasm",
                        "state": "installed",
                        "installed": true,
                        "launchable": true,
                        "launch_target": "browser",
                        "route": "/apps/browser/",
                        "interfaces": [{
                            "id": "elastos.browser.page",
                            "methods": [
                                { "id": "page_status", "risk": "read", "approval": "runtime_policy" },
                                { "id": "open", "risk": "launch", "approval": "runtime_policy" }
                            ]
                        }],
                        "projection": {
                            "web": { "state": "available" },
                            "cli": { "state": "available" },
                            "facts": { "state": "available" },
                            "affordances": { "state": "declared" },
                            "gates": {
                                "state": "declared",
                                "note": "Runtime route policy, launch tokens, Inbox/Wallet approval, and provider gates remain authoritative."
                            },
                            "audit_mirror": {
                                "state": "redacted",
                                "note": "signature=no-manifest-signature; cid=local-only; payment=not-declared; drm=not-declared; ordinary shells receive redacted mirror facts."
                            },
                            "carrier": { "state": "requires-provider-intents" }
                        },
                        "cid_state": "local-only",
                        "signature_state": "no-manifest-signature",
                        "trust_state": "local-dev",
                        "payment_state": "not-declared",
                        "drm_state": "not-declared",
                        "source": "installed"
                    },
                    {
                        "name": "home-cli",
                        "version": "0.1.0",
                        "title": "Home CLI",
                        "role": "shell",
                        "type": "wasm",
                        "state": "installed",
                        "installed": true,
                        "launchable": true,
                        "launch_target": "home-cli",
                        "route": "/apps/home-cli/",
                        "interfaces": [],
                        "projection": {
                            "web": { "state": "available" },
                            "cli": { "state": "available" },
                            "facts": { "state": "available" },
                            "affordances": { "state": "absent" },
                            "gates": { "state": "absent" },
                            "audit_mirror": { "state": "redacted" },
                            "carrier": { "state": "none" }
                        },
                        "cid_state": "local-only",
                        "signature_state": "no-manifest-signature",
                        "trust_state": "local-dev",
                        "payment_state": "not-declared",
                        "drm_state": "not-declared",
                        "source": "installed"
                    }
                ]
            })),
            capsule_interfaces: Some(serde_json::json!({
                "schema": "elastos.capsules.interfaces/v1",
                "counts": {
                    "capsules": 1,
                    "interfaces": 1,
                    "methods": 2
                },
                "interfaces": [{
                    "capsule": "browser",
                    "capsule_version": "0.1.0",
                    "title": "Browser",
                    "role": "app",
                    "type": "wasm",
                    "trust_state": "local-dev",
                    "interface": {
                        "id": "elastos.browser.page",
                        "title": "Browser Page",
                        "methods": [
                            { "id": "page_status", "risk": "read", "approval": "runtime_policy" },
                            { "id": "open", "risk": "launch", "approval": "runtime_policy" }
                        ]
                    }
                }]
            })),
            notice: None,
        }
    }

    #[derive(Clone)]
    struct InboxFixtureScenario {
        name: &'static str,
        entry: NotificationEntryStatus,
        primary_action: ActionInfo,
        extra_actions: Vec<ActionInfo>,
        primary_action_id: &'static str,
    }

    impl InboxFixtureScenario {
        fn apply(&self, snapshot: &mut HomeSnapshot) {
            snapshot.notifications.entries = vec![self.entry.clone()];
            snapshot.notifications.unread_count = 1;
            snapshot.notifications.attention_count =
                usize::from(self.entry.severity == "attention");
            snapshot.actions.push(self.primary_action.clone());
            snapshot.actions.extend(self.extra_actions.clone());
        }
    }

    fn inbox_action(
        id: &'static str,
        label: &'static str,
        description: &'static str,
        command: &'static str,
    ) -> ActionInfo {
        ActionInfo {
            id: id.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            command: command.to_string(),
            ready: true,
            reason: None,
        }
    }

    fn inbox_entry(
        id: &'static str,
        source_app: &'static str,
        kind: &'static str,
        title: &'static str,
        body: &'static str,
        severity: &'static str,
        action_app: &'static str,
        action_id: &'static str,
    ) -> NotificationEntryStatus {
        NotificationEntryStatus {
            id: id.to_string(),
            source_app: source_app.to_string(),
            kind: kind.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            action_ref: Some(NotificationActionRefStatus {
                app: action_app.to_string(),
                action_id: action_id.to_string(),
            }),
            read: false,
            severity: severity.to_string(),
        }
    }

    fn wallet_signing_inbox_fixture() -> InboxFixtureScenario {
        InboxFixtureScenario {
            name: "wallet signing",
            entry: inbox_entry(
                "wallet-signing:tx-1",
                "wallet",
                "wallet_signing_request",
                "Wallet signature requested",
                "ela.city wants Wallet to sign a transaction.",
                "attention",
                "wallet",
                "open-gui:wallet",
            ),
            primary_action: inbox_action(
                "open-gui:wallet",
                "Open Wallet",
                "Review and sign or reject the pending wallet request.",
                "home: open Wallet",
            ),
            extra_actions: Vec::new(),
            primary_action_id: "open-gui:wallet",
        }
    }

    fn inspect_approval_inbox_fixture() -> InboxFixtureScenario {
        InboxFixtureScenario {
            name: "inspect approval",
            entry: inbox_entry(
                "inspect-approval:key-release-1",
                "system",
                "inspect_approval_request",
                "Inspect approval requested",
                "Capsule Inspector wants approval for key.release.",
                "attention",
                "system",
                "open-gui:system",
            ),
            primary_action: inbox_action(
                "open-gui:system",
                "Open System",
                "Review the Inspector gate preview before approving.",
                "home: open System",
            ),
            extra_actions: Vec::new(),
            primary_action_id: "open-gui:system",
        }
    }

    fn people_request_inbox_fixture() -> InboxFixtureScenario {
        InboxFixtureScenario {
            name: "people request",
            entry: inbox_entry(
                "people-request:people-req-1",
                "people",
                "people_request",
                "Bob wants to connect",
                "Bob from peer-bob sent a People request.",
                "attention",
                "people",
                "people-accept-request:people-req-1",
            ),
            primary_action: inbox_action(
                "people-accept-request:people-req-1",
                "Accept Bob",
                "Accept this People request.",
                "home: accept People request",
            ),
            extra_actions: Vec::new(),
            primary_action_id: "people-accept-request:people-req-1",
        }
    }

    fn chat_guest_inbox_fixture() -> InboxFixtureScenario {
        InboxFixtureScenario {
            name: "chat guest request",
            entry: inbox_entry(
                "room-access-request:chat-guest-1",
                "chat-room",
                "room_access_request",
                "Alice wants to join Chat",
                "Alice on Phone wants to join Chat.",
                "attention",
                "chat-room",
                "room-approve-request:chat-guest-1",
            ),
            primary_action: inbox_action(
                "room-approve-request:chat-guest-1",
                "Approve Alice on Phone",
                "Approve this Chat guest request.",
                "home: approve Chat guest",
            ),
            extra_actions: vec![inbox_action(
                "room-deny-request:chat-guest-1",
                "Deny Alice on Phone",
                "Deny this Chat guest request.",
                "home: deny Chat guest",
            )],
            primary_action_id: "room-approve-request:chat-guest-1",
        }
    }

    fn generic_capsule_inbox_fixture() -> InboxFixtureScenario {
        InboxFixtureScenario {
            name: "generic capsule notification",
            entry: inbox_entry(
                "capsule-documents-ready",
                "documents",
                "capsule_notification",
                "Documents finished importing",
                "Documents has a completed import ready to review.",
                "attention",
                "documents",
                "open-gui:documents",
            ),
            primary_action: inbox_action(
                "open-gui:documents",
                "Open Documents",
                "Open Documents to review the completed import.",
                "home: open Documents",
            ),
            extra_actions: Vec::new(),
            primary_action_id: "open-gui:documents",
        }
    }

    fn inbox_fixture_scenarios() -> Vec<InboxFixtureScenario> {
        vec![
            wallet_signing_inbox_fixture(),
            inspect_approval_inbox_fixture(),
            people_request_inbox_fixture(),
            chat_guest_inbox_fixture(),
            generic_capsule_inbox_fixture(),
        ]
    }

    #[test]
    fn home_actions_stay_task_focused() {
        let snapshot = sample_snapshot();
        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat"]);
    }

    #[test]
    fn ignores_startup_enter_on_default_home_selection() {
        let state = TuiState::default();
        let now = Instant::now();
        assert!(matches!(
            startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
            HomeLaunchDecision::Defer(_)
        ));
    }

    #[test]
    fn ignores_duplicate_startup_enter_inside_settle_window() {
        let state = TuiState::default();
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(
                &state,
                UiKey::Enter,
                true,
                Some(now + STARTUP_ENTER_SETTLE_WINDOW),
                now
            ),
            HomeLaunchDecision::IgnoreDuplicate
        );
    }

    #[test]
    fn allows_enter_after_settle_window() {
        let state = TuiState::default();
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(
                &state,
                UiKey::Enter,
                true,
                Some(now),
                now + STARTUP_ENTER_SETTLE_WINDOW
            ),
            HomeLaunchDecision::Allow
        );
    }

    #[test]
    fn does_not_defer_enter_after_default_home_launch_is_armed_and_ready() {
        let state = TuiState::default();
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(
                &state,
                UiKey::Enter,
                true,
                Some(now),
                now + STARTUP_ENTER_SETTLE_WINDOW
            ),
            HomeLaunchDecision::Allow
        );
    }

    #[test]
    fn does_not_defer_non_enter_keys() {
        let state = TuiState::default();
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(&state, UiKey::Digit(1), false, None, now),
            HomeLaunchDecision::NotApplicable
        );
    }

    #[test]
    fn does_not_defer_enter_off_default_home_selection() {
        let mut state = TuiState::default();
        state.home_index = 1;
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
            HomeLaunchDecision::NotApplicable
        );
    }

    #[test]
    fn does_not_defer_enter_when_help_is_open() {
        let mut state = TuiState::default();
        state.show_help = true;
        let now = Instant::now();
        assert_eq!(
            startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
            HomeLaunchDecision::NotApplicable
        );
    }

    #[test]
    fn default_app_entries_only_include_cli_native_surfaces() {
        let snapshot = sample_snapshot();
        let entries = app_entries(&snapshot);
        let labels = entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["Chat"]);
        assert!(entries.iter().all(|entry| !entry.is_control));
        assert!(!labels.contains(&"Full-screen Chat"));
        assert!(!labels.contains(&"Browser"));
        assert!(!labels.contains(&"GBA UCity"));
        assert!(!labels.contains(&"Shared Conversation"));
        assert!(!labels.contains(&"Shared"));
    }

    #[test]
    fn quick_launch_only_includes_working_home_actions() {
        let snapshot = sample_snapshot();
        let ids: Vec<&str> = quick_launch_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat"]);
    }

    #[test]
    fn every_visible_default_menu_item_is_actionable_or_has_next_step() {
        let mut snapshot = sample_snapshot();
        snapshot.site.staged = false;
        snapshot.actions.insert(
            1,
            ActionInfo {
                id: "room-approve".to_string(),
                label: "Approve web guest".to_string(),
                description: "Approve a pending Chat request.".to_string(),
                command: "home approve".to_string(),
                ready: false,
                reason: Some("open Inbox and review the request first".to_string()),
            },
        );

        assert_eq!(
            DEFAULT_TABS,
            &[Tab::Home, Tab::Inbox, Tab::People, Tab::Apps, Tab::System]
        );

        for action_idx in home_action_indices(&snapshot) {
            let action = &snapshot.actions[action_idx];
            assert!(
                action.ready
                    || action
                        .reason
                        .as_ref()
                        .is_some_and(|reason| !reason.is_empty()),
                "visible Home action must be ready or explain the next step: {}",
                action.id
            );
        }
        let mut blocked_state = TuiState::default();
        blocked_state.home_index = 1;
        assert_eq!(blocked_state.activate(&snapshot), None);

        for (index, entry) in app_entries(&snapshot).iter().enumerate() {
            let action = selected_app_action(&snapshot, index)
                .unwrap_or_else(|| panic!("visible app has no action: {}", entry.label));
            assert!(
                action.ready,
                "visible default app must be immediately actionable: {}",
                entry.label
            );
        }

        let screen = build_tui_screen(&snapshot, &TuiState::default(), 120, 40);
        assert!(screen.contains("1 Chat [ready]"));
        assert!(screen.contains("Approve access [setup]"));
        assert!(screen.contains("setup: open Inbox and review the request first"));
        assert!(screen.contains("elastos site stage"));
        for hidden in [
            "Spaces",
            "Full-screen Chat",
            "Browser [ready]",
            "GBA UCity",
            "Updates [ready]",
            "Shared Conversation",
        ] {
            assert!(
                !screen.contains(hidden),
                "default Home CLI leaked hidden/developer item: {hidden}",
            );
        }
    }

    #[test]
    fn blocked_mywebsite_is_not_a_default_home_action() {
        let mut snapshot = sample_snapshot();
        snapshot.site.staged = false;
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "site-local")
        {
            action.ready = false;
            action.reason = Some("stage a site first".to_string());
        }
        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat"]);
        assert!(alerts_lines(&snapshot, 120, None)
            .iter()
            .any(|line| line.contains("elastos site stage")));
    }

    #[test]
    fn shared_catalog_entries_stay_out_of_default_home_actions() {
        let mut snapshot = sample_snapshot();
        snapshot.shares.channel_count = 1;
        snapshot.shares.active_count = 1;

        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat"]);
    }

    #[test]
    fn pending_browser_access_surfaces_approval_actions_on_home() {
        let mut snapshot = sample_snapshot();
        snapshot.room.pending_count = 1;
        snapshot.notifications.entries = vec![NotificationEntryStatus {
            id: "room-pair-request:req-1".to_string(),
            source_app: "chat-room".to_string(),
            kind: "room_pair_request".to_string(),
            title: "Alice wants to join Chat".to_string(),
            body: "Alice on Phone wants to join Chat.".to_string(),
            action_ref: Some(NotificationActionRefStatus {
                app: "chat-room".to_string(),
                action_id: "room-approve-request:req-1".to_string(),
            }),
            read: false,
            severity: "attention".to_string(),
        }];
        snapshot.notifications.unread_count = 1;
        snapshot.notifications.attention_count = 1;
        snapshot.actions.insert(
            1,
            ActionInfo {
                id: "room-approve".to_string(),
                label: "Approve web guest".to_string(),
                description: String::new(),
                command: "home approve".to_string(),
                ready: true,
                reason: None,
            },
        );
        snapshot.actions.insert(
            2,
            ActionInfo {
                id: "room-deny".to_string(),
                label: "Deny web guest".to_string(),
                description: String::new(),
                command: "home deny".to_string(),
                ready: true,
                reason: None,
            },
        );

        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "room-approve", "room-deny"]);
        let alerts = alerts_lines(&snapshot, 120, None);
        assert!(alerts
            .iter()
            .any(|line| line.contains("Alice on Phone wants to join Chat.")));
    }

    #[test]
    fn active_browser_sessions_surface_disconnect_action_on_home() {
        let mut snapshot = sample_snapshot();
        snapshot.room.active_session_count = 2;
        snapshot.room.active_participants = vec![
            RoomParticipantStatus {
                display_name: "Alice".to_string(),
                device_label: "Phone".to_string(),
            },
            RoomParticipantStatus {
                display_name: "Bob".to_string(),
                device_label: "Safari".to_string(),
            },
        ];
        snapshot.actions.insert(
            1,
            ActionInfo {
                id: "room-revoke-all".to_string(),
                label: "Disconnect browsers".to_string(),
                description: String::new(),
                command: "home revoke".to_string(),
                ready: true,
                reason: None,
            },
        );

        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "room-revoke-all"]);

        let alerts = alerts_lines(&snapshot, 120, None);
        assert!(alerts
            .iter()
            .any(|line| line
                .contains("2 active web guest session(s): Alice on Phone, Bob on Safari")));
    }

    #[test]
    fn people_tab_uses_people_model_and_keeps_transport_in_debug() {
        let mut snapshot = sample_snapshot();
        snapshot.room.pending_count = 1;
        snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
            request_id: "req-1".to_string(),
            display_name: "Alice".to_string(),
            device_label: "Phone".to_string(),
        }];
        snapshot.room.active_session_count = 1;
        snapshot.room.active_sessions = vec![RoomSessionStatus {
            token: "tok-1".to_string(),
            display_name: "Bob".to_string(),
            device_label: "Safari".to_string(),
        }];
        snapshot.actions.push(ActionInfo {
            id: "room-approve-request:req-1".to_string(),
            label: "Approve Alice on Phone".to_string(),
            description: String::new(),
            command: "home approve specific".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-deny-request:req-1".to_string(),
            label: "Deny Alice on Phone".to_string(),
            description: String::new(),
            command: "home deny specific".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-revoke-session:tok-1".to_string(),
            label: "Disconnect Bob on Safari".to_string(),
            description: String::new(),
            command: "home disconnect specific".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.people = PeopleStatus {
            schema: "elastos.people.contacts/v1".to_string(),
            contact_count: 1,
            contacts: vec![PeopleContactStatus {
                contact_id: "contact-alice".to_string(),
                display_name: "Alice".to_string(),
                handle: Some("@alice".to_string()),
                relationship: "connected".to_string(),
                route: "elastos://peer/peer-alice".to_string(),
                can_message: true,
                device_label: Some("peer-alice".to_string()),
                profile_card: None,
                last_seen_at: Some(10),
            }],
            service_offer_count: 0,
            discovery: PeopleDiscoveryStatus {
                schema: "elastos.people.discovery/v1".to_string(),
                enabled: true,
                remaining_seconds: Some(120),
                visibility: "visible".to_string(),
                status: "ready".to_string(),
                status_message: "Discovery is ready.".to_string(),
                topic: "__elastos_internal/people-discovery-v1".to_string(),
                local_peer_id: Some("peer-local".to_string()),
                discovered_count: 1,
                discovered_peers: vec![PeopleDiscoveryPeerStatus {
                    peer_id: "peer-bob".to_string(),
                    did: Some("did:key:bob".to_string()),
                    display_name: "Bob".to_string(),
                    handle: Some("@bob".to_string()),
                    last_seen_at: 20,
                    status: "visible".to_string(),
                }],
                request_count: 1,
                requests: vec![PeopleDiscoveryRequestStatus {
                    request_id: "request-carol".to_string(),
                    peer_id: "peer-carol".to_string(),
                    did: Some("did:key:carol".to_string()),
                    display_name: "Carol".to_string(),
                    handle: Some("@carol".to_string()),
                    created_at: 30,
                    status: "incoming".to_string(),
                    invite_id: None,
                }],
                next_refresh_after_ms: None,
            },
        };

        let ids: Vec<String> = people_actions(&snapshot)
            .into_iter()
            .map(|action| action.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "people-discovery-disable",
                "people-discovery-refresh",
                "people-accept-request:request-carol",
                "people-request-peer:peer-bob",
                "people-message:contact-alice",
                "people-remove-contact:contact-alice",
            ]
        );

        let mut buf = String::new();
        render_people_tab(&mut buf, &snapshot, &TuiState::default(), 120);
        assert!(buf.contains("My Profile"));
        assert!(buf.contains("People"));
        assert!(buf.contains("Discovery"));
        assert!(buf.contains("Visible People"));
        assert!(buf.contains("Requests"));
        assert!(buf.contains("Alice"));
        assert!(buf.contains("Bob"));
        assert!(buf.contains("Carol"));
        assert!(buf.contains("Chat with Alice"));
        assert!(buf.contains("Remove Alice"));
        for hidden in [
            "Conversation",
            "Request    Alice on Phone",
            "Web guest  Bob on Safari",
            "Ticket",
            "Carrier",
            "Roots",
            "RoomGuests",
        ] {
            assert!(
                !buf.contains(hidden),
                "normal People view leaked debug detail: {hidden}"
            );
        }

        let debug = people_debug_lines(&snapshot).join("\n");
        assert!(debug.contains("Ticket"));
        assert!(debug.contains("RoomGuests"));
        assert!(debug.contains("__elastos_internal/people-discovery-v1"));
    }

    #[test]
    fn room_control_details_remain_available_to_debug_helpers() {
        let mut snapshot = sample_snapshot();
        snapshot.room.title = "Room".to_string();
        snapshot.room.room_slug = "chat-room".to_string();
        snapshot.room.local_runtime_role = Some("owner".to_string());
        snapshot.room.owner_did = Some("did:key:z6Mkowner".to_string());
        snapshot.room.current_key_epoch = 3;
        snapshot.room.admin_count = 1;
        snapshot.room.member_count = 4;
        snapshot.room.active_member_count = 2;
        snapshot.room.pending_count = 1;
        snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
            request_id: "req-1".to_string(),
            display_name: "Alice".to_string(),
            device_label: "Phone".to_string(),
        }];
        snapshot.room.pending_invite_count = 1;
        snapshot.room.pending_invites = vec![RoomInviteStatus {
            invite_id: "inv-1".to_string(),
            invited_did: "did:key:z6invitee".to_string(),
        }];
        snapshot.room.active_session_count = 1;
        snapshot.room.active_sessions = vec![RoomSessionStatus {
            token: "tok-1".to_string(),
            display_name: "Bob".to_string(),
            device_label: "Safari".to_string(),
        }];
        snapshot.room.members = vec![
            RoomMemberStatus {
                member_did: "did:key:z6Mkowner".to_string(),
                role: "owner".to_string(),
            },
            RoomMemberStatus {
                member_did: "did:key:z6member".to_string(),
                role: "member".to_string(),
            },
        ];

        let entry = chat_room_app_entry(&snapshot).expect("room entry missing");

        let detail_lines = chat_room_app_detail_lines(&snapshot, &entry, 120);
        assert!(detail_lines.iter().any(
            |line| line.contains("Public URL https://elastos.elacitylabs.com/apps/chat-room/")
        ));
        assert!(detail_lines
            .iter()
            .any(|line| line.contains("Invite     did:key:z6invitee pending")));
        assert!(detail_lines
            .iter()
            .any(|line| line.contains("Person     did:key:z6member (trusted participant)")));
    }

    #[test]
    fn targeted_room_controls_do_not_appear_as_default_apps() {
        let mut snapshot = sample_snapshot();
        snapshot.room.allow_guest_invites = true;
        snapshot.room.allow_member_invites = false;
        snapshot.room.pending_count = 1;
        snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
            request_id: "req-1".to_string(),
            display_name: "Alice".to_string(),
            device_label: "Phone".to_string(),
        }];
        snapshot.room.pending_invite_count = 1;
        snapshot.room.pending_invites = vec![RoomInviteStatus {
            invite_id: "inv-1".to_string(),
            invited_did: "did:key:z6member".to_string(),
        }];
        snapshot.room.active_session_count = 1;
        snapshot.room.active_sessions = vec![RoomSessionStatus {
            token: "tok-1".to_string(),
            display_name: "Bob".to_string(),
            device_label: "Safari".to_string(),
        }];
        snapshot.room.members.push(RoomMemberStatus {
            member_did: "did:key:z6member".to_string(),
            role: "member".to_string(),
        });
        snapshot.actions.push(ActionInfo {
            id: "room-policy-toggle-guests".to_string(),
            label: "Close public join requests".to_string(),
            description: "Stop new web guests from requesting access through the public Chat link."
                .to_string(),
            command: "home toggle public join requests".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-policy-toggle-members".to_string(),
            label: "Open ElastOS user invites".to_string(),
            description: "Allow new invites for trusted ElastOS users.".to_string(),
            command: "home toggle ElastOS user invites".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-revoke-invite:inv-1".to_string(),
            label: "Revoke invite for did:key:z6member".to_string(),
            description: "Cancel this pending ElastOS user invite".to_string(),
            command: "home revoke invite".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-remove-member:did:key:z6member".to_string(),
            label: "Remove did:key:z6member".to_string(),
            description: "Remove this trusted participant".to_string(),
            command: "home remove member".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-approve-request:req-1".to_string(),
            label: "Approve Alice on Phone".to_string(),
            description: "Approve this web guest".to_string(),
            command: "home approve specific".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-deny-request:req-1".to_string(),
            label: "Deny Alice on Phone".to_string(),
            description: "Deny this browser".to_string(),
            command: "home deny specific".to_string(),
            ready: true,
            reason: None,
        });
        snapshot.actions.push(ActionInfo {
            id: "room-revoke-session:tok-1".to_string(),
            label: "Disconnect Bob on Safari".to_string(),
            description: "Disconnect this browser".to_string(),
            command: "home disconnect specific".to_string(),
            ready: true,
            reason: None,
        });

        let entries = app_entries(&snapshot);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Chat"]
        );

        let controls = room_control_entries(&snapshot);
        assert!(controls
            .iter()
            .any(|entry| entry.label == "Close public join requests" && entry.is_control));
        assert!(controls
            .iter()
            .any(|entry| entry.label == "Approve Alice on Phone" && entry.is_control));
        assert!(controls
            .iter()
            .any(|entry| entry.label == "Disconnect Bob on Safari" && entry.is_control));
    }

    #[test]
    fn inbox_tab_surfaces_notifications_and_resolves_actions() {
        let mut snapshot = sample_snapshot();
        snapshot.notifications.unread_count = 1;
        snapshot.notifications.attention_count = 1;
        snapshot.notifications.entries = vec![NotificationEntryStatus {
            id: "room-pair-request:req-1".to_string(),
            source_app: "chat-room".to_string(),
            kind: "room_pair_request".to_string(),
            title: "Alice wants to join Chat".to_string(),
            body: "Alice on Phone wants to join Chat.".to_string(),
            action_ref: Some(NotificationActionRefStatus {
                app: "chat-room".to_string(),
                action_id: "room-approve-request:req-1".to_string(),
            }),
            read: false,
            severity: "attention".to_string(),
        }];
        snapshot.actions.push(ActionInfo {
            id: "room-approve-request:req-1".to_string(),
            label: "Approve Alice on Phone".to_string(),
            description: "Approve this browser".to_string(),
            command: "home approve specific".to_string(),
            ready: true,
            reason: None,
        });

        let mut state = TuiState::default();
        state.tab = Tab::Inbox;

        let mut buf = String::new();
        render_inbox_tab(&mut buf, &snapshot, &state, 120);
        assert!(buf.contains("Alice wants to join Chat"));
        assert!(buf.contains("Approve Alice on Phone"));
        assert_eq!(
            state.activate(&snapshot),
            Some("room-approve-request:req-1".to_string())
        );
    }

    #[test]
    fn inbox_fixture_scenarios_resolve_actions_and_mark_dismiss_intents() {
        for scenario in inbox_fixture_scenarios() {
            let mut snapshot = sample_snapshot();
            scenario.apply(&mut snapshot);

            let mut state = TuiState::default();
            state.tab = Tab::Inbox;

            let mut buf = String::new();
            render_inbox_tab(&mut buf, &snapshot, &state, 200);
            assert!(
                buf.contains(&scenario.entry.title),
                "Inbox did not render {} scenario title",
                scenario.name
            );
            assert!(
                buf.contains(&scenario.entry.body),
                "Inbox did not render {} scenario body",
                scenario.name
            );
            assert_eq!(
                selected_notification_read_action(&snapshot, 0),
                Some(format!("notification-read:{}", scenario.entry.id)),
                "{} mark-read intent drifted",
                scenario.name
            );
            assert_eq!(
                selected_notification_dismiss_action(&snapshot, 0),
                Some(format!("notification-dismiss:{}", scenario.entry.id)),
                "{} dismiss intent drifted",
                scenario.name
            );
            assert_eq!(
                state.activate(&snapshot),
                Some(scenario.primary_action_id.to_string()),
                "{} primary action drifted",
                scenario.name
            );
            assert!(
                buf.contains("Enter      run this inbox action and return here"),
                "{} Inbox selected action did not promise return behavior",
                scenario.name
            );
        }
    }

    #[test]
    fn inbox_chat_guest_fixture_exposes_approve_and_deny_paths() {
        let scenario = chat_guest_inbox_fixture();
        let mut snapshot = sample_snapshot();
        scenario.apply(&mut snapshot);

        let approve = selected_notification_action(&snapshot, 0).expect("approve action");
        assert_eq!(approve.id, "room-approve-request:chat-guest-1");
        assert!(approve.ready);

        let deny = action_by_id(&snapshot, "room-deny-request:chat-guest-1").expect("deny action");
        assert_eq!(deny.label, "Deny Alice on Phone");
        assert!(deny.ready);

        let mut state = TuiState::default();
        state.tab = Tab::Inbox;
        assert_eq!(
            state.activate(&snapshot),
            Some("room-approve-request:chat-guest-1".to_string())
        );
        assert_eq!(
            selected_notification_dismiss_action(&snapshot, 0),
            Some("notification-dismiss:room-access-request:chat-guest-1".to_string())
        );
    }

    #[test]
    fn inbox_selected_index_controls_mark_dismiss_and_open() {
        let wallet = wallet_signing_inbox_fixture();
        let generic = generic_capsule_inbox_fixture();
        let mut snapshot = sample_snapshot();
        wallet.apply(&mut snapshot);
        snapshot.notifications.entries.push(generic.entry.clone());
        snapshot.actions.push(generic.primary_action.clone());
        snapshot.notifications.unread_count = 2;
        snapshot.notifications.attention_count = 2;

        let mut state = TuiState::default();
        state.tab = Tab::Inbox;
        state.inbox_index = 1;

        assert_eq!(
            selected_notification_read_action(&snapshot, state.inbox_index),
            Some("notification-read:capsule-documents-ready".to_string())
        );
        assert_eq!(
            selected_notification_dismiss_action(&snapshot, state.inbox_index),
            Some("notification-dismiss:capsule-documents-ready".to_string())
        );
        assert_eq!(
            state.activate(&snapshot),
            Some("open-gui:documents".to_string())
        );
    }

    #[test]
    fn tabs_keep_people_between_inbox_and_apps() {
        let mut state = TuiState::default();
        state.next_tab();
        assert_eq!(state.tab, Tab::Inbox);
        state.next_tab();
        assert_eq!(state.tab, Tab::People);
        state.next_tab();
        assert_eq!(state.tab, Tab::Apps);
        state.prev_tab();
        assert_eq!(state.tab, Tab::People);
        state.prev_tab();
        assert_eq!(state.tab, Tab::Inbox);
    }

    #[test]
    fn shared_catalog_entries_stay_out_of_default_apps() {
        let mut snapshot = sample_snapshot();
        assert!(!app_entries(&snapshot)
            .iter()
            .any(|entry| entry.label == "Shared"));

        snapshot.shares.channel_count = 1;
        snapshot.shares.active_count = 1;

        assert!(!app_entries(&snapshot)
            .iter()
            .any(|entry| entry.label == "Shared"));
    }

    #[test]
    fn blocked_home_action_is_visible_but_not_activated() {
        let mut snapshot = sample_snapshot();
        snapshot.actions.insert(
            1,
            ActionInfo {
                id: "room-approve".to_string(),
                label: "Approve web guest".to_string(),
                description: "Approve a pending request.".to_string(),
                command: "home approve".to_string(),
                ready: false,
                reason: Some("open Inbox and review the request first".to_string()),
            },
        );
        let mut state = TuiState::default();
        state.tab = Tab::Home;
        state.home_index = 1;

        assert_eq!(state.activate(&snapshot), None);
        assert!(
            render_home_actions(&snapshot, &home_action_indices(&snapshot), 1, 120)
                .join("\n")
                .contains("setup: open Inbox and review the request first")
        );
    }

    #[test]
    fn visible_width_ignores_ansi_escape_sequences() {
        assert_eq!(visible_text_width("\x1b[30;46;1m Home \x1b[0m"), 6);
    }

    #[test]
    fn parse_escape_sequence_bytes_handles_partial_and_arrow_sequences() {
        assert_eq!(parse_escape_sequence_bytes(&[]), UiKey::None);
        assert_eq!(parse_escape_sequence_bytes(&[b'[']), UiKey::None);
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'A']), UiKey::Up);
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'B']), UiKey::Down);
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'C']), UiKey::Right);
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'D']), UiKey::Left);
        assert_eq!(parse_escape_sequence_bytes(&[b'O', b'A']), UiKey::Up);
        assert_eq!(
            parse_escape_sequence_bytes(&[b'[', b'1', b';', b'5', b'A']),
            UiKey::Up
        );
        assert_eq!(
            parse_escape_sequence_bytes(&[b'[', b'1', b';', b'2', b'D']),
            UiKey::Left
        );
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'Z']), UiKey::Left);
        assert_eq!(
            parse_escape_sequence_bytes(&[b'[', b'1', b';', b'2', b'Z']),
            UiKey::Left
        );
    }

    #[test]
    fn standalone_escape_exits_tui() {
        assert_eq!(parse_escape_sequence_bytes(&[]), UiKey::None);
        assert_eq!(escape_sequence_key(&[]), UiKey::Quit);
        assert_eq!(escape_sequence_key(&[b'[', b'Z']), UiKey::Left);
    }

    #[test]
    fn parse_escape_sequence_bytes_handles_sgr_mouse_sequences() {
        assert_eq!(
            parse_escape_sequence_bytes(b"[<64;10;8M"),
            UiKey::Mouse(MouseEvent {
                button: 64,
                x: 10,
                y: 8,
                released: false,
            })
        );
        assert_eq!(
            parse_escape_sequence_bytes(b"[<0;44;4M"),
            UiKey::Mouse(MouseEvent {
                button: 0,
                x: 44,
                y: 4,
                released: false,
            })
        );
        let long_coordinate = b"[<0;129;43M";
        assert!(
            ESCAPE_SEQUENCE_MAX_BYTES >= long_coordinate.len(),
            "mouse coordinates must fit in the escape-sequence read buffer"
        );
        assert_eq!(
            parse_escape_sequence_bytes(long_coordinate),
            UiKey::Mouse(MouseEvent {
                button: 0,
                x: 129,
                y: 43,
                released: false,
            })
        );
        assert_eq!(
            parse_escape_sequence_bytes(b"[<0;44;4m"),
            UiKey::Mouse(MouseEvent {
                button: 0,
                x: 44,
                y: 4,
                released: true,
            })
        );
    }

    #[test]
    fn parse_escape_sequence_bytes_handles_legacy_mouse_sequences() {
        let click = [b'[', b'M', 32, 44, 35];
        assert_eq!(
            parse_escape_sequence_bytes(&click),
            UiKey::Mouse(MouseEvent {
                button: 0,
                x: 12,
                y: 3,
                released: false,
            })
        );

        let release = [b'[', b'M', 35, 44, 35];
        assert_eq!(
            parse_escape_sequence_bytes(&release),
            UiKey::Mouse(MouseEvent {
                button: 3,
                x: 12,
                y: 3,
                released: true,
            })
        );
    }

    #[test]
    fn escape_sequence_completion_waits_for_legacy_mouse_coordinates() {
        assert!(!is_escape_sequence_complete(b"["));
        assert!(!is_escape_sequence_complete(b"[M"));
        assert!(!is_escape_sequence_complete(b"[M  "));
        assert!(is_escape_sequence_complete(b"[M  #"));
        assert!(is_escape_sequence_complete(b"[<0;12;3M"));
        assert!(is_escape_sequence_complete(b"[A"));
    }

    #[test]
    fn mouse_clicks_use_the_rendered_tab_row() {
        let snapshot = sample_snapshot();
        let mut state = TuiState::default();
        assert!(state.handle_mouse(
            MouseEvent {
                button: 0,
                x: 45,
                y: TUI_TAB_ROW,
                released: false,
            },
            120,
            &snapshot,
        ));
        assert_eq!(state.tab, Tab::Inbox);

        let mut state = TuiState::default();
        assert!(!state.handle_mouse(
            MouseEvent {
                button: 0,
                x: 45,
                y: TUI_TAB_ROW + 1,
                released: false,
            },
            120,
            &snapshot,
        ));
        assert_eq!(state.tab, Tab::Home);
    }

    #[test]
    fn home_screen_stays_compact() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(&snapshot, &TuiState::default(), 100, 32);
        assert!(!screen.contains("Start Here"));
        assert!(!screen.contains("-- Status --"));
        assert!(screen.starts_with("\x1b[H\x1b[J"));
        assert!(!screen.ends_with("\r\n"));
        assert!(screen.contains("1 Chat [ready]"));
        assert!(!screen.contains("MyWebSite [ready]"));
        assert!(!screen.contains("Updates [ready]"));
        assert!(!screen.contains("Shared [ready]"));
        assert!(screen.contains("Up/Down select"));
        assert!(screen.contains("q/Esc home-gui"));
        assert!(screen.contains("? help"));
        assert!(!screen.contains("opens Browser"));
        assert!(!screen.contains("hjkl"));
    }

    #[test]
    fn tui_tabs_replace_redundant_banner() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                tab: Tab::Apps,
                ..TuiState::default()
            },
            100,
            32,
        );

        assert!(!screen.contains("ElastOS Home"));
        assert!(!screen.contains("ElastOS Apps"));
        assert!(screen.contains("\x1b[30;46;1m Apps \x1b[0m"));
    }

    #[test]
    fn tui_help_matches_keyboard_contract() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                show_help: true,
                ..TuiState::default()
            },
            120,
            60,
        );

        let plain = screen.replace("\r\n", " ");
        assert!(plain.contains("Tab"));
        assert!(plain.contains("switch to the next section"));
        assert!(plain.contains("Shift+Tab"));
        assert!(plain.contains("switch to the previous section"));
        assert!(plain.contains("q or Esc"));
        assert!(plain.contains("m marks read"));
        assert!(plain.contains("d dismisses"));
        assert!(screen.contains("? close help"));
        assert!(!screen.contains("? help"));
        assert!(!screen.contains("hjkl"));
    }

    #[test]
    fn default_tabs_stay_within_viewport_so_tabs_do_not_scroll_away() {
        let snapshot = sample_snapshot();
        let rows = 20usize;
        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                tab: Tab::Apps,
                ..TuiState::default()
            },
            100,
            rows,
        );
        let rendered_rows = screen.split("\r\n").count();
        let first_line = screen
            .strip_prefix("\x1b[H\x1b[J")
            .unwrap_or(&screen)
            .split("\r\n")
            .next()
            .unwrap_or_default();

        assert!(
            rendered_rows <= rows,
            "Apps tab rendered {rendered_rows} rows into a {rows}-row terminal"
        );
        assert!(first_line.contains("\x1b[30;46;1m Apps \x1b[0m"));
        assert!(!first_line.contains("anders"));
        assert!(!first_line.contains("Home CLI"));
    }

    #[test]
    fn every_tui_page_keeps_tabs_inside_viewport() {
        let snapshot = sample_snapshot();
        let rows = 20usize;
        let states = [
            TuiState {
                tab: Tab::Home,
                ..TuiState::default()
            },
            TuiState {
                tab: Tab::Inbox,
                ..TuiState::default()
            },
            TuiState {
                tab: Tab::People,
                ..TuiState::default()
            },
            TuiState {
                tab: Tab::Apps,
                ..TuiState::default()
            },
            TuiState {
                tab: Tab::System,
                ..TuiState::default()
            },
        ];

        for state in states {
            let screen = build_tui_screen(&snapshot, &state, 100, rows);
            let rendered_rows = screen.split("\r\n").count();
            let first_line = screen
                .strip_prefix("\x1b[H\x1b[J")
                .unwrap_or(&screen)
                .split("\r\n")
                .next()
                .unwrap_or_default();

            assert!(
                rendered_rows <= rows,
                "TUI page {:?} rendered {rendered_rows} rows into a {rows}-row terminal",
                state.tab
            );
            assert!(
                first_line.contains("Home")
                    && first_line.contains("Inbox")
                    && first_line.contains("People")
                    && first_line.contains("Apps")
                    && first_line.contains("System")
                    && !first_line.contains("Spaces"),
                "TUI page {:?} lost its tab row: {first_line:?}",
                state.tab
            );
        }
    }

    #[test]
    fn tui_starts_at_tab_row_without_summary_header() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(&snapshot, &TuiState::default(), 100, 20);
        let first_line = screen
            .strip_prefix("\x1b[H\x1b[J")
            .unwrap_or(&screen)
            .split("\r\n")
            .next()
            .unwrap_or_default();

        assert!(first_line.contains("\x1b[30;46;1m Home \x1b[0m"));
        assert!(!first_line.contains("anders"));
        assert!(!first_line.contains("Home CLI"));
        assert!(!first_line.contains("identity ready"));
        assert!(!first_line.contains("bootstrap ready"));
        assert!(!first_line.contains("site empty"));
    }

    #[test]
    fn tui_lines_do_not_trigger_terminal_autowrap() {
        let snapshot = sample_snapshot();
        let cols = 100usize;
        let states = [
            TuiState {
                tab: Tab::Home,
                ..TuiState::default()
            },
            TuiState {
                tab: Tab::Apps,
                ..TuiState::default()
            },
        ];

        for state in states {
            let screen = build_tui_screen(&snapshot, &state, cols, 24);
            for line in screen
                .strip_prefix("\x1b[H\x1b[J")
                .unwrap_or(&screen)
                .split("\r\n")
            {
                assert!(
                    visible_text_width(line) < cols,
                    "TUI page {:?} emitted a full-width line that can trigger xterm autowrap: {:?}",
                    state.tab,
                    line
                );
            }
        }
    }

    #[test]
    fn mywebsite_notice_dedupes_empty_alert_on_home() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                notice: Some(
                    "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`. Then reopen MyWebSite from Home to preview or go public.".to_string(),
                ),
                ..TuiState::default()
            },
            100,
            32,
        );

        assert_eq!(screen.matches("MyWebSite is empty.").count(), 1);
        assert!(!screen.contains("Needs attention"));
    }

    #[test]
    fn staged_site_summary_and_banner_stay_honest() {
        let mut snapshot = sample_snapshot();
        snapshot.site.local_url = None;
        snapshot.site.active_release = None;
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "site-local")
        {
            action.ready = false;
            action.reason =
                Some("missing site-provider — run: elastos setup --profile demo".to_string());
        }

        assert_eq!(
            website_summary(&snapshot),
            "staged at localhost://MyWebSite"
        );
    }

    #[test]
    fn mywebsite_tasks_show_staged_site_and_next_steps() {
        let mut snapshot = sample_snapshot();
        snapshot.site.local_url = None;
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "site-local")
        {
            action.ready = false;
            action.reason =
                Some("missing site-provider — run: elastos setup --profile demo".to_string());
        }

        let screen = mywebsite_task_lines(&snapshot).join("\n");
        assert!(screen.contains("Status   staged at localhost://MyWebSite"));
        assert!(screen.contains("Stage    mywebsite stage <dir>"));
        assert!(screen.contains("Preview  mywebsite preview (blocked"));
        assert!(screen.contains("Open     mywebsite open"));
        assert!(!screen.contains("press Enter"));
    }

    #[test]
    fn system_tab_stays_short_and_actionable() {
        let snapshot = sample_snapshot();
        let lines = compact_system_lines(&snapshot);
        assert!(lines.len() <= 10);
        assert!(lines.iter().any(|line| line.starts_with("Updates")));
        assert!(lines.iter().any(|line| line.starts_with("ElastOS")));
        assert!(lines.iter().any(|line| line.starts_with("Switch")));
        assert!(lines.iter().any(|line| line == "Root       ElastOS"));
        assert!(lines.iter().any(|line| line.starts_with("Next")));
        assert!(lines
            .iter()
            .any(|line| line.contains("system shell home-gui")));
        assert!(!lines.iter().any(|line| line.starts_with("API")));
    }
}
