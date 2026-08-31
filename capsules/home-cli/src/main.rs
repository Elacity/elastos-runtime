use anyhow::{anyhow, Result};
use elastos_guest::runtime::RuntimeClient;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};

mod terminal;
mod text;

use terminal::{
    read_ui_key, stdin_has_input, term_cols, term_rows, wait_for_enter, MouseEvent, TerminalGuard,
    UiKey, LIVE_REFRESH_POLL_MS,
};
#[cfg(test)]
use text::visible_text_width;
use text::{conversation_role_label, fit_line, pad_ansi_line, rule, truncate, wrap_text};

#[cfg(test)]
use terminal::{
    escape_sequence_key, is_escape_sequence_complete, parse_escape_sequence_bytes,
    ESCAPE_SEQUENCE_MAX_BYTES,
};

const DASHBOARD_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
thread_local! {
    static CLIENT: RefCell<RuntimeClient> = RefCell::new(RuntimeClient::new());
}

const COMMAND_CONTRACT_JSON: &str = include_str!("../browser/commands.json");
const TUI_TAB_ROW: u16 = 1;
const DESCRIPTOR_AUTHORITY_COPY: &str = "descriptors are declared capabilities, not grants";
const TUI_FOOTER_TEXT: &str =
    " Keys: Up/Down select  Left/Right/Tab sections  Enter open  r refresh  q/Esc Desktop  ? help";
const TUI_HELP_FOOTER_TEXT: &str = " Keys: ? close help  q/Esc Desktop  Left/Right/Tab sections";
const PEOPLE_TARGET_ID: &str = "people";
const INBOX_TARGET_ID: &str = "inbox";
const INBOX_NOTIFICATION_HANDOFF_ACTION_PREFIX: &str = "inbox-review-notification:";

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
    targets: Vec<HomeTargetStatus>,
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
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ActiveShellStatus {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    candidates: Vec<ActiveShellCandidateStatus>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ActiveShellCandidateStatus {
    name: String,
    #[serde(default)]
    launchable: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct HomeTargetStatus {
    target: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    target_kind: String,
    #[serde(default)]
    viewer: Option<String>,
    #[serde(default)]
    viewer_title: Option<String>,
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
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomParticipantStatus {
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomPendingRequestStatus {
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RoomSessionStatus {
    display_name: String,
    device_label: String,
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
    id: String,
    source_app: String,
    kind: String,
    title: String,
    body: String,
    action_ref: Option<NotificationActionRefStatus>,
    read: bool,
    severity: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotificationActionRefStatus {
    app: String,
    action_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SourceStatus {
    name: String,
    #[serde(default)]
    channel: String,
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
    resource: String,
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
const HOME_ACTION_IDS: &[&str] = &["chat", "room-approve", "room-deny", "room-revoke-all"];

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
    action_id: Option<String>,
    label: String,
    category: &'static str,
    description: String,
    command: String,
    state: String,
    viewer: Option<String>,
    viewer_title: Option<String>,
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
    #[cfg(test)]
    surface: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ControlSpec {
    key: String,
    description: String,
}

const HELP_TAB_COMMANDS: &[&str] = &["home", "inbox", "people", "apps", "system"];
const HELP_CONTROL_COMMANDS: &[&str] = &["refresh", "help", "exit"];
const HELP_ADVANCED_COMMANDS: &[&str] = &["mywebsite", "wallet", "exits", "invoke"];
const HELP_DEBUG_COMMANDS: &[&str] = &["debug"];

include!("runtime_io.rs");
include!("line_views.rs");
include!("tui_state.rs");
include!("tui_render.rs");
include!("view_models.rs");
