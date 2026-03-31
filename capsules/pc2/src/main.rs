use anyhow::{anyhow, Result};
use elastos_guest::runtime::RuntimeClient;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
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
const ESCAPE_SEQUENCE_MAX_BYTES: usize = 8;

#[derive(Debug, Clone, Deserialize)]
struct Pc2Snapshot {
    version: String,
    user: String,
    nickname: Option<String>,
    did: Option<String>,
    source: Option<SourceStatus>,
    runtime: RuntimeStatus,
    system_services: Vec<SystemServiceStatus>,
    site: SiteStatus,
    #[serde(default)]
    shares: ShareStatus,
    roots: Vec<RootStatus>,
    actions: Vec<ActionInfo>,
    #[serde(default)]
    cached_capsules: Vec<String>,
    #[serde(default)]
    notice: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
struct SourceStatus {
    name: String,
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
struct Pc2Intent<'a> {
    action: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    People,
    Spaces,
    Apps,
    System,
}

#[derive(Debug, Clone)]
struct TuiState {
    tab: Tab,
    home_index: usize,
    people_index: usize,
    space_index: usize,
    app_index: usize,
    show_help: bool,
    notice: Option<String>,
}

#[derive(Debug, Clone)]
struct AppEntry {
    name: String,
    action_id: String,
    label: String,
    category: &'static str,
    description: &'static str,
    command: String,
    state: String,
}

#[derive(Clone, Copy)]
struct AppSurfaceSpec {
    name: &'static str,
    action_id: &'static str,
    label: &'static str,
    category: &'static str,
    description: &'static str,
    command: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Refresh,
    Quit,
    Help,
    Digit(usize),
    None,
}

struct TerminalGuard;

const APP_SURFACES: &[AppSurfaceSpec] = &[
    AppSurfaceSpec {
        name: "chat",
        action_id: "chat",
        label: "Chat",
        category: "Communication",
        description: "Talk to people and connected PC2s from this local world.",
        command: "elastos chat",
    },
    AppSurfaceSpec {
        name: "chat-microvm",
        action_id: "chat-microvm",
        label: "IRC",
        category: "Communication",
        description: "Packaged full-screen Carrier-backed IRC chat in a microVM, using the same room as native Chat.",
        command: "elastos capsule chat --lifecycle interactive --interactive",
    },
    AppSurfaceSpec {
        name: "chat-wasm",
        action_id: "chat-wasm",
        label: "Chat WASM",
        category: "Communication",
        description: "Packaged full-screen Carrier-backed IRC chat in WASM for the explicit non-KVM capsule path.",
        command: "elastos capsule chat-wasm --lifecycle interactive --interactive",
    },
    AppSurfaceSpec {
        name: "site-local",
        action_id: "site-local",
        label: "MyWebSite",
        category: "Web",
        description: "Open your local site preview in the browser and keep release/public actions nearby.",
        command: "elastos site serve --mode local --browser",
    },
    AppSurfaceSpec {
        name: "site-ephemeral",
        action_id: "site-ephemeral",
        label: "Go public",
        category: "Web",
        description: "Start a temporary public HTTPS URL for MyWebSite and return home when it is ready.",
        command: "elastos site serve --mode ephemeral",
    },
    AppSurfaceSpec {
        name: "shares-list",
        action_id: "shares-list",
        label: "Shared",
        category: "Web",
        description: "Review shared channels, open links, and follow the next steps for public content.",
        command: "elastos shares list",
    },
    AppSurfaceSpec {
        name: "gba-ucity",
        action_id: "gba-ucity",
        label: "GBA UCity",
        category: "Creative",
        description:
            "Open the bundled uCity game in the GBA viewer with the ROM preloaded, plus a public URL when available.",
        command: "./scripts/gba.sh capsules/gba-ucity",
    },
    AppSurfaceSpec {
        name: "update-check",
        action_id: "update-check",
        label: "Updates",
        category: "System",
        description: "Check the stamped trusted release line and return home with a concise result.",
        command: "elastos update --check",
    },
];

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

fn provider_call(
    token: &str,
    scheme: &str,
    op: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    with_client(|client| {
        client
            .provider_call(scheme, op, body, token)
            .map_err(|e| anyhow!("Provider call {}/{} failed: {}", scheme, op, e))
    })
}

fn storage_read(token: &str, path: &str) -> Result<Vec<u8>> {
    let result = provider_call(
        token,
        "localhost",
        "read",
        &serde_json::json!({
            "path": path,
            "token": token,
        }),
    )?;
    let data = result
        .get("data")
        .and_then(|value| value.get("content").or_else(|| value.get("data")))
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
    provider_call(
        token,
        "localhost",
        "write",
        &serde_json::json!({
            "path": path,
            "token": token,
            "content": content,
            "append": false,
        }),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let session_root = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("pc2 capsule missing session root argument"))?;
    let session_scope = format!("{}/*", session_root.trim_end_matches('/'));
    let read_token = request_capability(&session_scope, "read")?;
    let write_token = request_capability(&session_scope, "write")?;
    let snapshot_path = format!("{}/snapshot.json", session_root.trim_end_matches('/'));
    let intent_path = format!("{}/intent.json", session_root.trim_end_matches('/'));
    let snapshot: Pc2Snapshot =
        serde_json::from_slice(&storage_read(&read_token, &snapshot_path)?)?;

    dashboard_loop(&snapshot, &write_token, &intent_path)
}

fn dashboard_loop(snapshot: &Pc2Snapshot, write_token: &str, intent_path: &str) -> Result<()> {
    if should_use_tui() {
        dashboard_tui_loop(snapshot, write_token, intent_path)
    } else {
        dashboard_line_loop(snapshot, write_token, intent_path)
    }
}

fn should_use_tui() -> bool {
    if let Ok(mode) = std::env::var("ELASTOS_PC2_TUI") {
        return matches!(mode.as_str(), "1" | "true" | "yes");
    }

    if std::env::var("ELASTOS_TERM_COLS").is_ok() && std::env::var("ELASTOS_TERM_ROWS").is_ok() {
        return true;
    }

    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn dashboard_tui_loop(snapshot: &Pc2Snapshot, write_token: &str, intent_path: &str) -> Result<()> {
    let mut state = TuiState::default();
    let _guard = TerminalGuard::enter()?;
    let mut startup_input_drained = false;
    let mut home_launch_armed = false;
    let mut home_launch_ready_at: Option<Instant> = None;
    let mut needs_render = true;

    loop {
        if needs_render {
            render_tui(snapshot, &state)?;
            needs_render = false;
        }
        if !startup_input_drained {
            drain_startup_input()?;
            startup_input_drained = true;
        }

        let key = read_ui_key()?;
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
                state.move_prev(snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Down => {
                state.move_next(snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Enter => {
                state.notice = None;
                home_launch_ready_at = None;
                if let Some(action_id) = state.activate(snapshot) {
                    write_intent(write_token, intent_path, action_id)?;
                    return Ok(());
                }
            }
            UiKey::Digit(index) => {
                let quick_actions = quick_launch_action_indices(snapshot);
                if let Some(action_idx) = quick_actions.get(index.saturating_sub(1)).copied() {
                    state.tab = Tab::Home;
                    state.home_index = index.saturating_sub(1).min(quick_actions.len() - 1);
                    state.notice = None;
                    let action = &snapshot.actions[action_idx];
                    write_intent(write_token, intent_path, &action.id)?;
                    return Ok(());
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

fn dashboard_line_loop(snapshot: &Pc2Snapshot, write_token: &str, intent_path: &str) -> Result<()> {
    let quick_actions = quick_launch_action_indices(snapshot);
    loop {
        render_line_dashboard(snapshot)?;
        print!("Select action (number, r refresh, q exit, ? help): ");
        io::stdout().flush()?;

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

        let Ok(index) = trimmed.parse::<usize>() else {
            println!("Unknown command: {}. Type ? for help.", trimmed);
            wait_for_enter()?;
            continue;
        };

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
    let data = serde_json::to_vec_pretty(&Pc2Intent { action })?;
    storage_write(write_token, intent_path, data)
}

fn render_line_dashboard(snapshot: &Pc2Snapshot) -> Result<()> {
    print!("\x1B[2J\x1B[H");
    println!("ElastOS PC2");
    println!("A small-device home for people, spaces, apps, and system trust.");
    println!(
        "Version: runtime {}  pc2 {}  installed {}",
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
    println!("  MyWebSite: {}", website_summary(snapshot));
    println!("  Spaces:    MyWebSite, Public, Local, WebSpaces");
    println!(
        "  Capsules:  {} installed / {} running",
        snapshot.cached_capsules.len(),
        snapshot.runtime.running_capsules.len()
    );
    println!("  Source:    {}", source_label(snapshot));

    println!();
    let quick_actions = quick_launch_action_indices(snapshot);
    println!("Start Here");
    for (slot, action_idx) in quick_actions.iter().enumerate() {
        let action = &snapshot.actions[*action_idx];
        println!(
            "  {}. {} [{}]",
            slot + 1,
            action.label,
            if action.ready { "ready" } else { "blocked" }
        );
        println!("     {}", action.description);
        println!("     {}", action.command);
        if let Some(reason) = &action.reason {
            println!("     setup: {}", reason);
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
    println!("People");
    for line in people_summary_lines(snapshot) {
        println!("  {}", line);
    }

    println!();
    println!("Spaces");
    for line in spaces_summary_lines(snapshot) {
        println!("  {}", line);
    }

    println!();
    println!("Apps");
    for line in apps_summary_lines(snapshot) {
        println!("  {}", line);
    }

    println!();
    println!("System");
    for line in compact_system_summary_lines(snapshot) {
        println!("  {}", line);
    }

    let ready = snapshot
        .system_services
        .iter()
        .filter(|service| service.ready)
        .count();
    println!();
    println!(
        "  Services ready: {} / {}",
        ready,
        snapshot.system_services.len()
    );

    if let Some(notice) = &snapshot.notice {
        println!();
        println!("Notice");
        println!("  {}", notice);
    }

    println!();
    println!("Choose an action number, `r` to refresh, `q` to exit PC2 home, `?` for help.");
    io::stdout().flush()?;
    Ok(())
}

fn print_line_help() -> Result<()> {
    println!();
    println!("PC2 Dashboard Commands");
    println!("  <number>    Launch a quick action and return home afterward");
    println!("  r           Refresh the sovereign snapshot");
    println!("  q           Leave PC2 home");
    println!("  ?           Show this help");
    wait_for_enter()
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            tab: Tab::Home,
            home_index: 0,
            people_index: 0,
            space_index: 0,
            app_index: 0,
            show_help: false,
            notice: None,
        }
    }
}

impl TuiState {
    fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Home => Tab::People,
            Tab::People => Tab::Spaces,
            Tab::Spaces => Tab::Apps,
            Tab::Apps => Tab::System,
            Tab::System => Tab::Home,
        };
    }

    fn prev_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Home => Tab::System,
            Tab::People => Tab::Home,
            Tab::Spaces => Tab::People,
            Tab::Apps => Tab::Spaces,
            Tab::System => Tab::Apps,
        };
    }

    fn move_prev(&mut self, snapshot: &Pc2Snapshot) {
        match self.tab {
            Tab::Home => {
                if !home_action_indices(snapshot).is_empty() {
                    self.home_index = self.home_index.saturating_sub(1);
                }
            }
            Tab::People => {
                if !people_action_indices(snapshot).is_empty() {
                    self.people_index = self.people_index.saturating_sub(1);
                }
            }
            Tab::Spaces => {
                if !space_root_indices(snapshot).is_empty() {
                    self.space_index = self.space_index.saturating_sub(1);
                }
            }
            Tab::Apps => {
                if !app_entries(snapshot).is_empty() {
                    self.app_index = self.app_index.saturating_sub(1);
                }
            }
            Tab::System => {}
        }
    }

    fn move_next(&mut self, snapshot: &Pc2Snapshot) {
        match self.tab {
            Tab::Home => {
                let items = home_action_indices(snapshot);
                if !items.is_empty() {
                    self.home_index = (self.home_index + 1).min(items.len() - 1);
                }
            }
            Tab::People => {
                let items = people_action_indices(snapshot);
                if !items.is_empty() {
                    self.people_index = (self.people_index + 1).min(items.len() - 1);
                }
            }
            Tab::Spaces => {
                let items = space_root_indices(snapshot);
                if !items.is_empty() {
                    self.space_index = (self.space_index + 1).min(items.len() - 1);
                }
            }
            Tab::Apps => {
                let items = app_entries(snapshot);
                if !items.is_empty() {
                    self.app_index = (self.app_index + 1).min(items.len() - 1);
                }
            }
            Tab::System => {}
        }
    }

    fn activate<'a>(&self, snapshot: &'a Pc2Snapshot) -> Option<&'a str> {
        match self.tab {
            Tab::Home => selected_action(snapshot, &home_action_indices(snapshot), self.home_index)
                .map(|action| action.id.as_str()),
            Tab::People => selected_action(
                snapshot,
                &people_action_indices(snapshot),
                self.people_index,
            )
            .filter(|action| action.ready)
            .map(|action| action.id.as_str()),
            Tab::Spaces => {
                selected_space_action(snapshot, self.space_index).map(|action| action.id.as_str())
            }
            Tab::Apps => selected_app_action(snapshot, self.app_index)
                .filter(|action| action.ready)
                .map(|action| action.id.as_str()),
            Tab::System => None,
        }
    }
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        print!("\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H");
        io::stdout().flush()?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = write!(io::stdout(), "\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

fn read_ui_key() -> Result<UiKey> {
    let byte = read_stdin_byte()?;
    let key = match byte {
        b'q' | b'Q' => UiKey::Quit,
        b'r' | b'R' => UiKey::Refresh,
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

    if pc2_debug_keys() {
        eprintln!("[pc2-keys] byte={byte} parsed={key:?}");
    }

    Ok(key)
}

fn read_escape_sequence() -> Result<UiKey> {
    std::thread::sleep(ESCAPE_SEQUENCE_SETTLE_WINDOW);
    let mut seq = Vec::with_capacity(ESCAPE_SEQUENCE_MAX_BYTES);
    while seq.len() < ESCAPE_SEQUENCE_MAX_BYTES {
        let byte = read_stdin_byte()?;
        seq.push(byte);
        if is_escape_sequence_terminator(byte) {
            break;
        }
    }

    let key = parse_escape_sequence_bytes(&seq);
    if pc2_debug_keys() {
        eprintln!("[pc2-keys] esc-seq={seq:?} parsed={key:?}");
    }
    Ok(key)
}

fn parse_escape_sequence_bytes(seq: &[u8]) -> UiKey {
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
        _ => UiKey::None,
    }
}

fn is_escape_sequence_terminator(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'~')
}

fn render_tui(snapshot: &Pc2Snapshot, state: &TuiState) -> Result<()> {
    let cols = term_cols();
    let rows = term_rows();
    let screen = build_tui_screen(snapshot, state, cols, rows);

    print!("{}", screen);
    io::stdout().flush()?;
    Ok(())
}

fn build_tui_screen(snapshot: &Pc2Snapshot, state: &TuiState, cols: usize, rows: usize) -> String {
    let body_width = cols.saturating_sub(4);
    let mut screen = String::new();
    // Steady-state redraws repaint from the home position and clear to the end of the
    // alternate screen. This avoids old tail lines surviving shorter frames without
    // bringing back the heavier full-screen clear on every keypress.
    screen.push_str("\x1b[H\x1b[J");
    push_screen_line(&mut screen, &banner_line(snapshot, cols));
    push_screen_line(&mut screen, &fit_line(&header_summary_line(snapshot), cols));
    push_screen_line(&mut screen, &rule(cols));
    push_screen_line(&mut screen, &render_tabs(state.tab, cols));
    push_screen_line(&mut screen, &rule(cols));

    match state.tab {
        Tab::Home => render_home_tab(&mut screen, snapshot, state, body_width),
        Tab::People => render_people_tab(&mut screen, snapshot, state, body_width),
        Tab::Spaces => render_spaces_tab(&mut screen, snapshot, state, body_width),
        Tab::Apps => render_apps_tab(&mut screen, snapshot, state, body_width),
        Tab::System => render_system_tab(&mut screen, snapshot, body_width),
    }

    if state.show_help {
        push_screen_blank(&mut screen);
        push_screen_line(&mut screen, &section_title("Help", cols));
        for line in wrap_text(
            "Arrows or hjkl move, Tab switches sections, Enter launches the selected app, digits 1-9 quick-launch actions, r refreshes the snapshot, q leaves PC2 home.",
            body_width,
        ) {
            push_screen_line(&mut screen, &format!("  {}", line));
        }
    }

    if let Some(notice) = state
        .notice
        .as_deref()
        .or(snapshot.notice.as_deref())
        .filter(|notice| should_render_notice(notice))
    {
        push_screen_blank(&mut screen);
        push_screen_line(&mut screen, &section_title("Notice", cols));
        for line in wrap_text(notice, body_width) {
            push_screen_line(&mut screen, &format!("  {}", line));
        }
    }

    let footer_lines = 3usize;
    let used_lines = count_screen_lines(&screen);
    if rows > footer_lines && used_lines < rows.saturating_sub(footer_lines) {
        for _ in 0..(rows.saturating_sub(footer_lines) - used_lines) {
            push_screen_blank(&mut screen);
        }
    }

    push_screen_blank(&mut screen);
    push_screen_line(&mut screen, &rule(cols));
    push_screen_line(
        &mut screen,
        &fit_line(
            " Keys: hjkl/arrows  Tab  Enter  1-9  r refresh  q quit  ? help",
            cols,
        ),
    );
    trim_trailing_screen_newline(&mut screen);
    screen
}

fn trim_trailing_screen_newline(screen: &mut String) {
    if screen.ends_with("\r\n") {
        screen.truncate(screen.len().saturating_sub(2));
    }
}

fn render_home_tab(buf: &mut String, snapshot: &Pc2Snapshot, state: &TuiState, width: usize) {
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

fn render_people_tab(buf: &mut String, snapshot: &Pc2Snapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let mut you = vec![
        format!("User       {}", snapshot.user),
        format!("Nick       {}", display_name(snapshot)),
        format!("Identity   {}", identity_summary(snapshot)),
        format!("Network    {}", network_summary(snapshot)),
        format!(
            "Carrier    {} reachable",
            snapshot.runtime.peer_count.unwrap_or_default()
        ),
        "Roots      localhost://Users · localhost://UsersAI".to_string(),
        "Use        Keep your files in Users and your resident AI homes in UsersAI".to_string(),
    ];
    if let Some(ticket) = &snapshot.runtime.ticket {
        you.extend(wrap_with_label(
            "Ticket",
            &format!(
                "Share this with another PC2 to connect directly: {}",
                truncate(ticket, column_width.saturating_sub(44).max(16))
            ),
            column_width,
        ));
    } else {
        you.push("Ticket     waiting for runtime".to_string());
    }
    push_section_lines(&mut left, "You", &you);

    let people_actions = people_action_indices(snapshot);
    if !people_actions.is_empty() {
        let actions = people_actions
            .iter()
            .enumerate()
            .map(|(slot, action_idx)| {
                let action = &snapshot.actions[*action_idx];
                format!(
                    "{} {} [{}]",
                    selected_marker(slot == state.people_index),
                    action.label,
                    if action.ready { "ready" } else { "blocked" }
                )
            })
            .collect::<Vec<_>>();
        push_section_lines(&mut left, "Actions", &actions);
    }

    let mut connections = vec![
        format!(
            "Chat       {}",
            action_state_label(action_by_id(snapshot, "chat"))
        ),
        "Flow       Home -> Chat shows bootstrap, room join, peer count, and send status"
            .to_string(),
        format!(
            "Delivery   {}",
            if snapshot.runtime.peer_count.unwrap_or_default() == 0 {
                "Local only until another participant joins Chat, IRC, or Chat WASM"
            } else {
                "Open Chat, IRC, or Chat WASM and send a line to confirm room delivery"
            }
        ),
        format!(
            "IRC        {}",
            action_state_label(action_by_id(snapshot, "chat-microvm"))
        ),
        "Mode       IRC and Chat WASM are full-screen Carrier chat capsules, separate from native Chat but on the same room.".to_string(),
    ];
    if let Some(action) = action_by_id(snapshot, "chat-microvm") {
        if let Some(reason) = &action.reason {
            connections.extend(wrap_with_label("Prep", reason, column_width));
        }
    }
    push_section_lines(&mut right, "Connections", &connections);

    if let Some(action) = selected_action(snapshot, &people_actions, state.people_index) {
        let mut profile = vec![
            format!("Action     {}", action.label),
            format!(
                "State      {}",
                if action.ready { "ready" } else { "blocked" }
            ),
            format!("Command    {}", action.command),
        ];
        if let Some(reason) = &action.reason {
            profile.extend(wrap_with_label("Prep", reason, column_width));
        } else {
            profile.push("Enter      run this People action and return home".to_string());
        }
        push_section_lines(&mut right, "Profile", &profile);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_spaces_tab(buf: &mut String, snapshot: &Pc2Snapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let space_indices = space_root_indices(snapshot);
    let places: Vec<String> = space_indices
        .iter()
        .enumerate()
        .map(|(slot, root_idx)| {
            let root = &snapshot.roots[*root_idx];
            format!(
                "{} {}. {} [{}]",
                selected_marker(slot == state.space_index),
                slot + 1,
                root.name,
                space_state_label(root, snapshot)
            )
        })
        .collect();
    push_section_lines(&mut left, "Spaces", &places);

    if let Some(root) = selected_root(snapshot, state.space_index) {
        let mut details = space_detail_lines(root, snapshot, column_width);
        if let Some(action) = space_action_for_root(snapshot, &root.name) {
            details.push(format!(
                "Enter      {}",
                if action.ready {
                    space_enter_summary(&root.name, &action.label)
                } else {
                    blocked_space_enter_summary(&root.name)
                }
            ));
        }
        push_section_lines(&mut right, &root.name, &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_apps_tab(buf: &mut String, snapshot: &Pc2Snapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let entries = app_entries(snapshot);
    let list = render_app_list(&entries, state.app_index);
    push_section_lines(&mut left, "Apps", &list);

    if let Some(entry) = entries.get(state.app_index.min(entries.len().saturating_sub(1))) {
        let mut details = vec![
            format!("Surface    {}", entry.name),
            format!("State      {}", entry.state),
            format!("Category   {}", entry.category),
        ];
        details.extend(wrap_with_label(
            "What it does",
            entry.description,
            column_width,
        ));
        details.extend(wrap_with_label("Command", &entry.command, column_width));
        if let Some(action) = selected_app_action(snapshot, state.app_index) {
            if action.ready {
                details.push("Enter      launch from PC2".to_string());
            } else {
                details.push("Enter      not ready from PC2 yet".to_string());
                if let Some(reason) = &action.reason {
                    details.extend(wrap_with_label("Setup", reason, column_width));
                }
            }
        } else {
            details.push("Enter      no direct launch from PC2 yet".to_string());
        }
        push_section_lines(&mut right, &entry.label, &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_system_tab(buf: &mut String, snapshot: &Pc2Snapshot, width: usize) {
    let total_width = width.max(60);
    let lines = compact_system_lines(snapshot);
    for line in lines {
        push_screen_line(buf, &format!("  {}", fit_line(&line, total_width)));
    }
}

fn push_screen_line(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push_str("\r\n");
}

fn push_screen_blank(buf: &mut String) {
    buf.push_str("\r\n");
}

fn render_tabs(active: Tab, cols: usize) -> String {
    let tabs = [
        render_tab(active == Tab::Home, "Home"),
        render_tab(active == Tab::People, "People"),
        render_tab(active == Tab::Spaces, "Spaces"),
        render_tab(active == Tab::Apps, "Apps"),
        render_tab(active == Tab::System, "System"),
    ]
    .join("  ");
    pad_ansi_line(&tabs, cols)
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

fn people_summary_lines(snapshot: &Pc2Snapshot) -> Vec<String> {
    let mut lines = vec![
        format!("You        {}", snapshot.user),
        format!("Nick       {}", display_name(snapshot)),
        format!("Identity   {}", identity_summary(snapshot)),
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
            "IRC        {}",
            action_state_label(action_by_id(snapshot, "chat-microvm"))
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
    lines.push("Manage     elastos identity nickname set".to_string());
    lines
}

fn spaces_summary_lines(snapshot: &Pc2Snapshot) -> Vec<String> {
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

fn apps_summary_lines(snapshot: &Pc2Snapshot) -> Vec<String> {
    let entries = app_entries(snapshot);
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

fn compact_system_summary_lines(snapshot: &Pc2Snapshot) -> Vec<String> {
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
        format!("Trust      {}", source_label(snapshot)),
        format!(
            "Updates    {}",
            action_state_label(action_by_id(snapshot, "update-check"))
        ),
        format!(
            "Services   {} / {} ready",
            ready,
            snapshot.system_services.len()
        ),
        "Roots      ElastOS · PC2Host".to_string(),
    ]
}

fn compact_system_lines(snapshot: &Pc2Snapshot) -> Vec<String> {
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
    lines.push(format!(
        "PC2Host    {}",
        root_example(snapshot, "PC2Host", "localhost://PC2Host/AdaptationLayer")
    ));
    lines.push("Next       elastos pc2 --status for full detail".to_string());
    lines
}

fn root_example(snapshot: &Pc2Snapshot, name: &str, fallback: &str) -> String {
    snapshot
        .roots
        .iter()
        .find(|root| root.name == name)
        .map(|root| root.example.as_str())
        .filter(|example| !example.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn header_summary_line(snapshot: &Pc2Snapshot) -> String {
    let identity = if snapshot.did.is_some() {
        "identity ready"
    } else {
        "finish setup"
    };
    let peers = match snapshot.runtime.peer_count {
        Some(0) if snapshot.runtime.ticket.is_some() => "awaiting peers".to_string(),
        Some(0) => "starting up".to_string(),
        Some(1) => "1 endpoint reachable".to_string(),
        Some(count) => format!("{} endpoints reachable", count),
        None => "runtime offline".to_string(),
    };
    let site = website_status_label(snapshot);
    format!(
        "{}  •  {}  •  {}  •  {}",
        display_name(snapshot),
        identity,
        peers,
        site
    )
}

fn root_group_name(root: &str) -> &'static str {
    match root {
        "Users" | "UsersAI" => "People",
        "Local" | "Public" | "MyWebSite" | "WebSpaces" => "Spaces",
        "AppCapsules" => "Apps",
        "ElastOS" | "PC2Host" => "System",
        _ => "World",
    }
}

fn truncate_did(did: &str) -> String {
    truncate(did, 36)
}

fn network_summary(snapshot: &Pc2Snapshot) -> String {
    if !snapshot.runtime.running {
        return "home session not running yet".to_string();
    }

    let peers = snapshot.runtime.peer_count.unwrap_or(0);
    if peers == 0 {
        if snapshot.runtime.ticket.is_some() {
            "Carrier ready; waiting for another participant".to_string()
        } else {
            "starting up".to_string()
        }
    } else if peers == 1 {
        "1 Carrier endpoint reachable".to_string()
    } else {
        format!("{} Carrier endpoints reachable", peers)
    }
}

fn website_status_label(snapshot: &Pc2Snapshot) -> &'static str {
    if snapshot.site.active_release.is_some() {
        "site live"
    } else if snapshot.site.local_url.is_some() {
        "site preview ready"
    } else if snapshot.site.staged {
        "site staged"
    } else if snapshot.site.release_count > 0 {
        "site releases saved"
    } else {
        "site empty"
    }
}

fn identity_summary(snapshot: &Pc2Snapshot) -> String {
    snapshot
        .did
        .as_deref()
        .map(truncate_did)
        .unwrap_or_else(|| "not initialized yet".to_string())
}

fn display_name(snapshot: &Pc2Snapshot) -> String {
    snapshot
        .nickname
        .as_deref()
        .filter(|nick| !nick.is_empty())
        .unwrap_or(&snapshot.user)
        .to_string()
}

fn website_summary(snapshot: &Pc2Snapshot) -> String {
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

fn source_label(snapshot: &Pc2Snapshot) -> String {
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

fn banner_line(snapshot: &Pc2Snapshot, cols: usize) -> String {
    let title = if snapshot.runtime.running {
        "ElastOS PC2"
    } else {
        "ElastOS PC2 (offline)"
    };
    fit_line(title, cols)
}

fn section_title(title: &str, cols: usize) -> String {
    fit_line(title, cols)
}

fn home_action_indices(snapshot: &Pc2Snapshot) -> Vec<usize> {
    let mut indices = prioritized_action_indices(snapshot, &["chat", "site-local", "update-check"]);
    for idx in prioritized_ready_action_indices(snapshot, &["site-ephemeral"]) {
        if !indices.contains(&idx) {
            indices.push(idx);
        }
    }
    if snapshot.shares.channel_count > 0 {
        for idx in prioritized_ready_action_indices(snapshot, &["shares-list"]) {
            if !indices.contains(&idx) {
                indices.push(idx);
            }
        }
    }
    indices
}

fn people_action_indices(snapshot: &Pc2Snapshot) -> Vec<usize> {
    prioritized_action_indices(snapshot, &["identity-nickname-set", "chat", "chat-microvm"])
}

fn space_root_indices(snapshot: &Pc2Snapshot) -> Vec<usize> {
    root_indices_by_priority(snapshot, &["MyWebSite", "Public", "Local", "WebSpaces"])
}

fn quick_launch_action_indices(snapshot: &Pc2Snapshot) -> Vec<usize> {
    home_action_indices(snapshot)
}

fn prioritized_action_indices(snapshot: &Pc2Snapshot, ids: &[&str]) -> Vec<usize> {
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

fn prioritized_ready_action_indices(snapshot: &Pc2Snapshot, ids: &[&str]) -> Vec<usize> {
    prioritized_action_indices(snapshot, ids)
        .into_iter()
        .filter(|idx| snapshot.actions.get(*idx).map(|action| action.ready).unwrap_or(false))
        .collect()
}

fn root_indices_by_priority(snapshot: &Pc2Snapshot, names: &[&str]) -> Vec<usize> {
    let mut indices = Vec::new();
    for name in names {
        if let Some(idx) = snapshot.roots.iter().position(|root| root.name == *name) {
            indices.push(idx);
        }
    }
    indices
}

fn selected_action<'a>(
    snapshot: &'a Pc2Snapshot,
    indices: &[usize],
    selected: usize,
) -> Option<&'a ActionInfo> {
    let idx = indices.get(selected.min(indices.len().saturating_sub(1)))?;
    snapshot.actions.get(*idx)
}

fn selected_root<'a>(snapshot: &'a Pc2Snapshot, selected: usize) -> Option<&'a RootStatus> {
    let indices = space_root_indices(snapshot);
    let idx = indices.get(selected.min(indices.len().saturating_sub(1)))?;
    snapshot.roots.get(*idx)
}

fn selected_space_action<'a>(snapshot: &'a Pc2Snapshot, selected: usize) -> Option<&'a ActionInfo> {
    let root = selected_root(snapshot, selected)?;
    space_action_for_root(snapshot, &root.name)
}

fn selected_app_action<'a>(snapshot: &'a Pc2Snapshot, selected: usize) -> Option<&'a ActionInfo> {
    let entries = app_entries(snapshot);
    let entry = entries.get(selected.min(entries.len().saturating_sub(1)))?;
    action_by_id(snapshot, &entry.action_id)
}

fn action_by_id<'a>(snapshot: &'a Pc2Snapshot, id: &str) -> Option<&'a ActionInfo> {
    snapshot.actions.iter().find(|action| action.id == id)
}

fn space_action_for_root<'a>(snapshot: &'a Pc2Snapshot, root_name: &str) -> Option<&'a ActionInfo> {
    let action_id = match root_name {
        "MyWebSite" => "site-local",
        "Public" => "shares-list",
        _ => return None,
    };
    action_by_id(snapshot, action_id)
}

fn space_state_label(root: &RootStatus, snapshot: &Pc2Snapshot) -> &'static str {
    match root.name.as_str() {
        "MyWebSite" => {
            if snapshot.site.local_url.is_some() {
                "preview"
            } else if snapshot.site.staged {
                "staged"
            } else {
                "empty"
            }
        }
        "Public" => {
            if snapshot.shares.channel_count > 0 {
                "ready"
            } else {
                "empty"
            }
        }
        _ => {
            if root.exists || root.kind == "dynamic" {
                "ready"
            } else {
                "empty"
            }
        }
    }
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

fn space_enter_summary(root_name: &str, label: &str) -> String {
    match root_name {
        "MyWebSite" => "open MyWebSite status, preview, or next steps".to_string(),
        "Public" => "review shared channels and open links".to_string(),
        _ => format!("launch {}", label),
    }
}

fn blocked_space_enter_summary(root_name: &str) -> String {
    match root_name {
        "MyWebSite" => "show MyWebSite next steps and return home".to_string(),
        "Public" => "show public sharing status and return home".to_string(),
        _ => "show next step notice".to_string(),
    }
}

fn render_home_actions(
    snapshot: &Pc2Snapshot,
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

fn home_action_state<'a>(action: &'a ActionInfo, snapshot: &Pc2Snapshot) -> &'a str {
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
        "chat-microvm" => "Open the full-screen IRC capsule and return home",
        "chat-wasm" => "Open the full-screen WASM IRC capsule and return home",
        "site-local" => "Stage, preview, and check live state for MyWebSite",
        "site-ephemeral" => "Open a temporary public HTTPS URL for MyWebSite",
        "gba-ucity" => "Open uCity in the browser viewer and keep the public URL nearby",
        "shares-list" => "Review shared channels, open links, and next steps",
        "update-check" => "Check the current trusted release status",
        _ => action.description.as_str(),
    }
}

fn action_display_label<'a>(action: &'a ActionInfo) -> &'a str {
    match action.id.as_str() {
        "chat" => "Chat",
        "chat-microvm" => "IRC",
        "chat-wasm" => "Chat WASM",
        "site-local" => "MyWebSite",
        "site-ephemeral" => "Go public",
        "shares-list" => "Shared",
        "gba-ucity" => "GBA UCity",
        "update-check" => "Updates",
        _ => action.label.as_str(),
    }
}

fn current_notice<'a>(state: &'a TuiState, snapshot: &'a Pc2Snapshot) -> Option<&'a str> {
    state.notice.as_deref().or(snapshot.notice.as_deref())
}

fn alerts_lines(snapshot: &Pc2Snapshot, width: usize, notice: Option<&str>) -> Vec<String> {
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

fn runtime_state_label(snapshot: &Pc2Snapshot) -> String {
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
        && trimmed
            != "PC2 home is live. Launch an app and you return here automatically when it exits."
        && trimmed != "Snapshot refreshed from live local state."
        && !trimmed.starts_with("Returned home from ")
}

fn space_detail_lines(root: &RootStatus, snapshot: &Pc2Snapshot, width: usize) -> Vec<String> {
    let mut details = vec![
        format!("Group      {}", root_group_name(&root.name)),
        format!("Kind       {}", root.kind),
        format!("URI        {}", root.uri),
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
            } else if snapshot.site.staged {
                details.push("Preview    press Enter to start the local preview".to_string());
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
            details.push("Public     Home -> Go public gives a temporary HTTPS URL".to_string());
            details.extend(wrap_with_label(
                "Commands",
                "elastos site stage <dir> · Go public in PC2 · elastos site publish --release <name> · elastos site activate --channel live · elastos site rollback --target publisher",
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

fn app_entries(snapshot: &Pc2Snapshot) -> Vec<AppEntry> {
    let mut entries = Vec::new();

    for spec in APP_SURFACES {
        let Some(action) = action_by_id(snapshot, spec.action_id) else {
            continue;
        };
        let active = app_surface_active(snapshot, spec);
        if !(action.ready || active) {
            continue;
        }
        let state = if active { "active" } else { "ready" }.to_string();
        entries.push(AppEntry {
            name: spec.name.to_string(),
            action_id: spec.action_id.to_string(),
            label: spec.label.to_string(),
            category: spec.category,
            description: spec.description,
            command: if action.command.is_empty() {
                spec.command.to_string()
            } else {
                action.command.clone()
            },
            state,
        });
    }

    entries.sort_by_key(|entry| {
        (
            app_category_order(entry.category),
            entry.label.to_ascii_lowercase(),
        )
    });
    entries
}

fn app_surface_active(snapshot: &Pc2Snapshot, spec: &AppSurfaceSpec) -> bool {
    match spec.action_id {
        "site-local" => snapshot.site.local_url.is_some(),
        _ => snapshot.runtime.running_capsules.iter().any(|item| {
            item == spec.name
                || item.starts_with(&format!("{} ", spec.name))
                || item.starts_with(&format!("{}(", spec.name))
        }),
    }
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
            entry.label,
            entry.state
        ));
    }
    lines
}

fn app_category_order(category: &str) -> usize {
    match category {
        "Communication" => 0,
        "Web" => 1,
        "Creative" => 2,
        "System" => 3,
        _ => 5,
    }
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

fn count_screen_lines(screen: &str) -> usize {
    screen.matches("\r\n").count()
}

fn pc2_debug_keys() -> bool {
    std::env::var("ELASTOS_PC2_DEBUG_KEYS")
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

    fn sample_snapshot() -> Pc2Snapshot {
        Pc2Snapshot {
            version: "0.1.0".to_string(),
            user: "anders".to_string(),
            nickname: Some("anders".to_string()),
            did: Some("did:key:z6MkhExample".to_string()),
            source: Some(SourceStatus {
                name: "elastos.elacitylabs.com".to_string(),
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
            site: SiteStatus {
                staged: true,
                local_url: None,
                active_release: None,
                active_channel: None,
                active_bundle_cid: None,
                release_count: 0,
            },
            shares: ShareStatus::default(),
            roots: vec![
                RootStatus {
                    name: "Users".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://Users".to_string(),
                    path: Some("/tmp/Users".to_string()),
                    exists: true,
                    description: "People root".to_string(),
                    example: "localhost://Users/self".to_string(),
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
                    example: "localhost://Local/SharedByLocalUsersAndBots".to_string(),
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
                RootStatus {
                    name: "PC2Host".to_string(),
                    kind: "file-backed".to_string(),
                    uri: "localhost://PC2Host".to_string(),
                    path: Some("/tmp/PC2Host".to_string()),
                    exists: true,
                    description: "Host root".to_string(),
                    example: "localhost://PC2Host/AdaptationLayer".to_string(),
                },
            ],
            actions: vec![
                ActionInfo {
                    id: "chat".to_string(),
                    label: "Chat".to_string(),
                    description: String::new(),
                    command: "elastos chat".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "site-local".to_string(),
                    label: "MyWebSite".to_string(),
                    description: String::new(),
                    command: "elastos site serve --mode local".to_string(),
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
                    id: "update-check".to_string(),
                    label: "Check for updates".to_string(),
                    description: String::new(),
                    command: "elastos update --check".to_string(),
                    ready: true,
                    reason: None,
                },
                ActionInfo {
                    id: "chat-microvm".to_string(),
                    label: "IRC".to_string(),
                    description: String::new(),
                    command: "elastos capsule chat --lifecycle interactive --interactive"
                        .to_string(),
                    ready: false,
                    reason: Some("missing crosvm".to_string()),
                },
            ],
            cached_capsules: vec![
                "chat".to_string(),
                "agent".to_string(),
                "mystery-capsule".to_string(),
            ],
            notice: None,
        }
    }

    #[test]
    fn home_actions_stay_task_focused() {
        let snapshot = sample_snapshot();
        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "site-local", "update-check"]);
    }

    #[test]
    fn spaces_follow_user_facing_order() {
        let snapshot = sample_snapshot();
        let names: Vec<&str> = space_root_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.roots[idx].name.as_str())
            .collect();
        assert_eq!(names, vec!["MyWebSite", "Public", "Local", "WebSpaces"]);
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
    fn app_entries_only_include_launchable_capsules() {
        let snapshot = sample_snapshot();
        let entries = app_entries(&snapshot);
        assert!(entries
            .iter()
            .any(|entry| entry.label == "Chat" && entry.state == "active"));
        assert!(!entries.iter().any(|entry| entry.label == "Codex"));
        assert!(!entries.iter().any(|entry| entry.label == "Mystery Capsule"));
        assert!(!entries.iter().any(|entry| entry.label == "PC2 Home"));
        assert!(!entries.iter().any(|entry| entry.label == "GBA Viewer"));
        assert!(!entries.iter().any(|entry| entry.label == "IPFS Provider"));
    }

    #[test]
    fn quick_launch_includes_remaining_actions_after_primary_cards() {
        let snapshot = sample_snapshot();
        let ids: Vec<&str> = quick_launch_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "site-local", "update-check"]);
    }

    #[test]
    fn home_actions_stay_focused_even_when_irc_is_ready() {
        let mut snapshot = sample_snapshot();
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "chat-microvm")
        {
            action.ready = true;
            action.reason = None;
        }
        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "site-local", "update-check"]);
    }

    #[test]
    fn blocked_mywebsite_stays_on_home_before_shared() {
        let mut snapshot = sample_snapshot();
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
        assert_eq!(ids, vec!["chat", "site-local", "update-check"]);
    }

    #[test]
    fn shared_only_appears_on_home_when_catalog_has_entries() {
        let mut snapshot = sample_snapshot();
        snapshot.shares.channel_count = 1;
        snapshot.shares.active_count = 1;

        let ids: Vec<&str> = home_action_indices(&snapshot)
            .into_iter()
            .map(|idx| snapshot.actions[idx].id.as_str())
            .collect();
        assert_eq!(ids, vec!["chat", "site-local", "update-check", "shares-list"]);
    }

    #[test]
    fn blocked_home_action_can_still_return_next_step_notice() {
        let mut snapshot = sample_snapshot();
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "site-local")
        {
            action.ready = false;
            action.reason = Some("stage a site first".to_string());
        }
        let mut state = TuiState::default();
        state.tab = Tab::Home;
        state.home_index = 1;

        assert_eq!(state.activate(&snapshot), Some("site-local"));
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
        assert_eq!(parse_escape_sequence_bytes(&[b'[', b'Z']), UiKey::None);
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
        assert!(!screen.contains("IRC [setup]"));
        assert!(screen.contains("2 MyWebSite [ready]"));
        assert!(screen.contains("3 Updates [ready]"));
        assert!(!screen.contains("Shared [ready]"));
    }

    #[test]
    fn mywebsite_notice_dedupes_empty_alert_on_home() {
        let snapshot = sample_snapshot();
        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                notice: Some(
                    "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`. Then reopen MyWebSite from PC2 to preview or go public.".to_string(),
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
            action.reason = Some("missing site-provider".to_string());
        }

        assert_eq!(website_status_label(&snapshot), "site staged");
        assert_eq!(
            website_summary(&snapshot),
            "staged at localhost://MyWebSite"
        );
    }

    #[test]
    fn spaces_show_staged_site_and_next_step_entry() {
        let mut snapshot = sample_snapshot();
        snapshot.site.local_url = None;
        if let Some(action) = snapshot
            .actions
            .iter_mut()
            .find(|action| action.id == "site-local")
        {
            action.ready = false;
            action.reason = Some("missing site-provider".to_string());
        }

        let screen = build_tui_screen(
            &snapshot,
            &TuiState {
                tab: Tab::Spaces,
                ..TuiState::default()
            },
            120,
            32,
        );
        assert!(screen.contains("1. MyWebSite [staged]"));
        assert!(screen.contains("Enter      show MyWebSite next steps and return home"));
    }

    #[test]
    fn system_tab_stays_short_and_actionable() {
        let snapshot = sample_snapshot();
        let lines = compact_system_lines(&snapshot);
        assert!(lines.len() <= 10);
        assert!(lines.iter().any(|line| line.starts_with("Updates")));
        assert!(lines.iter().any(|line| line.starts_with("ElastOS")));
        assert!(lines.iter().any(|line| line.starts_with("PC2Host")));
        assert!(lines.iter().any(|line| line.starts_with("Next")));
        assert!(!lines.iter().any(|line| line.starts_with("API")));
    }
}
