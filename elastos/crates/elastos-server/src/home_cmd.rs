use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::sleep;

use elastos_common::localhost::{
    edge_site_head_path, my_website_root_path, publisher_site_releases_dir, ALL_ROOTS,
    DYNAMIC_ROOTS, FILE_BACKED_ROOTS, MY_WEBSITE_URI,
};
use elastos_server::sources::{default_data_dir, load_trusted_sources};

use crate::runtime_control;

const LOBBY_VERSION: &str = env!("ELASTOS_VERSION");
const HOME_CLI_CAPSULE_NAME: &str = "home-cli";
const HOME_TERMINAL_HOST_INTENT_OSC_PREFIX: &str = "\x1b]777;elastos-home-intent=";
const HOME_TERMINAL_HOST_INTENT_OSC_SUFFIX: &str = "\x07";
const HOME_SESSION_ROOT: &str = "Local/SharedByLocalUsersAndBots/Home/sessions";
const COMMAND_GROUPS: &[(&str, &[&str])] = &[
    ("Home", &["home", "chat"]),
    (
        "Spaces",
        &["share", "open", "shares", "attest", "site", "webspace"],
    ),
    (
        "Operators",
        &["serve", "gateway", "agent", "capsule", "run"],
    ),
    (
        "Trust",
        &[
            "source",
            "update",
            "upgrade",
            "publish-release",
            "verify",
            "sign-payload",
        ],
    ),
    ("Setup", &["setup", "keys", "config", "init", "emergency"]),
];

const COMPONENTS: &[(&str, &str)] = &[
    ("localhost-provider", "provider"),
    ("did-provider", "provider"),
    ("webspace-provider", "provider"),
    ("ipfs-provider", "provider"),
    ("site-provider", "provider"),
    ("tunnel-provider", "provider"),
    ("shell", "system capsule"),
    ("cloudflared", "external"),
    ("kubo", "external"),
    ("crosvm", "external"),
    ("vmlinux", "external"),
];

const PLATFORM_LAYERS: &[(&str, &str)] = &[
    (
        "Home",
        "The front door of your sovereign local computer.",
    ),
    (
        "Apps",
        "Things you launch from Home, such as chat, sharing, and site tools.",
    ),
    (
        "ElastOS",
        "The trusted local host that runs apps, services, and capability checks.",
    ),
    (
        "Carrier",
        "The network between ElastOS homes for elastos:// discovery, messaging, and content exchange.",
    ),
    (
        "Home Session",
        "Internal return-home and approval plumbing. Usually invisible to the user.",
    ),
];

const SYSTEM_SERVICES: &[SystemServiceSpec] = &[
    SystemServiceSpec {
        name: "Home Session",
        role: "Keeps Home persistent while launched apps return back here when they exit.",
        backing: &["shell"],
    },
    SystemServiceSpec {
        name: "Local World",
        role: "Provides rooted localhost:// spaces such as Users, UsersAI, Public, and MyWebSite.",
        backing: &["localhost-provider"],
    },
    SystemServiceSpec {
        name: "Identity",
        role: "Provides the DID identity of this Home and signs local identity operations.",
        backing: &["did-provider"],
    },
    SystemServiceSpec {
        name: "WebSpaces",
        role: "Resolves localhost://WebSpaces/<moniker>/... into dynamic typed handles instead of ordinary storage paths.",
        backing: &["webspace-provider"],
    },
    SystemServiceSpec {
        name: "Content Exchange",
        role: "Moves shared content when this Home needs transport or verification.",
        backing: &["ipfs-provider", "kubo"],
    },
    SystemServiceSpec {
        name: "Site Edge",
        role: "Serves localhost://MyWebSite into a browser-facing local edge when you open your site.",
        backing: &["site-provider"],
    },
    SystemServiceSpec {
        name: "Public Edge",
        role: "Gives MyWebSite a temporary public browser URL when you explicitly ask for one.",
        backing: &["tunnel-provider", "cloudflared"],
    },
    SystemServiceSpec {
        name: "Full-screen Apps",
        role: "Supports immersive full-screen app capsules such as packaged chat in microVM and WASM form.",
        backing: &["crosvm", "vmlinux"],
    },
];

/// Core actions are built into the runtime binary. They are always visible.
const CORE_ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "identity-nickname-set",
        label: "Set nickname",
        description: "Set the DID-backed local nickname used by Chat and shown in Home.",
        args: &["identity", "nickname", "set"],
        core: true,
    },
    ActionSpec {
        id: "chat",
        label: "Chat",
        description: "Open native chat, send a message, and return here when you exit.",
        args: &["chat"],
        core: true,
    },
    ActionSpec {
        id: "room-approve",
        label: "Approve web guest",
        description: "Approve the next pending Chat join request from the public link.",
        args: &[],
        core: false,
    },
    ActionSpec {
        id: "room-deny",
        label: "Deny web guest",
        description: "Deny the next pending Chat join request from the public link.",
        args: &[],
        core: false,
    },
    ActionSpec {
        id: "room-revoke-all",
        label: "Disconnect browsers",
        description: "Disconnect all active web guest sessions from Chat.",
        args: &[],
        core: false,
    },
    ActionSpec {
        id: "site-local",
        label: "Preview",
        description: "Start or reuse the local MyWebSite preview without opening a browser.",
        args: &["site", "serve", "--mode", "local"],
        core: true,
    },
    ActionSpec {
        id: "site-ephemeral",
        label: "Publish",
        description: "Start a temporary public URL for MyWebSite without opening a browser.",
        args: &["site", "serve", "--mode", "ephemeral"],
        core: false,
    },
    ActionSpec {
        id: "site-open",
        label: "Open",
        description: "Explicitly open the MyWebSite preview in a browser.",
        args: &[],
        core: false,
    },
    ActionSpec {
        id: "shares-list",
        label: "Shared",
        description: "Open files and folders this Home already shared, then return here.",
        args: &["shares", "list"],
        core: true,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HomeSnapshot {
    version: String,
    user: String,
    nickname: Option<String>,
    did: Option<String>,
    session: HomeCliSessionStatus,
    data_dir: String,
    source: Option<SourceStatus>,
    runtime: RuntimeStatus,
    platform_layers: Vec<PlatformLayer>,
    system_services: Vec<SystemServiceStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    services: Option<serde_json::Value>,
    #[serde(default)]
    active_shell: serde_json::Value,
    #[serde(default)]
    targets: Vec<HomeTargetStatus>,
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
    components: Vec<ComponentStatus>,
    cached_capsules: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capsule_catalog: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capsule_interfaces: Option<serde_json::Value>,
    command_groups: Vec<CommandGroup>,
    actions: Vec<ActionInfo>,
    #[serde(default)]
    notice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HomeCliSessionStatus {
    mode: String,
    passkey_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShareChannelStatus {
    name: String,
    latest_cid: String,
    latest_version: u64,
    status: String,
    #[serde(default)]
    head_cid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    local_runtime_did: Option<String>,
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
    latest_request_name: Option<String>,
    #[serde(default)]
    latest_request_device: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RoomParticipantStatus {
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RoomPendingRequestStatus {
    request_id: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RoomSessionStatus {
    token: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RoomMemberStatus {
    member_did: String,
    role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RoomInviteStatus {
    invite_id: String,
    invited_did: String,
    role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    service_offers: Vec<serde_json::Value>,
    #[serde(default)]
    discovery: PeopleDiscoveryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleContactStatus {
    contact_id: String,
    #[serde(default)]
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    #[serde(default)]
    relationship: String,
    #[serde(default)]
    route: String,
    #[serde(default)]
    can_message: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_card: Option<PeopleProfileCardStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleProfileCardStatus {
    #[serde(default)]
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleDiscoveryStatus {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_seconds: Option<u64>,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    status_message: String,
    #[serde(default)]
    topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_peer_id: Option<String>,
    #[serde(default)]
    discovered_count: usize,
    #[serde(default)]
    discovered_peers: Vec<PeopleDiscoveryPeerStatus>,
    #[serde(default)]
    request_count: usize,
    #[serde(default)]
    requests: Vec<PeopleDiscoveryRequestStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_refresh_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleDiscoveryPeerStatus {
    #[serde(default)]
    peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    #[serde(default)]
    last_seen_at: u64,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleDiscoveryRequestStatus {
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invite_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct HomeSummaryFactsProjection {
    #[serde(default)]
    runtime: Option<RuntimeStatus>,
    #[serde(default)]
    active_shell: Option<serde_json::Value>,
    #[serde(default)]
    people: PeopleStatus,
    #[serde(default)]
    services: Option<serde_json::Value>,
    #[serde(default)]
    notifications: NotificationStatus,
    #[serde(default)]
    capsule_catalog: Option<serde_json::Value>,
    #[serde(default)]
    capsule_interfaces: Option<serde_json::Value>,
    #[serde(default)]
    targets: Vec<HomeTargetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HomeTargetStatus {
    target: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    route: String,
    #[serde(default)]
    attach_kind: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    target_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewer_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotificationStatus {
    #[serde(default)]
    unread_count: usize,
    #[serde(default)]
    attention_count: usize,
    #[serde(default)]
    entries: Vec<NotificationEntryStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotificationEntryStatus {
    id: String,
    source_app: String,
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    action_ref: Option<NotificationActionRefStatus>,
    #[serde(default)]
    read: bool,
    severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotificationActionRefStatus {
    app: String,
    action_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceStatus {
    name: String,
    channel: String,
    installed_version: String,
    gateway: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RuntimeStatus {
    running: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peer_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
    #[serde(default)]
    running_capsules: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlatformLayer {
    name: String,
    role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SystemServiceStatus {
    name: String,
    role: String,
    backing: String,
    state: String,
    ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteStatus {
    staged: bool,
    root_uri: String,
    path: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RootStatus {
    name: String,
    kind: String,
    uri: String,
    path: Option<String>,
    exists: bool,
    description: String,
    example: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComponentStatus {
    name: String,
    kind: String,
    installed: bool,
    available: bool,
    source: String,
    installed_path: String,
    resolved_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandGroup {
    name: String,
    commands: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct SiteHeadPayloadLite {
    #[serde(default)]
    bundle_cid: Option<String>,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct SiteHeadEnvelopeLite {
    #[serde(default)]
    payload: SiteHeadPayloadLite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActionInfo {
    id: String,
    label: String,
    description: String,
    command: String,
    ready: bool,
    reason: Option<String>,
}

#[derive(Clone, Copy)]
struct ActionSpec {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    /// Core actions are always visible. Extended actions are hidden when blocked.
    core: bool,
}

struct SystemServiceSpec {
    name: &'static str,
    role: &'static str,
    backing: &'static [&'static str],
}

#[derive(Debug, Clone)]
enum ActionReadiness {
    Ready,
    Blocked(String),
}

#[derive(Debug, Clone)]
struct HomeSession {
    uri_root: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeIntent {
    action: String,
    #[serde(default)]
    invoke: Option<HomeInvokeIntent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeInvokeIntent {
    capsule: String,
    #[serde(rename = "interface")]
    interface_id: String,
    method: String,
    resource: String,
    #[serde(default)]
    input: serde_json::Value,
}

#[derive(Clone)]
struct SessionAccess {
    client: reqwest::Client,
    api_url: String,
    client_token: String,
    read_cap: String,
    write_cap: String,
}

struct SessionCleanup {
    path: PathBuf,
}

struct ScopedEnvVar {
    name: &'static str,
    previous: Option<OsString>,
}

struct LoggingSuppressionGuard {
    previous: bool,
}

#[derive(Default)]
struct DashboardContext {
    local_site_preview: Option<crate::site_cmd::LocalSitePreviewSession>,
    local_site_url: Option<String>,
    local_site_public_url: Option<String>,
    local_site_tunnel: Option<crate::site_cmd::PublicTunnelSession>,
}

#[derive(Debug, Clone, PartialEq)]
struct PeopleApiAction {
    path: String,
    body: serde_json::Value,
    success_message: &'static str,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl ScopedEnvVar {
    fn set(name: &'static str, value: impl Into<OsString>) -> Self {
        let previous = std::env::var_os(name);
        let value: OsString = value.into();
        std::env::set_var(name, &value);
        Self { name, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.name, value);
        } else {
            std::env::remove_var(self.name);
        }
    }
}

impl LoggingSuppressionGuard {
    fn enter() -> Self {
        Self {
            previous: crate::set_logging_suppressed(true),
        }
    }
}

impl Drop for LoggingSuppressionGuard {
    fn drop(&mut self) {
        crate::set_logging_suppressed(self.previous);
    }
}

impl DashboardContext {
    fn local_site_url(&self) -> Option<&str> {
        self.local_site_url.as_deref()
    }

    async fn shutdown(&mut self) {
        let _ = crate::site_cmd::shutdown_local_site_preview(&mut self.local_site_preview).await;
        self.local_site_url = None;
        let _ = crate::site_cmd::shutdown_public_tunnel(&mut self.local_site_tunnel).await;
        self.local_site_public_url = None;
    }
}

pub(crate) async fn run(status: bool, json: bool) -> anyhow::Result<()> {
    let snapshot = gather_snapshot().await?;

    if json {
        let mut stdout = std::io::stdout().lock();
        let json = serde_json::to_string_pretty(&snapshot)?;
        if let Err(err) = writeln!(stdout, "{}", json) {
            if is_broken_pipe(&err) {
                return Ok(());
            }
            return Err(err.into());
        }
        return Ok(());
    }

    if status {
        if let Err(err) = print_home_state_probe(&snapshot) {
            if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                if is_broken_pipe(io_err) {
                    return Ok(());
                }
            }
            return Err(err);
        }
        return Ok(());
    }

    run_managed_dashboard().await
}

async fn run_managed_dashboard() -> anyhow::Result<()> {
    install_gateway_terminal_parent_watchdog();
    let data_dir = default_data_dir();
    let _logging_guard = LoggingSuppressionGuard::enter();
    let _quiet_runtime_notices = ScopedEnvVar::set("ELASTOS_QUIET_RUNTIME_NOTICES", "1");
    let coords_override = data_dir.join("home-runtime-coords.json");
    std::env::set_var("ELASTOS_RUNTIME_COORDS_FILE", &coords_override);
    let coords = runtime_control::ensure_runtime_for_home(&data_dir).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let tokens = runtime_control::attach_to_runtime(&coords).await?;
    let session = create_session(&data_dir)?;
    let access =
        create_session_access(&client, &coords.api_url, &tokens.client_token, &session).await?;
    let _cleanup = SessionCleanup {
        path: session.path.clone(),
    };
    let mut notice = None;
    let mut dashboard = DashboardContext::default();

    let result = loop {
        let mut snapshot = gather_snapshot_with_site_preview(dashboard.local_site_url()).await?;
        snapshot.notice = notice.take();
        write_snapshot(&access, &session, &snapshot).await?;
        clear_intent(&access, &session).await?;
        let updater_access = access.clone();
        let updater_session = session.clone();
        let updater_notice = snapshot.notice.clone();
        let updater_site_url = dashboard.local_site_url().map(|url| url.to_string());
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let updater = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = sleep(Duration::from_millis(300)) => {}
                }

                let mut next_snapshot =
                    match gather_snapshot_with_site_preview(updater_site_url.as_deref()).await {
                        Ok(snapshot) => snapshot,
                        Err(_) => continue,
                    };
                next_snapshot.notice = updater_notice.clone();
                let _ = write_snapshot(&updater_access, &updater_session, &next_snapshot).await;
            }
        });
        let capsule_result =
            run_home_capsule(&data_dir, &coords.api_url, &tokens.client_token, &session).await;
        let _ = stop_tx.send(true);
        let _ = updater.await;
        capsule_result?;

        let Some(intent) = read_intent(&access, &session).await? else {
            break Ok(());
        };

        match intent.action.as_str() {
            "quit" => {
                if gateway_owned_home_terminal()
                    && emit_gateway_home_terminal_host_intent("shell-switch:home-gui", &snapshot)?
                {
                    break Ok(());
                }
                break Ok(());
            }
            "refresh" => {
                notice = None;
            }
            "invoke" => {
                notice = Some(
                    match dispatch_home_cli_invoke_intent(&access, &data_dir, intent.invoke).await {
                        Ok(message) => message,
                        Err(err) => format!("Invoke failed: {}", err),
                    },
                );
            }
            action_id => {
                if emit_gateway_home_terminal_host_intent(action_id, &snapshot)? {
                    break Ok(());
                }
                notice = Some(
                    match dispatch_action(action_id, &snapshot, &coords, &mut dashboard).await {
                        Ok(message) => {
                            let _ = elastos_server::notifications::mark_acted_for_action(
                                &default_data_dir(),
                                action_id,
                            );
                            message
                        }
                        Err(err) => format!("Action failed: {}", err),
                    },
                );
            }
        }
    };

    dashboard.shutdown().await;

    result
}

fn gateway_owned_home_terminal() -> bool {
    std::env::var_os(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV).as_deref()
        == Some(std::ffi::OsStr::new("1"))
}

fn issue_home_cli_launch_token(data_dir: &Path, app: &str) -> anyhow::Result<String> {
    if let Some(token) =
        elastos_server::api::gateway::issue_gateway_owned_home_cli_launch_token(data_dir, app)?
    {
        return Ok(token);
    }
    elastos_server::api::gateway::issue_local_runtime_home_launch_token(data_dir, app)
}

fn gateway_home_cli_api_url() -> Option<String> {
    if !gateway_owned_home_terminal() {
        return None;
    }
    std::env::var(elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn gather_active_shell_snapshot(data_dir: &Path) -> anyhow::Result<serde_json::Value> {
    if let Some(snapshot) =
        elastos_server::api::gateway::gateway_owned_home_cli_active_shell_snapshot(data_dir)?
    {
        return Ok(snapshot);
    }
    elastos_server::api::gateway::home_active_shell_snapshot(data_dir)
}

#[cfg(unix)]
fn install_gateway_terminal_parent_watchdog() {
    if !gateway_owned_home_terminal() {
        return;
    }
    let parent_pid = unsafe { libc::getppid() };
    if gateway_terminal_parent_lost(parent_pid, parent_pid) {
        std::process::exit(0);
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let current_parent_pid = unsafe { libc::getppid() };
        if gateway_terminal_parent_lost(parent_pid, current_parent_pid)
            || gateway_terminal_parent_missing(parent_pid)
        {
            std::process::exit(0);
        }
    });
}

#[cfg(not(unix))]
fn install_gateway_terminal_parent_watchdog() {}

#[cfg(unix)]
fn gateway_terminal_parent_lost(
    original_parent_pid: libc::pid_t,
    current_parent_pid: libc::pid_t,
) -> bool {
    original_parent_pid <= 1 || current_parent_pid <= 1 || current_parent_pid != original_parent_pid
}

#[cfg(unix)]
fn gateway_terminal_parent_missing(parent_pid: libc::pid_t) -> bool {
    if parent_pid <= 1 {
        return true;
    }
    if unsafe { libc::kill(parent_pid, 0) } == 0 {
        return false;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn home_terminal_host_intent_for_action(
    action_id: &str,
    snapshot: &HomeSnapshot,
) -> Option<serde_json::Value> {
    if action_id == "auth-sign-out" {
        if snapshot.session.mode != "browser_pty" {
            return None;
        }
        return Some(serde_json::json!({
            "schema": "elastos.home.terminal-host-intent/v1",
            "action": "sign-out",
            "action_id": action_id,
            "target": "home",
        }));
    }

    if let Some(contact_id) = action_id.strip_prefix("people-message:").map(str::trim) {
        if contact_id.is_empty() {
            return None;
        }
        let contact = snapshot
            .people
            .contacts
            .iter()
            .find(|contact| contact.contact_id == contact_id)?;
        let target = people_contact_message_target(contact)?;
        if target == HOME_CLI_CAPSULE_NAME {
            return None;
        }
        return Some(serde_json::json!({
            "schema": "elastos.home.terminal-host-intent/v1",
            "action": "open-target",
            "action_id": action_id,
            "target": target,
            "source": "people-contact",
            "contact_id": contact.contact_id.as_str(),
            "route": contact.route.as_str(),
        }));
    }

    if let Some(target) = action_id.strip_prefix("open-gui:").map(str::trim) {
        if target.is_empty() || target == HOME_CLI_CAPSULE_NAME {
            return None;
        }
        if !snapshot
            .actions
            .iter()
            .any(|action| action.ready && action.id == action_id)
        {
            return None;
        }
        return Some(serde_json::json!({
            "schema": "elastos.home.terminal-host-intent/v1",
            "action": "open-target",
            "action_id": action_id,
            "target": target,
        }));
    }

    let target = action_id.strip_prefix("shell-switch:")?.trim();
    if target != "home-gui" {
        return None;
    }
    Some(serde_json::json!({
        "schema": "elastos.home.terminal-host-intent/v1",
        "action": "active-shell",
        "action_id": action_id,
        "target": "home-gui",
    }))
}

fn home_app_target_from_route(route: &str) -> Option<String> {
    let rest = route.trim().strip_prefix("/apps/")?;
    let target = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

fn people_contact_message_target(contact: &PeopleContactStatus) -> Option<String> {
    if !contact.can_message {
        return None;
    }
    home_app_target_from_route(&contact.route)
}

fn emit_gateway_home_terminal_host_intent(
    action_id: &str,
    snapshot: &HomeSnapshot,
) -> anyhow::Result<bool> {
    if !gateway_owned_home_terminal() {
        return Ok(false);
    }
    let Some(intent) = home_terminal_host_intent_for_action(action_id, snapshot) else {
        return Ok(false);
    };
    let payload = serde_json::to_string(&intent)?;
    print!(
        "{}{}{}",
        HOME_TERMINAL_HOST_INTENT_OSC_PREFIX, payload, HOME_TERMINAL_HOST_INTENT_OSC_SUFFIX
    );
    std::io::stdout().flush()?;
    Ok(true)
}

async fn dispatch_home_cli_invoke_intent(
    access: &SessionAccess,
    data_dir: &Path,
    invoke: Option<HomeInvokeIntent>,
) -> anyhow::Result<String> {
    let invoke = invoke.context("invoke intent missing payload")?;
    if invoke.resource.trim().is_empty() {
        anyhow::bail!("invoke intent is missing its Runtime resource binding");
    }
    let token = issue_home_cli_launch_token(data_dir, &invoke.capsule)?;
    let request_id = home_cli_invoke_request_id();
    let principal = home_cli_invoke_principal(data_dir)?;
    let expected_binding = elastos_server::esp_binding::esp_request_binding(
        &request_id,
        &principal,
        &invoke.capsule,
        Some(&invoke.interface_id),
        &invoke.method,
        [invoke.resource.clone()],
        &invoke.input,
    );
    let api_url = gateway_home_cli_api_url().unwrap_or_else(|| access.api_url.clone());
    let url = format!(
        "{}/api/capsules/interfaces/invoke",
        api_url.trim_end_matches('/')
    );
    let response = access
        .client
        .post(url)
        .header("x-elastos-home-token", token)
        .json(&serde_json::json!({
            "request_id": request_id,
            "capsule": invoke.capsule,
            "interface": invoke.interface_id,
            "method": invoke.method,
            "input": invoke.input,
        }))
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|_| {
        serde_json::json!({
            "message": text,
        })
    });
    if !status.is_success() || body.get("status").and_then(|value| value.as_str()) == Some("error")
    {
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .or_else(|| body.get("error").and_then(|value| value.as_str()))
            .unwrap_or("Runtime rejected the invoke request");
        let code = body
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or(status.as_str());
        anyhow::bail!("{} {} {}: {}", invoke.capsule, invoke.method, code, message);
    }
    validate_home_cli_invoke_result(&body, &expected_binding)?;
    Ok(format_home_cli_invoke_notice(&body, &invoke))
}

fn home_cli_invoke_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    format!("home-cli-invoke-{}", hex::encode(bytes))
}

fn home_cli_invoke_principal(data_dir: &Path) -> anyhow::Result<String> {
    if gateway_owned_home_terminal() {
        return std::env::var(elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID_ENV)
            .map(|value| value.trim().to_string())
            .ok()
            .filter(|value| !value.is_empty())
            .context("browser Home CLI principal is unavailable");
    }
    let (_, did) = elastos_identity::load_or_create_did(data_dir)?;
    Ok(elastos_runtime::auth::PrincipalId::device_did(&did)
        .as_str()
        .to_string())
}

fn validate_home_cli_invoke_result(
    body: &serde_json::Value,
    expected: &elastos_server::esp_binding::EspRequestBinding,
) -> anyhow::Result<()> {
    if body.get("schema").and_then(serde_json::Value::as_str)
        != Some("elastos.capsules.invoke-result/v1")
        || body.get("status").and_then(serde_json::Value::as_str) != Some("ok")
    {
        anyhow::bail!("Runtime invoke response is not a successful ESP result");
    }
    let actual = body
        .get("request_binding")
        .cloned()
        .context("Runtime invoke response is missing its exact request binding")?;
    let actual: elastos_server::esp_binding::EspRequestBinding = serde_json::from_value(actual)
        .context("Runtime invoke response has an invalid request binding")?;
    if &actual != expected
        || body.get("request_id").and_then(serde_json::Value::as_str)
            != Some(expected.request_id.as_str())
        || body.get("capsule").and_then(serde_json::Value::as_str)
            != Some(expected.capsule.as_str())
        || body.get("interface").and_then(serde_json::Value::as_str)
            != expected.interface.as_deref()
        || body.get("method").and_then(serde_json::Value::as_str) != Some(expected.method.as_str())
    {
        anyhow::bail!("Runtime invoke response does not match the exact request");
    }
    if body.get("output").is_none() {
        anyhow::bail!("Runtime invoke response is missing its bound output");
    }
    Ok(())
}

fn format_home_cli_invoke_notice(body: &serde_json::Value, invoke: &HomeInvokeIntent) -> String {
    let output = body.get("output").unwrap_or(&serde_json::Value::Null);
    let output_hint = output
        .get("target")
        .and_then(|value| value.as_str())
        .or_else(|| output.get("route").and_then(|value| value.as_str()))
        .map(|value| format!(" -> {value}"))
        .unwrap_or_default();
    format!(
        "invoke: Runtime confirmed {} {}{}",
        invoke.capsule, invoke.method, output_hint
    )
}

async fn gather_snapshot() -> anyhow::Result<HomeSnapshot> {
    gather_snapshot_with_site_preview(None).await
}

async fn gather_snapshot_with_site_preview(
    site_local_url: Option<&str>,
) -> anyhow::Result<HomeSnapshot> {
    let data_dir = default_data_dir();
    let did = load_existing_did(&data_dir);
    let source = load_default_source(&data_dir)?;
    let local_runtime = gather_runtime_status(&data_dir).await;
    let home_summary_projection =
        gather_home_summary_projection(&data_dir, local_runtime.api_url.as_deref()).await?;
    let runtime = home_summary_projection
        .as_ref()
        .and_then(|projection| projection.runtime.clone())
        .unwrap_or(local_runtime);
    let capsule_catalog = home_summary_projection
        .as_ref()
        .and_then(|projection| projection.capsule_catalog.clone())
        .unwrap_or_else(|| elastos_server::api::gateway::capsule_catalog_snapshot(&data_dir));
    let capsule_interfaces = if let Some(interfaces) = home_summary_projection
        .as_ref()
        .and_then(|projection| projection.capsule_interfaces.clone())
    {
        interfaces
    } else {
        gather_capsule_interface_snapshot(&data_dir, runtime.api_url.as_deref()).await?
    };
    let people = home_summary_projection
        .as_ref()
        .map(|projection| projection.people.clone())
        .unwrap_or_default();
    let active_shell = if let Some(active_shell) = home_summary_projection
        .as_ref()
        .and_then(|projection| projection.active_shell.clone())
    {
        active_shell
    } else {
        gather_active_shell_snapshot(&data_dir)?
    };
    let site_root = my_website_root_path(&data_dir);
    let site_head = load_site_head_summary(&data_dir);
    let release_count = count_site_releases(&data_dir);
    let nickname = load_runtime_nickname(&data_dir).await;
    let room_summary = elastos_server::room_service::load_summary(&data_dir).unwrap_or_default();
    let _ = elastos_server::notifications::sync_room_notifications(&data_dir, &room_summary);
    let notification_summary =
        elastos_server::notifications::load_summary(&data_dir).unwrap_or_default();

    let mut snapshot = HomeSnapshot {
        version: LOBBY_VERSION.to_string(),
        user: current_user(),
        nickname,
        did,
        session: gather_home_cli_session_status(),
        data_dir: data_dir.display().to_string(),
        source,
        runtime,
        platform_layers: gather_platform_layers(),
        system_services: Vec::new(),
        services: home_summary_projection
            .as_ref()
            .and_then(|projection| projection.services.clone())
            .or_else(|| {
                Some(elastos_server::api::gateway::home_services_snapshot(
                    &data_dir,
                ))
            }),
        active_shell,
        targets: home_summary_projection
            .as_ref()
            .map(|projection| projection.targets.clone())
            .filter(|targets| !targets.is_empty())
            .unwrap_or_else(|| {
                serde_json::from_value(elastos_server::api::gateway::home_targets_snapshot(
                    &data_dir,
                ))
                .unwrap_or_default()
            }),
        site: SiteStatus {
            staged: site_root.join("index.html").exists(),
            root_uri: "localhost://MyWebSite".to_string(),
            path: site_root.display().to_string(),
            local_url: site_local_url.map(|url| url.to_string()),
            active_release: site_head
                .as_ref()
                .and_then(|head| head.payload.release_name.clone()),
            active_channel: site_head
                .as_ref()
                .and_then(|head| head.payload.channel_name.clone()),
            active_bundle_cid: site_head
                .as_ref()
                .and_then(|head| head.payload.bundle_cid.clone()),
            release_count,
        },
        shares: gather_share_status(),
        room: RoomStatus {
            room_slug: room_summary.room_slug,
            title: room_summary.room_control.title,
            owner_did: room_summary.room_control.owner_did,
            current_key_epoch: room_summary.room_control.current_key_epoch,
            admin_count: room_summary.room_control.admin_count,
            member_count: room_summary.room_control.member_count,
            active_member_count: room_summary.room_control.active_member_count,
            pending_invite_count: room_summary.room_control.pending_invites.len(),
            allow_guest_invites: room_summary.room_control.access_policy.allow_guest_invites,
            allow_member_invites: room_summary.room_control.access_policy.allow_member_invites,
            allow_members_to_host_guests: room_summary
                .room_control
                .access_policy
                .allow_members_to_host_guests,
            local_runtime_did: room_summary.local_runtime_did,
            local_runtime_role: room_summary.local_runtime_role.map(|role| {
                match role {
                    elastos_server::room_service::RoomRole::Owner => "owner",
                    elastos_server::room_service::RoomRole::Admin => "admin",
                    elastos_server::room_service::RoomRole::Member => "member",
                }
                .to_string()
            }),
            canonical_hosted_guest_url: room_summary.canonical_hosted_guest_url,
            ephemeral_hosted_guest_url: room_summary.ephemeral_hosted_guest_url,
            browser_access_allowed: room_summary.browser_access_allowed,
            browser_access_block_reason: room_summary.browser_access_block_reason,
            pending_count: room_summary.pending_count,
            active_session_count: room_summary.active_session_count,
            latest_request_name: room_summary.latest_request_name,
            latest_request_device: room_summary.latest_request_device,
            active_participants: room_summary
                .active_participants
                .into_iter()
                .map(|participant| RoomParticipantStatus {
                    display_name: participant.display_name,
                    device_label: participant.device_label,
                })
                .collect(),
            pending_requests: room_summary
                .pending_requests
                .into_iter()
                .map(|request| RoomPendingRequestStatus {
                    request_id: request.request_id,
                    display_name: request.display_name,
                    device_label: request.device_label,
                })
                .collect(),
            active_sessions: room_summary
                .active_sessions
                .into_iter()
                .map(|session| RoomSessionStatus {
                    token: session.token,
                    display_name: session.display_name,
                    device_label: session.device_label,
                })
                .collect(),
            members: room_summary
                .room_control
                .members
                .into_iter()
                .map(|member| RoomMemberStatus {
                    member_did: member.member_did,
                    role: match member.role {
                        elastos_server::room_service::RoomRole::Owner => "owner",
                        elastos_server::room_service::RoomRole::Admin => "admin",
                        elastos_server::room_service::RoomRole::Member => "member",
                    }
                    .to_string(),
                })
                .collect(),
            pending_invites: room_summary
                .room_control
                .pending_invites
                .into_iter()
                .map(|invite| RoomInviteStatus {
                    invite_id: invite.invite_id,
                    invited_did: invite.invited_did,
                    role: match invite.role {
                        elastos_server::room_service::RoomRole::Owner => "owner",
                        elastos_server::room_service::RoomRole::Admin => "admin",
                        elastos_server::room_service::RoomRole::Member => "member",
                    }
                    .to_string(),
                })
                .collect(),
        },
        people,
        notifications: home_summary_projection
            .as_ref()
            .map(|projection| projection.notifications.clone())
            .unwrap_or_else(|| notification_status_from_summary(notification_summary)),
        roots: gather_roots(&data_dir),
        components: gather_components(&data_dir),
        cached_capsules: gather_cached_capsules(&data_dir),
        capsule_catalog: Some(capsule_catalog),
        capsule_interfaces: Some(capsule_interfaces),
        command_groups: COMMAND_GROUPS
            .iter()
            .map(|(name, commands)| CommandGroup {
                name: (*name).to_string(),
                commands: commands.iter().map(|cmd| (*cmd).to_string()).collect(),
            })
            .collect(),
        actions: Vec::new(),
        notice: None,
    };

    snapshot.system_services = gather_system_services(&snapshot.components);

    // Core + site actions from the hardcoded list.
    snapshot.actions = CORE_ACTIONS
        .iter()
        .filter_map(|action| {
            let readiness = action_readiness(action.id, &snapshot);
            // Hide non-core actions when their prerequisites are not installed.
            if !action.core && matches!(readiness, ActionReadiness::Blocked(_)) {
                return None;
            }
            Some(ActionInfo {
                id: action.id.to_string(),
                label: action.label.to_string(),
                description: action.description.to_string(),
                command: action_command(*action, &snapshot),
                ready: matches!(readiness, ActionReadiness::Ready),
                reason: match readiness {
                    ActionReadiness::Ready => None,
                    ActionReadiness::Blocked(reason) => Some(reason),
                },
            })
        })
        .collect();

    // The catalog projection is the only source for dynamic Home CLI actions.
    if let Some(catalog) = snapshot.capsule_catalog.as_ref() {
        snapshot.actions.extend(gather_capsule_actions(catalog));
    }
    snapshot.actions.extend(gather_room_actions(&snapshot));
    snapshot
        .actions
        .extend(gather_notification_host_actions(&snapshot));

    Ok(snapshot)
}

fn notification_status_from_summary(
    summary: elastos_server::notifications::NotificationSummary,
) -> NotificationStatus {
    NotificationStatus {
        unread_count: summary.unread_count,
        attention_count: summary.attention_count,
        entries: summary
            .entries
            .into_iter()
            .map(|entry| NotificationEntryStatus {
                id: entry.id,
                source_app: entry.source_app,
                kind: entry.kind,
                title: entry.title,
                body: entry.body,
                action_ref: entry
                    .action_ref
                    .map(|action_ref| NotificationActionRefStatus {
                        app: action_ref.app,
                        action_id: action_ref.action_id,
                    }),
                read: entry.read,
                severity: match entry.severity {
                    elastos_server::notifications::NotificationSeverity::Info => "info",
                    elastos_server::notifications::NotificationSeverity::Attention => "attention",
                    elastos_server::notifications::NotificationSeverity::Critical => "critical",
                }
                .to_string(),
            })
            .collect(),
    }
}

fn gather_notification_host_actions(snapshot: &HomeSnapshot) -> Vec<ActionInfo> {
    let mut actions = Vec::new();
    for entry in &snapshot.notifications.entries {
        let Some(action_ref) = entry.action_ref.as_ref() else {
            continue;
        };
        let action_id = action_ref.action_id.trim();
        let Some(target) = action_id.strip_prefix("open-gui:").map(str::trim) else {
            continue;
        };
        if target.is_empty()
            || target == HOME_CLI_CAPSULE_NAME
            || snapshot.actions.iter().any(|action| action.id == action_id)
            || actions
                .iter()
                .any(|action: &ActionInfo| action.id == action_id)
        {
            continue;
        }
        let title = home_target_title(target);
        actions.push(ActionInfo {
            id: action_id.to_string(),
            label: format!("Open {title}"),
            description: format!("Open {title} to handle this trusted Home notification."),
            command: format!("home: open {title}"),
            ready: true,
            reason: None,
        });
    }
    actions
}

fn home_target_title(target: &str) -> String {
    target
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn gather_home_cli_session_status() -> HomeCliSessionStatus {
    if gateway_owned_home_terminal() {
        let data_dir = default_data_dir();
        let passkey_state =
            if elastos_server::api::gateway::gateway_owned_home_cli_authority_available(&data_dir) {
                "launch-token authorized browser Home session"
            } else {
                "browser Home terminal missing launch authority"
            };
        HomeCliSessionStatus {
            mode: "browser_pty".to_string(),
            passkey_state: passkey_state.to_string(),
        }
    } else {
        HomeCliSessionStatus {
            mode: "native_terminal".to_string(),
            passkey_state: "local operator session; no browser passkey active".to_string(),
        }
    }
}

fn load_site_head_summary(data_dir: &Path) -> Option<SiteHeadEnvelopeLite> {
    let path = edge_site_head_path(data_dir, MY_WEBSITE_URI);
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn count_site_releases(data_dir: &Path) -> usize {
    let dir = publisher_site_releases_dir(data_dir, MY_WEBSITE_URI);
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count()
}

fn create_session(data_dir: &Path) -> anyhow::Result<HomeSession> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let id = format!("{}-{}", std::process::id(), stamp);
    let local_root = format!("{}/{}", HOME_SESSION_ROOT, id);
    let uri_root = format!("localhost://{}", local_root);
    let path = data_dir
        .join("Local")
        .join("Shared")
        .join("Home")
        .join("sessions")
        .join(&id);
    fs::create_dir_all(&path)?;

    Ok(HomeSession { uri_root, path })
}

async fn create_session_access(
    client: &reqwest::Client,
    api_url: &str,
    client_token: &str,
    session: &HomeSession,
) -> anyhow::Result<SessionAccess> {
    let session_scope = format!("{}/*", session.uri_root.trim_end_matches('/'));
    let read_cap =
        crate::request_attached_capability(client, api_url, client_token, &session_scope, "read")
            .await?;
    let write_cap =
        crate::request_attached_capability(client, api_url, client_token, &session_scope, "write")
            .await?;

    Ok(SessionAccess {
        client: client.clone(),
        api_url: api_url.to_string(),
        client_token: client_token.to_string(),
        read_cap,
        write_cap,
    })
}

async fn write_snapshot(
    access: &SessionAccess,
    session: &HomeSession,
    snapshot: &HomeSnapshot,
) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(snapshot)?;
    write_localhost_file(
        access,
        &format!("{}/snapshot.json", session.uri_root.trim_end_matches('/')),
        data,
    )
    .await?;
    Ok(())
}

async fn clear_intent(access: &SessionAccess, session: &HomeSession) -> anyhow::Result<()> {
    let path = format!("{}/intent.json", session.uri_root.trim_end_matches('/'));
    if !localhost_exists(access, &path).await? {
        return Ok(());
    }
    delete_localhost_file(access, &path).await
}

async fn read_intent(
    access: &SessionAccess,
    session: &HomeSession,
) -> anyhow::Result<Option<HomeIntent>> {
    let path = format!("{}/intent.json", session.uri_root.trim_end_matches('/'));
    if !localhost_exists(access, &path).await? {
        return Ok(None);
    }
    let data = read_localhost_file(access, &path).await?;
    let intent: HomeIntent = serde_json::from_slice(&data)?;
    Ok(Some(intent))
}

async fn run_home_capsule(
    data_dir: &Path,
    api_url: &str,
    client_token: &str,
    session: &HomeSession,
) -> anyhow::Result<()> {
    let capsule_dir = resolve_home_capsule_dir(data_dir)?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_slice(
        &fs::read(capsule_dir.join("capsule.json")).with_context(|| {
            format!(
                "failed to read Home capsule manifest from {}",
                capsule_dir.display()
            )
        })?,
    )
    .context("failed to parse Home capsule manifest")?;
    let mut manifest_capabilities = manifest.resource_authority_bounds();
    manifest_capabilities.push(format!("{}/*", session.uri_root.trim_end_matches('/')));
    let runtime_storage = data_dir
        .join("Local")
        .join("Shared")
        .join("Home")
        .join("bootstrap-storage");
    fs::create_dir_all(&runtime_storage)?;

    let runtime = crate::create_runtime(&runtime_storage).await?;
    let api_url = api_url.to_string();
    let client_token = client_token.to_string();

    let api_hostcall_url = api_url.clone();
    let api_hostcall_token = client_token.clone();
    let api_hostcall_manifest_capabilities = manifest_capabilities.clone();
    let api_hostcall_data_dir = data_dir.to_path_buf();
    let api_hostcall_handle = tokio::runtime::Handle::current();
    runtime.set_bridge_hostcall(std::sync::Arc::new(
        move |line, capsule_id, principal_id| {
            let response = api_hostcall_handle
                .block_on(
                    elastos_server::carrier_bridge::handle_remote_request_with_audit_dir(
                        line,
                        &api_hostcall_url,
                        &api_hostcall_token,
                        capsule_id,
                        &api_hostcall_manifest_capabilities,
                        principal_id,
                        Some(api_hostcall_data_dir.as_path()),
                    ),
                )
                .map_err(|err| err.to_string())?;
            serde_json::to_string(&response).map_err(|err| err.to_string())
        },
    ));

    runtime
        .run_local(&capsule_dir, vec![session.uri_root.clone()])
        .await
        .map_err(|e| anyhow::anyhow!("Home CLI component descriptor failed: {}", e))?;

    run_home_cli_renderer(data_dir, &api_url, &client_token, session)?;
    Ok(())
}

fn run_home_cli_renderer(
    data_dir: &Path,
    api_url: &str,
    client_token: &str,
    session: &HomeSession,
) -> anyhow::Result<()> {
    let renderer = resolve_home_cli_renderer_program(data_dir)?;
    let status = Command::new(&renderer)
        .arg(&session.uri_root)
        .env("ELASTOS_API", api_url)
        .env("ELASTOS_TOKEN", client_token)
        .env_remove("ELASTOS_CARRIER_PATH")
        .status()
        .with_context(|| format!("failed to start Home CLI renderer {}", renderer.display()))?;
    if !status.success() {
        anyhow::bail!("Home CLI renderer exited with {}", status);
    }
    Ok(())
}

fn resolve_home_cli_renderer_program(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let installed = data_dir.join("bin").join(HOME_CLI_CAPSULE_NAME);
    if installed.is_file() {
        return Ok(installed);
    }

    let dev = source_capsule_dir(HOME_CLI_CAPSULE_NAME)
        .join("target")
        .join("release")
        .join(HOME_CLI_CAPSULE_NAME);
    if dev.is_file() {
        return Ok(dev);
    }

    anyhow::bail!(
        "Home CLI native renderer missing.\n\nBuild and install source Home first:\n\n  scripts/setup-source-home.sh\n\nOr build it directly:\n\n  cargo build --manifest-path capsules/home-cli/Cargo.toml --release --bin home-cli"
    );
}

fn resolve_home_capsule_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let dev = source_capsule_dir(HOME_CLI_CAPSULE_NAME);
    if prefer_dev_home_capsule() && capsule_dir_has_entrypoint(&dev)? {
        return Ok(dev);
    }

    let installed = data_dir.join("capsules").join(HOME_CLI_CAPSULE_NAME);
    if capsule_dir_has_entrypoint(&installed)? {
        return Ok(installed);
    }

    if capsule_dir_has_entrypoint(&dev)? {
        return Ok(dev);
    }

    if prefer_dev_home_capsule() {
        anyhow::bail!(
            "home capsule component not built yet.\n\nBuild and install source Home first:\n\n  scripts/setup-source-home.sh\n\nOr build the Home CLI component artifact directly:\n\n  cd {}\n  cargo build --lib --target wasm32-unknown-unknown --release\n  cd ../..\n  cargo run --quiet --manifest-path elastos/tools/componentize/Cargo.toml -- capsules/home-cli/target/wasm32-unknown-unknown/release/home_cli.wasm capsules/home-cli/home-cli.component.wasm\n\nOr install the published Home surface with:\n\n  elastos setup",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../capsules")
                .join(HOME_CLI_CAPSULE_NAME)
                .display()
        );
    }

    anyhow::bail!("Home is not installed yet.\n\nRun:\n\n  elastos setup");
}

fn capsule_dir_has_entrypoint(dir: &Path) -> anyhow::Result<bool> {
    let manifest_path = dir.join("capsule.json");
    if !manifest_path.is_file() {
        return Ok(false);
    }
    let manifest: elastos_common::CapsuleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).with_context(|| {
            format!(
                "failed to read Home capsule manifest {}",
                manifest_path.display()
            )
        })?)
        .with_context(|| {
            format!(
                "failed to parse Home capsule manifest {}",
                manifest_path.display()
            )
        })?;
    manifest
        .validate()
        .map_err(|err| anyhow::anyhow!("invalid {}: {}", manifest_path.display(), err))?;
    Ok(dir.join(&manifest.entrypoint).is_file())
}

fn source_capsule_dir(capsule_name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../capsules")
        .join(capsule_name)
}

fn prefer_dev_home_capsule() -> bool {
    std::env::current_exe()
        .ok()
        .map(|path| {
            path.components()
                .any(|component| component.as_os_str() == "target")
        })
        .unwrap_or(false)
}

async fn dispatch_action(
    action_id: &str,
    snapshot: &HomeSnapshot,
    coords: &runtime_control::RuntimeCoords,
    dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    // Handle dynamically discovered capsule actions.
    if let Some(capsule_name) = action_id.strip_prefix("capsule-") {
        return dispatch_capsule_action(capsule_name, snapshot, dashboard).await;
    }
    if action_id == "shell-switch:home-gui" {
        return Ok(
            "Shell switch needs the browser Home host. Open Home in a browser, then run `system shell home-gui` from Home CLI."
                .to_string(),
        );
    }
    if action_id.starts_with("people-") {
        return dispatch_people_action(action_id, snapshot, coords, dashboard).await;
    }
    if let Some(notification_id) = action_id.strip_prefix("notification-read:") {
        return match elastos_server::notifications::mark_read(&default_data_dir(), notification_id)?
        {
            true => Ok("Marked inbox entry read.".to_string()),
            false => Ok("That inbox entry was already read or is no longer present.".to_string()),
        };
    }
    if let Some(notification_id) = action_id.strip_prefix("notification-dismiss:") {
        return match elastos_server::notifications::dismiss(&default_data_dir(), notification_id)? {
            true => Ok("Dismissed inbox entry.".to_string()),
            false => Ok("That inbox entry is already gone.".to_string()),
        };
    }
    if let Some(request_id) = action_id.strip_prefix("room-approve-request:") {
        return match elastos_server::room_service::approve_request(&default_data_dir(), request_id)?
        {
            Some(outcome) => Ok(format!(
                "Approved Chat web guest access for {} on {}.",
                outcome.display_name, outcome.device_label
            )),
            None => Ok("That Chat web guest request is no longer pending.".to_string()),
        };
    }
    if let Some(request_id) = action_id.strip_prefix("room-deny-request:") {
        return match elastos_server::room_service::deny_request(
            &default_data_dir(),
            request_id,
            "Denied in Home",
        )? {
            Some(outcome) => Ok(format!(
                "Denied Chat web guest access for {} on {}.",
                outcome.display_name, outcome.device_label
            )),
            None => Ok("That Chat web guest request is no longer pending.".to_string()),
        };
    }
    if let Some(token) = action_id.strip_prefix("room-revoke-session:") {
        return match elastos_server::room_service::revoke_session(&default_data_dir(), token)? {
            Some(outcome) => Ok(format!(
                "Disconnected Chat web guest session for {} on {}.",
                outcome.display_name, outcome.device_label
            )),
            None => Ok("That Chat web guest session is already gone.".to_string()),
        };
    }
    if let Some(invite_id) = action_id.strip_prefix("room-accept-invite:") {
        let actor_did = snapshot.room.local_runtime_did.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "local ElastOS identity is not available for ElastOS user invite acceptance"
            )
        })?;
        let member = elastos_server::room_service::accept_room_invite(
            &default_data_dir(),
            elastos_server::room_service::RoomInviteAcceptInput {
                actor_did,
                invite_id: invite_id.to_string(),
            },
        )?;
        return Ok(format!("Joined Chat as {}.", member.member_did));
    }
    if let Some(invite_id) = action_id.strip_prefix("room-revoke-invite:") {
        let actor_did = require_room_admin_actor(snapshot)?;
        return match elastos_server::room_service::revoke_room_invite(
            &default_data_dir(),
            &actor_did,
            invite_id,
        )? {
            Some(invite) => Ok(format!(
                "Canceled ElastOS user invite for {}.",
                invite.invited_did
            )),
            None => Ok("That ElastOS user invite is already gone.".to_string()),
        };
    }
    if let Some(member_did) = action_id.strip_prefix("room-remove-member:") {
        let actor_did = require_room_admin_actor(snapshot)?;
        return match elastos_server::room_service::remove_room_member(
            &default_data_dir(),
            elastos_server::room_service::RoomMemberRemoveInput {
                actor_did,
                member_did: member_did.to_string(),
            },
        )? {
            Some(member) => Ok(format!(
                "Removed trusted participant {} from Chat.",
                member.member_did
            )),
            None => Ok("That trusted participant is already gone.".to_string()),
        };
    }
    if let Some(source) = action_id.strip_prefix("site-stage:") {
        let source = source.trim();
        if source.is_empty() {
            anyhow::bail!("MyWebSite stage needs a directory path");
        }
        let source_path = PathBuf::from(source);
        crate::site_cmd::stage_site_from_home(&source_path)?;
        return Ok(format!(
            "Staged MyWebSite from {}. Next: run `mywebsite preview`.",
            source_path.display()
        ));
    }
    if action_id == "room-policy-toggle-guests" {
        let actor_did = require_room_admin_actor(snapshot)?;
        let updated = elastos_server::room_service::update_room_access_policy(
            &default_data_dir(),
            elastos_server::room_service::RoomAccessPolicyUpdateInput {
                actor_did,
                allow_guest_invites: !snapshot.room.allow_guest_invites,
                allow_member_invites: snapshot.room.allow_member_invites,
                allow_members_to_host_guests: snapshot.room.allow_members_to_host_guests,
            },
        )?;
        return Ok(if updated.allow_guest_invites {
            "Opened public Chat join requests.".to_string()
        } else {
            "Closed public Chat join requests.".to_string()
        });
    }
    if action_id == "room-policy-toggle-members" {
        let actor_did = require_room_admin_actor(snapshot)?;
        let updated = elastos_server::room_service::update_room_access_policy(
            &default_data_dir(),
            elastos_server::room_service::RoomAccessPolicyUpdateInput {
                actor_did,
                allow_guest_invites: snapshot.room.allow_guest_invites,
                allow_member_invites: !snapshot.room.allow_member_invites,
                allow_members_to_host_guests: snapshot.room.allow_members_to_host_guests,
            },
        )?;
        return Ok(if updated.allow_member_invites {
            "Opened ElastOS user invites for Chat.".to_string()
        } else {
            "Closed ElastOS user invites for Chat.".to_string()
        });
    }
    if action_id == "room-policy-toggle-member-hosts" {
        let actor_did = require_room_admin_actor(snapshot)?;
        let updated = elastos_server::room_service::update_room_access_policy(
            &default_data_dir(),
            elastos_server::room_service::RoomAccessPolicyUpdateInput {
                actor_did,
                allow_guest_invites: snapshot.room.allow_guest_invites,
                allow_member_invites: snapshot.room.allow_member_invites,
                allow_members_to_host_guests: !snapshot.room.allow_members_to_host_guests,
            },
        )?;
        return Ok(if updated.allow_members_to_host_guests {
            "Allowed trusted ElastOS users to approve web guests.".to_string()
        } else {
            "Restricted web guest approvals to conversation managers.".to_string()
        });
    }

    let Some(action) = action_spec(action_id) else {
        anyhow::bail!("Unknown Home action: {}", action_id);
    };

    match action_readiness(action_id, snapshot) {
        ActionReadiness::Ready => run_action(action, snapshot, coords, dashboard).await,
        ActionReadiness::Blocked(reason) => match action_id {
            "site-local" | "site-open" => Ok(render_site_local_blocked_notice(snapshot, &reason)),
            "site-ephemeral" => Ok(render_site_public_blocked_notice(snapshot, &reason)),
            _ => Ok(format!("{} unavailable: {}", action.label, reason)),
        },
    }
}

async fn dispatch_people_action(
    action_id: &str,
    snapshot: &HomeSnapshot,
    coords: &runtime_control::RuntimeCoords,
    dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    if let Some(action) = people_api_action(action_id) {
        let action = action?;
        people_api_post(coords, &action.path, action.body).await?;
        return Ok(action.success_message.to_string());
    }
    if let Some(contact_id) = action_id.strip_prefix("people-message:") {
        let contact = snapshot
            .people
            .contacts
            .iter()
            .find(|contact| contact.contact_id == contact_id)
            .ok_or_else(|| anyhow::anyhow!("People contact is no longer available"))?;
        if !contact.can_message {
            anyhow::bail!("People contact is not message-ready yet");
        }
        if people_contact_message_target(contact).is_none() {
            anyhow::bail!("People contact has no message route in the current Runtime facts");
        }
        let Some(action) = action_spec("chat") else {
            anyhow::bail!("Chat action is not available");
        };
        run_action(action, snapshot, coords, dashboard).await?;
        let label = people_contact_display_name(contact, "contact");
        return Ok(format!("Returned home from Chat with {label}."));
    }
    anyhow::bail!("Unknown People action: {}", action_id)
}

fn people_api_action(action_id: &str) -> Option<anyhow::Result<PeopleApiAction>> {
    if action_id == "people-discovery-enable" {
        return Some(Ok(PeopleApiAction {
            path: "/api/apps/people/discovery".to_string(),
            body: serde_json::json!({ "enabled": true }),
            success_message: "People discovery is on.",
        }));
    }
    if action_id == "people-discovery-disable" {
        return Some(Ok(PeopleApiAction {
            path: "/api/apps/people/discovery".to_string(),
            body: serde_json::json!({ "enabled": false }),
            success_message: "People discovery is off.",
        }));
    }
    if action_id == "people-discovery-refresh" {
        return Some(Ok(PeopleApiAction {
            path: "/api/apps/people/discovery/refresh".to_string(),
            body: serde_json::json!({}),
            success_message: "People discovery refreshed.",
        }));
    }
    if let Some(peer_id) = action_id.strip_prefix("people-request-peer:") {
        if peer_id.trim().is_empty() {
            return Some(Err(anyhow::anyhow!("People peer id is missing")));
        }
        return Some(Ok(PeopleApiAction {
            path: "/api/apps/people/discovery/requests".to_string(),
            body: serde_json::json!({ "peer_id": peer_id }),
            success_message: "People request sent.",
        }));
    }
    if let Some(request_id) = action_id.strip_prefix("people-accept-request:") {
        if request_id.trim().is_empty() {
            return Some(Err(anyhow::anyhow!("People request id is missing")));
        }
        return Some(Ok(PeopleApiAction {
            path: format!(
                "/api/apps/people/discovery/requests/{}/accept",
                percent_encode_path_segment(request_id)
            ),
            body: serde_json::json!({}),
            success_message: "People request accepted.",
        }));
    }
    if let Some(contact_id) = action_id.strip_prefix("people-remove-contact:") {
        if contact_id.trim().is_empty() {
            return Some(Err(anyhow::anyhow!("People contact id is missing")));
        }
        return Some(Ok(PeopleApiAction {
            path: "/api/apps/people/contacts/remove".to_string(),
            body: serde_json::json!({ "contact_id": contact_id }),
            success_message: "Removed from People.",
        }));
    }
    None
}

async fn people_api_post(
    coords: &runtime_control::RuntimeCoords,
    path: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let data_dir = default_data_dir();
    let token = issue_home_cli_launch_token(&data_dir, HOME_CLI_CAPSULE_NAME)?;
    let api_url = gateway_home_cli_api_url().unwrap_or_else(|| coords.api_url.clone());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!(
        "{}/{}",
        api_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let response = client
        .post(url)
        .header("x-elastos-home-token", token)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "Runtime People route failed with {}: {}",
            status.as_u16(),
            text
        );
    }
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "message": text })))
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

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn dispatch_capsule_action(
    capsule_name: &str,
    snapshot: &HomeSnapshot,
    dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    if capsule_name == HOME_CLI_CAPSULE_NAME {
        return Ok("Home CLI is already active.".to_string());
    }
    let Some(capsule) = snapshot
        .capsule_catalog
        .as_ref()
        .and_then(|catalog| capsule_catalog_entry(catalog, capsule_name))
    else {
        anyhow::bail!("Capsule {} is not installed.", capsule_name);
    };
    if !catalog_capsule_has_cli_projection(capsule) {
        anyhow::bail!("Capsule {} is not a Home CLI launch action.", capsule_name);
    }
    run_capsule_action(capsule_name, dashboard).await
}

fn action_spec(action_id: &str) -> Option<ActionSpec> {
    CORE_ACTIONS
        .iter()
        .copied()
        .find(|action| action.id == action_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionLaunch {
    External(&'static [&'static str]),
    ManagedIdentityNicknameSet,
    ManagedChat,
    ManagedRoomApprove,
    ManagedRoomDeny,
    ManagedRoomRevokeAll,
    ManagedLocalSitePreview,
    ManagedLocalSiteOpen,
    ManagedPublicSitePublish,
    ManagedSharesList,
}

async fn run_action(
    action: ActionSpec,
    snapshot: &HomeSnapshot,
    coords: &runtime_control::RuntimeCoords,
    dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    match action_launch(action, snapshot) {
        ActionLaunch::ManagedIdentityNicknameSet => {
            let nickname =
                crate::identity_cmd::set_local_nickname(&default_data_dir(), None).await?;
            Ok(format!(
                "Saved DID nickname as '{}'. You are back at Home.",
                nickname
            ))
        }
        ActionLaunch::ManagedChat => {
            let _parent_surface = ScopedEnvVar::set("ELASTOS_PARENT_SURFACE", "home");
            crate::chat_cmd::run_chat_from_home(None, None, coords.clone()).await?;
            Ok(format!("Returned home from {}.", action.label))
        }
        ActionLaunch::ManagedRoomApprove => {
            match elastos_server::room_service::approve_next_request(&default_data_dir())? {
                Some(outcome) => Ok(format!(
                    "Approved Chat web guest access for {} on {}.",
                    outcome.display_name, outcome.device_label
                )),
                None => Ok("No pending Chat web guest requests.".to_string()),
            }
        }
        ActionLaunch::ManagedRoomDeny => {
            match elastos_server::room_service::deny_next_request(
                &default_data_dir(),
                "Denied in Home",
            )? {
                Some(outcome) => Ok(format!(
                    "Denied Chat web guest access for {} on {}.",
                    outcome.display_name, outcome.device_label
                )),
                None => Ok("No pending Chat web guest requests.".to_string()),
            }
        }
        ActionLaunch::ManagedRoomRevokeAll => {
            match elastos_server::room_service::revoke_all_sessions(&default_data_dir())? {
                Some(outcome) => {
                    let detail = outcome
                        .revoked_participants
                        .iter()
                        .map(|participant| {
                            format!(
                                "{} on {}",
                                participant.display_name, participant.device_label
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!(
                        "Disconnected {} Chat web guest session(s): {}.",
                        outcome.revoked_count, detail
                    ))
                }
                None => Ok("No active Chat web guest sessions.".to_string()),
            }
        }
        ActionLaunch::ManagedLocalSitePreview => {
            let addr = crate::choose_local_open_addr(None)?;
            let status = crate::site_cmd::ensure_local_site_preview(
                &mut dashboard.local_site_preview,
                &addr,
            )
            .await?;
            let local_url = status
                .local_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("site-provider start response missing local_url"))?;
            dashboard.local_site_url = Some(local_url.clone());
            let local_url = local_url.trim_end_matches('/').to_string();
            Ok(render_site_preview_notice(
                &snapshot.site,
                &local_url,
                status.reused.unwrap_or(false),
            ))
        }
        ActionLaunch::ManagedLocalSiteOpen => {
            let addr = crate::choose_local_open_addr(None)?;
            let status = crate::site_cmd::ensure_local_site_preview(
                &mut dashboard.local_site_preview,
                &addr,
            )
            .await?;
            let local_url = status
                .local_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("site-provider start response missing local_url"))?;
            dashboard.local_site_url = Some(local_url.clone());
            crate::open_browser(&local_url);
            Ok(format!(
                "Opened MyWebSite preview at {}.",
                local_url.trim_end_matches('/')
            ))
        }
        ActionLaunch::ManagedPublicSitePublish => {
            let addr = crate::choose_local_open_addr(None)?;
            let status = crate::site_cmd::ensure_local_site_preview(
                &mut dashboard.local_site_preview,
                &addr,
            )
            .await?;
            let local_url = status
                .local_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("site-provider start response missing local_url"))?;
            dashboard.local_site_url = Some(local_url.clone());
            match crate::site_cmd::ensure_public_tunnel(
                &mut dashboard.local_site_tunnel,
                &local_url,
                20,
            )
            .await
            {
                Ok(public_url) => {
                    dashboard.local_site_public_url = Some(public_url.clone());
                    Ok(render_site_public_notice(
                        &snapshot.site,
                        &local_url,
                        &public_url,
                    ))
                }
                Err(err) => {
                    dashboard.local_site_public_url = None;
                    Ok(format!(
                        "MyWebSite public URL setup failed ({}). Local preview remains at {}.",
                        err,
                        local_url.trim_end_matches('/'),
                    ))
                }
            }
        }
        ActionLaunch::ManagedSharesList => Ok(render_share_notice(&snapshot.shares)),
        ActionLaunch::External(args) => {
            let exe = std::env::current_exe().context("current exe unavailable")?;
            let status = Command::new(exe)
                .args(args)
                .env("ELASTOS_PARENT_SURFACE", "home")
                .status()?;
            let exit = status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            if status.success() {
                Ok(format!("Returned home from {}.", action.label))
            } else {
                Ok(format!(
                    "{} ended with exit {}. You are back at Home.",
                    action.label, exit
                ))
            }
        }
    }
}

/// Launch a dynamically discovered capsule by name.
async fn run_capsule_action(
    capsule_name: &str,
    _dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("current exe unavailable")?;
    let status = Command::new(exe)
        .args([
            "capsule",
            capsule_name,
            "--lifecycle",
            "interactive",
            "--interactive",
        ])
        .env("ELASTOS_PARENT_SURFACE", "home")
        .status()?;
    if status.success() {
        Ok(format!("Returned home from {}.", capsule_name))
    } else {
        let exit = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        Ok(format!(
            "{} ended with exit {}. You are back at Home.",
            capsule_name, exit
        ))
    }
}

fn capsule_catalog_entry<'a>(
    catalog: &'a serde_json::Value,
    capsule_name: &str,
) -> Option<&'a serde_json::Value> {
    catalog
        .get("capsules")?
        .as_array()?
        .iter()
        .find(|capsule| capsule.get("name").and_then(|name| name.as_str()) == Some(capsule_name))
}

fn catalog_capsule_has_cli_projection(capsule: &serde_json::Value) -> bool {
    matches!(
        capsule.get("role").and_then(|role| role.as_str()),
        Some("app" | "viewer" | "content")
    ) && capsule
        .get("projection")
        .and_then(|projection| projection.get("cli"))
        .and_then(|cli| cli.get("state"))
        .and_then(|state| state.as_str())
        == Some("available")
}

fn action_launch(action: ActionSpec, snapshot: &HomeSnapshot) -> ActionLaunch {
    action_launch_with_kvm(action, snapshot, elastos_crosvm::is_supported())
}

fn action_launch_with_kvm(
    action: ActionSpec,
    snapshot: &HomeSnapshot,
    kvm_supported: bool,
) -> ActionLaunch {
    if action.id == "identity-nickname-set" {
        ActionLaunch::ManagedIdentityNicknameSet
    } else if action.id == "chat" {
        let _ = (snapshot, kvm_supported);
        ActionLaunch::ManagedChat
    } else if action.id == "room-approve" {
        let _ = (snapshot, kvm_supported);
        ActionLaunch::ManagedRoomApprove
    } else if action.id == "room-deny" {
        let _ = (snapshot, kvm_supported);
        ActionLaunch::ManagedRoomDeny
    } else if action.id == "room-revoke-all" {
        let _ = (snapshot, kvm_supported);
        ActionLaunch::ManagedRoomRevokeAll
    } else if action.id == "site-local" {
        ActionLaunch::ManagedLocalSitePreview
    } else if action.id == "site-open" {
        ActionLaunch::ManagedLocalSiteOpen
    } else if action.id == "site-ephemeral" {
        ActionLaunch::ManagedPublicSitePublish
    } else if action.id == "shares-list" {
        ActionLaunch::ManagedSharesList
    } else {
        ActionLaunch::External(action.args)
    }
}

fn action_args_with_kvm(
    action: ActionSpec,
    snapshot: &HomeSnapshot,
    kvm_supported: bool,
) -> &'static [&'static str] {
    let _ = (snapshot, kvm_supported);
    action.args
}

fn action_command(action: ActionSpec, snapshot: &HomeSnapshot) -> String {
    action_command_with_kvm(action, snapshot, elastos_crosvm::is_supported())
}

fn action_command_with_kvm(
    action: ActionSpec,
    snapshot: &HomeSnapshot,
    kvm_supported: bool,
) -> String {
    if action.id == "identity-nickname-set" {
        return "home: set the local DID profile nickname used across chat and people surfaces"
            .to_string();
    }
    if action.id == "site-local" {
        return "home: start MyWebSite local preview".to_string();
    }
    if action.id == "site-open" {
        return "home: open MyWebSite preview in browser".to_string();
    }
    if action.id == "chat" {
        return "home: open Chat".to_string();
    }
    if action.id == "room-approve" {
        return "home: approve the next pending Chat web guest request".to_string();
    }
    if action.id == "room-deny" {
        return "home: deny the next pending Chat web guest request".to_string();
    }
    if action.id == "room-revoke-all" {
        return "home: disconnect all active Chat web guest sessions".to_string();
    }
    if action.id == "site-ephemeral" {
        return "home: publish a temporary HTTPS URL for MyWebSite".to_string();
    }
    if action.id == "shares-list" {
        return "home: review current shared channels and open URLs".to_string();
    }
    format!(
        "elastos {}",
        action_args_with_kvm(action, snapshot, kvm_supported).join(" ")
    )
}

fn gather_platform_layers() -> Vec<PlatformLayer> {
    PLATFORM_LAYERS
        .iter()
        .map(|(name, role)| PlatformLayer {
            name: (*name).to_string(),
            role: (*role).to_string(),
        })
        .collect()
}

fn gather_system_services(components: &[ComponentStatus]) -> Vec<SystemServiceStatus> {
    SYSTEM_SERVICES
        .iter()
        .map(|service| {
            let ready = service
                .backing
                .iter()
                .all(|name| component_available_in(components, name));
            let state = summarize_component_sources(components, service.backing);
            SystemServiceStatus {
                name: service.name.to_string(),
                role: service.role.to_string(),
                backing: service.backing.join(" + "),
                state,
                ready,
            }
        })
        .collect()
}

fn gather_share_status() -> ShareStatus {
    let Ok(catalog) = elastos_server::shares::load_share_catalog() else {
        return ShareStatus::default();
    };

    let mut channels: Vec<ShareChannelStatus> = catalog
        .channels
        .iter()
        .map(|(name, channel)| ShareChannelStatus {
            name: name.clone(),
            latest_cid: channel.latest_cid.clone(),
            latest_version: channel.latest_version,
            status: channel.status.to_string(),
            head_cid: channel.head_cid.clone(),
        })
        .collect();

    channels.sort_by(|a, b| a.name.cmp(&b.name));

    ShareStatus {
        channel_count: channels.len(),
        active_count: channels
            .iter()
            .filter(|channel| channel.status == "active")
            .count(),
        author_did: catalog.author_did.clone(),
        channels,
    }
}

fn render_share_notice(shares: &ShareStatus) -> String {
    if shares.channel_count == 0 {
        return "Shared has nothing yet. Run `elastos share <path>` to publish a file or folder, then open it again from Home."
            .to_string();
    }

    let mut parts = Vec::new();
    for channel in shares.channels.iter().take(3) {
        parts.push(format!(
            "{} v{} {} elastos://{}",
            channel.name,
            channel.latest_version,
            channel.status,
            truncate_for_notice(&channel.latest_cid, 18)
        ));
    }

    let more = shares.channel_count.saturating_sub(parts.len());
    let mut summary = format!(
        "Shared now has {} channel{} ({} active): {}.",
        shares.channel_count,
        if shares.channel_count == 1 { "" } else { "s" },
        shares.active_count,
        parts.join(" - ")
    );
    if more > 0 {
        summary.push_str(&format!(" +{} more.", more));
    }
    summary.push_str(
        " Next: `elastos open elastos://<cid>` or `elastos shares list` for the full catalog.",
    );
    summary
}

fn render_site_local_blocked_notice(snapshot: &HomeSnapshot, reason: &str) -> String {
    if !snapshot.site.staged {
        return "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`. Then reopen MyWebSite from Home to preview or go public."
            .to_string();
    }
    if !component_available_in(&snapshot.components, "site-provider") {
        return "MyWebSite is staged at localhost://MyWebSite. Run `elastos setup --profile demo` to install site-provider, then reopen MyWebSite from Home."
            .to_string();
    }
    format!("MyWebSite unavailable: {}", reason)
}

fn render_site_public_blocked_notice(snapshot: &HomeSnapshot, reason: &str) -> String {
    if !snapshot.site.staged {
        return "MyWebSite needs a staged directory before it can go public. Run `elastos site stage <dir>` first."
            .to_string();
    }
    if !component_available_in(&snapshot.components, "site-provider")
        || !component_available_in(&snapshot.components, "tunnel-provider")
        || !component_available_in(&snapshot.components, "cloudflared")
    {
        return "Publish needs site-provider, tunnel-provider, and cloudflared. Run `elastos setup --profile demo`, then try again."
            .to_string();
    }
    format!("Publish unavailable: {}", reason)
}

fn render_site_preview_notice(site: &SiteStatus, local_url: &str, reused: bool) -> String {
    let mut notice = if reused {
        format!(
            "MyWebSite preview is already live at {} for localhost://MyWebSite.",
            local_url
        )
    } else {
        format!(
            "MyWebSite preview is live at {} for localhost://MyWebSite.",
            local_url
        )
    };

    if let Some(release) = site.active_release.as_deref() {
        if let Some(channel) = site.active_channel.as_deref() {
            notice.push_str(&format!(" Live release: {} on {}.", release, channel));
        } else {
            notice.push_str(&format!(" Live release: {}.", release));
        }
    } else if site.release_count > 0 {
        notice.push_str(&format!(" Saved releases: {}.", site.release_count));
    }

    if let Some(cid) = site.active_bundle_cid.as_deref() {
        notice.push_str(&format!(
            " Bundle: elastos://{}.",
            truncate_for_notice(cid, 22)
        ));
    }

    notice.push_str(
        " Next: use `mywebsite open` to open it, or `mywebsite publish` for a temporary HTTPS URL.",
    );
    notice
}

fn render_site_public_notice(site: &SiteStatus, local_url: &str, public_url: &str) -> String {
    let mut notice = format!(
        "MyWebSite public URL is live at {}. Local preview remains at {}.",
        public_url.trim_end_matches('/'),
        local_url.trim_end_matches('/'),
    );

    if let Some(release) = site.active_release.as_deref() {
        if let Some(channel) = site.active_channel.as_deref() {
            notice.push_str(&format!(" Live release: {} on {}.", release, channel));
        } else {
            notice.push_str(&format!(" Live release: {}.", release));
        }
    } else if site.release_count > 0 {
        notice.push_str(&format!(" Saved releases: {}.", site.release_count));
    }

    notice.push_str(" Next: share the HTTPS URL, or manage releases with `elastos site publish|activate|rollback`.");
    notice
}

fn truncate_for_notice(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

fn summarize_component_sources(components: &[ComponentStatus], required: &[&str]) -> String {
    let resolved: Vec<&ComponentStatus> = required
        .iter()
        .filter_map(|name| components.iter().find(|component| component.name == *name))
        .collect();
    if resolved.len() != required.len() {
        return "missing prerequisites".to_string();
    }
    if resolved
        .iter()
        .any(|component| component.installed && !component.available)
    {
        return "stale install".to_string();
    }
    if resolved.iter().any(|component| !component.available) {
        return "missing prerequisites".to_string();
    }
    if resolved
        .iter()
        .all(|component| component.source == "installed")
    {
        return "installed".to_string();
    }
    if resolved.iter().any(|component| component.source == "dev") {
        return "local workspace".to_string();
    }
    "ready".to_string()
}

fn load_existing_did(data_dir: &Path) -> Option<String> {
    let device_key = data_dir.join("identity").join("device.key");
    if !device_key.exists() {
        return None;
    }

    elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_, did)| did)
}

async fn load_runtime_nickname(data_dir: &Path) -> Option<String> {
    elastos_identity::load_nickname(data_dir).ok().flatten()
}

fn load_default_source(data_dir: &Path) -> anyhow::Result<Option<SourceStatus>> {
    let cfg = load_trusted_sources(data_dir)?;
    let Some(source) = cfg.default_source() else {
        return Ok(None);
    };

    Ok(Some(SourceStatus {
        name: source.name.clone(),
        channel: if source.channel.trim().is_empty() {
            "stable".to_string()
        } else {
            source.channel.clone()
        },
        installed_version: source.installed_version.clone(),
        gateway: source.gateways.first().cloned(),
    }))
}

fn gather_roots(data_dir: &Path) -> Vec<RootStatus> {
    ALL_ROOTS
        .iter()
        .map(|root| {
            let path = if FILE_BACKED_ROOTS.contains(root) {
                Some(data_dir.join(root))
            } else {
                None
            };
            let (description, example) = root_descriptor(root);

            RootStatus {
                name: (*root).to_string(),
                kind: if DYNAMIC_ROOTS.contains(root) {
                    "dynamic".to_string()
                } else {
                    "file-backed".to_string()
                },
                uri: format!("localhost://{}", root),
                exists: path.as_ref().is_some_and(|p| p.exists()),
                path: path.map(|p| p.display().to_string()),
                description: description.to_string(),
                example: example.to_string(),
            }
        })
        .collect()
}

fn gather_components(data_dir: &Path) -> Vec<ComponentStatus> {
    COMPONENTS
        .iter()
        .map(|(name, kind)| {
            let installed_path = data_dir.join("bin").join(name);
            let resolved_path = crate::find_installed_provider_binary(name);
            let installed = installed_path.is_file();
            let (available, source) = match resolved_path.as_ref() {
                Some(path) if path == &installed_path => {
                    if crate::setup::verify_installed_component_binary(data_dir, name, path).is_ok()
                    {
                        (true, "installed")
                    } else {
                        (false, "stale")
                    }
                }
                None => (false, "missing"),
                Some(_) => (true, "installed"),
            };

            ComponentStatus {
                name: (*name).to_string(),
                kind: (*kind).to_string(),
                installed,
                available,
                source: source.to_string(),
                installed_path: installed_path.display().to_string(),
                resolved_path: resolved_path.map(|path| path.display().to_string()),
            }
        })
        .collect()
}

fn gather_cached_capsules(data_dir: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let cache_dir = data_dir.join("capsules");
    if let Ok(read_dir) = fs::read_dir(cache_dir) {
        for entry in read_dir.flatten() {
            if entry.path().is_dir() {
                entries.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    entries.sort();
    entries
}

/// Discover installed capsules and produce launch actions for user-facing ones.
///
/// Uses the installed-active catalog and keeps only capsules whose canonical
/// projection declares an available CLI surface.
fn gather_capsule_actions(catalog: &serde_json::Value) -> Vec<ActionInfo> {
    let mut actions = Vec::new();
    let capsules = catalog["capsules"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    for capsule in capsules {
        if !catalog_capsule_has_cli_projection(capsule) {
            continue;
        }
        let Some(name) = capsule.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let label = capsule
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or(name);
        let description = capsule
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let command = format!(
            "elastos capsule {} --lifecycle interactive --interactive",
            name
        );
        actions.push(ActionInfo {
            id: format!("capsule-{}", name),
            label: label.to_string(),
            description: description.to_string(),
            command,
            ready: true,
            reason: None,
        });
    }
    actions.sort_by(|a, b| a.label.cmp(&b.label));
    actions
}

async fn gather_runtime_status(data_dir: &Path) -> RuntimeStatus {
    let coords_path = runtime_control::runtime_coord_path(data_dir);
    let Some(coords) = runtime_control::read_runtime_coords(&coords_path).await else {
        return RuntimeStatus {
            running: false,
            kind: None,
            version: None,
            api_url: None,
            pid: None,
            peer_count: None,
            ticket: None,
            running_capsules: Vec::new(),
            note: Some("No active local ElastOS service".to_string()),
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return RuntimeStatus {
                running: true,
                kind: Some(coords.runtime_kind.clone()),
                version: None,
                api_url: Some(coords.api_url.clone()),
                pid: Some(coords.pid),
                peer_count: None,
                ticket: None,
                running_capsules: Vec::new(),
                note: Some(format!("runtime client unavailable: {}", err)),
            };
        }
    };

    let version = fetch_runtime_version(&client, &coords.api_url).await;
    let mut status = RuntimeStatus {
        running: true,
        kind: Some(coords.runtime_kind.clone()),
        version,
        api_url: Some(coords.api_url.clone()),
        pid: Some(coords.pid),
        peer_count: None,
        ticket: None,
        running_capsules: Vec::new(),
        note: None,
    };

    let client_token = match attach_client(&client, &coords).await {
        Ok(token) => token,
        Err(err) => {
            status.note = Some(format!("attach failed: {}", err));
            return status;
        }
    };

    if let Ok(capsules) = list_runtime_capsules(&client, &coords.api_url, &client_token).await {
        status.running_capsules = capsules;
    }

    if let Ok(peer_cap) = crate::request_attached_capability(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://peer/*",
        "message",
    )
    .await
    {
        status.peer_count = fetch_peer_count(&client, &coords.api_url, &client_token, &peer_cap)
            .await
            .ok();
        status.ticket = fetch_ticket(&client, &coords.api_url, &client_token, &peer_cap)
            .await
            .ok();
    }

    status
}

async fn gather_home_summary_projection(
    data_dir: &Path,
    api_url: Option<&str>,
) -> anyhow::Result<Option<HomeSummaryFactsProjection>> {
    let gateway_owned = gateway_owned_home_terminal();
    let api_url = if gateway_owned {
        gateway_home_cli_api_url().ok_or_else(|| {
            anyhow::anyhow!("gateway-owned Home CLI summary unavailable: missing gateway API URL")
        })?
    } else {
        let Some(api_url) = api_url
            .map(str::trim)
            .filter(|api_url| !api_url.is_empty())
            .map(|api_url| api_url.trim_end_matches('/').to_string())
        else {
            return Ok(None);
        };
        api_url
    };
    let token = match issue_home_cli_launch_token(data_dir, HOME_CLI_CAPSULE_NAME) {
        Ok(token) => token,
        Err(err) if gateway_owned => {
            return Err(err).context("gateway-owned Home CLI summary unavailable: launch token");
        }
        Err(_) => return Ok(None),
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) if gateway_owned => {
            return Err(err).context("gateway-owned Home CLI summary unavailable: HTTP client");
        }
        Err(_) => return Ok(None),
    };
    let url = format!("{}/api/apps/home/summary", api_url.trim_end_matches('/'));
    let response = match client
        .get(&url)
        .header("x-elastos-home-token", token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if gateway_owned => {
            return Err(err).with_context(|| {
                format!("gateway-owned Home CLI summary unavailable: request {url}")
            });
        }
        Err(_) => return Ok(None),
    };
    let status = response.status();
    if !status.is_success() {
        if gateway_owned {
            anyhow::bail!("gateway-owned Home CLI summary unavailable: {url} returned {status}");
        }
        return Ok(None);
    }
    match response.json::<HomeSummaryFactsProjection>().await {
        Ok(projection) => Ok(Some(projection)),
        Err(err) if gateway_owned => {
            Err(err).context("gateway-owned Home CLI summary unavailable: invalid summary response")
        }
        Err(_) => Ok(None),
    }
}

async fn gather_capsule_interface_snapshot(
    data_dir: &Path,
    api_url: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let gateway_owned = gateway_owned_home_terminal();
    let api_url = if gateway_owned {
        gateway_home_cli_api_url().ok_or_else(|| {
            anyhow::anyhow!(
                "gateway-owned Home CLI interface registry unavailable: missing gateway API URL"
            )
        })?
    } else {
        let Some(api_url) = api_url
            .map(str::trim)
            .filter(|api_url| !api_url.is_empty())
            .map(|api_url| api_url.trim_end_matches('/').to_string())
        else {
            return Ok(elastos_server::api::gateway::capsule_interface_registry_snapshot(data_dir));
        };
        api_url
    };
    let token = match issue_home_cli_launch_token(data_dir, HOME_CLI_CAPSULE_NAME) {
        Ok(token) => token,
        Err(err) if gateway_owned => {
            return Err(err)
                .context("gateway-owned Home CLI interface registry unavailable: launch token");
        }
        Err(_) => {
            return Ok(elastos_server::api::gateway::capsule_interface_registry_snapshot(data_dir));
        }
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let url = format!("{}/api/capsules/interfaces", api_url.trim_end_matches('/'));
    let response = match client
        .get(&url)
        .header("x-elastos-home-token", token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if gateway_owned => {
            return Err(err).with_context(|| {
                format!("gateway-owned Home CLI interface registry unavailable: request {url}")
            });
        }
        Err(_) => {
            return Ok(elastos_server::api::gateway::capsule_interface_registry_snapshot(data_dir));
        }
    };
    if !response.status().is_success() {
        if gateway_owned {
            anyhow::bail!(
                "gateway-owned Home CLI interface registry unavailable: {url} returned {}",
                response.status()
            );
        }
        return Ok(elastos_server::api::gateway::capsule_interface_registry_snapshot(data_dir));
    }
    response.json().await.map_err(Into::into)
}

async fn fetch_runtime_version(client: &reqwest::Client, api_url: &str) -> Option<String> {
    let resp = client
        .get(format!("{}/api/health", api_url))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("version")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

async fn attach_client(
    client: &reqwest::Client,
    coords: &runtime_control::RuntimeCoords,
) -> anyhow::Result<String> {
    let resp = client
        .post(format!("{}/api/auth/attach", coords.api_url))
        .json(&serde_json::json!({
            "secret": coords.attach_secret,
            "scope": "client",
        }))
        .send()
        .await?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "attach failed [{}]: {}",
            status,
            body.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("request failed")
        );
    }
    body.get("token")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("attach response missing token"))
}

async fn list_runtime_capsules(
    client: &reqwest::Client,
    api_url: &str,
    client_token: &str,
) -> anyhow::Result<Vec<String>> {
    let resp = client
        .get(format!("{}/api/capsules", api_url))
        .header("Authorization", format!("Bearer {}", client_token))
        .send()
        .await?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("capsule list failed [{}]", status);
    }

    let mut capsules = Vec::new();
    if let Some(items) = body.get("capsules").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                let rendered = if let Some(state) = item.get("status").and_then(|v| v.as_str()) {
                    format!("{} ({})", name, state)
                } else {
                    name.to_string()
                };
                capsules.push(rendered);
            }
        }
    }
    Ok(capsules)
}

async fn fetch_peer_count(
    client: &reqwest::Client,
    api_url: &str,
    client_token: &str,
    peer_cap: &str,
) -> anyhow::Result<usize> {
    let body = provider_call(
        client,
        api_url,
        client_token,
        peer_cap,
        "peer",
        "list_peers",
        &serde_json::json!({}),
    )
    .await?;
    let count = body
        .get("data")
        .and_then(|d| d.get("peers"))
        .and_then(|p| p.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(count)
}

async fn fetch_ticket(
    client: &reqwest::Client,
    api_url: &str,
    client_token: &str,
    peer_cap: &str,
) -> anyhow::Result<String> {
    let body = provider_call(
        client,
        api_url,
        client_token,
        peer_cap,
        "peer",
        "get_ticket",
        &serde_json::json!({}),
    )
    .await?;
    body.get("data")
        .and_then(|d| d.get("ticket"))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("ticket response missing ticket"))
}

async fn provider_call(
    client: &reqwest::Client,
    api_url: &str,
    client_token: &str,
    cap_token: &str,
    scheme: &str,
    op: &str,
    body: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let resp = client
        .post(format!("{}/api/provider/{}/{}", api_url, scheme, op))
        .header("Authorization", format!("Bearer {}", client_token))
        .header("X-Capability-Token", cap_token)
        .json(body)
        .send()
        .await?;
    let status = resp.status();
    let value: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("provider {}/{} failed [{}]", scheme, op, status);
    }
    Ok(value)
}

async fn localhost_exists(access: &SessionAccess, path: &str) -> anyhow::Result<bool> {
    let body = provider_call(
        &access.client,
        &access.api_url,
        &access.client_token,
        &access.read_cap,
        "localhost",
        "exists",
        &serde_json::json!({
            "path": path,
        }),
    )
    .await?;
    Ok(body
        .get("data")
        .and_then(|d| d.get("exists"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

async fn read_localhost_file(access: &SessionAccess, path: &str) -> anyhow::Result<Vec<u8>> {
    let body = provider_call(
        &access.client,
        &access.api_url,
        &access.client_token,
        &access.read_cap,
        "localhost",
        "read",
        &serde_json::json!({
            "path": path,
        }),
    )
    .await?;

    let data = body
        .get("data")
        .and_then(|d| d.get("content").or_else(|| d.get("data")))
        .ok_or_else(|| anyhow::anyhow!("localhost/read response missing data"))?;

    if let Some(bytes) = data.as_array() {
        return Ok(bytes
            .iter()
            .filter_map(|value| value.as_u64().map(|byte| byte as u8))
            .collect());
    }

    if let Some(text) = data.as_str() {
        return Ok(text.as_bytes().to_vec());
    }

    anyhow::bail!("localhost/read returned unsupported data shape")
}

async fn write_localhost_file(
    access: &SessionAccess,
    path: &str,
    content: Vec<u8>,
) -> anyhow::Result<()> {
    let _ = provider_call(
        &access.client,
        &access.api_url,
        &access.client_token,
        &access.write_cap,
        "localhost",
        "write",
        &serde_json::json!({
            "path": path,
            "content": content,
            "append": false,
        }),
    )
    .await?;
    Ok(())
}

async fn delete_localhost_file(access: &SessionAccess, path: &str) -> anyhow::Result<()> {
    let _ = provider_call(
        &access.client,
        &access.api_url,
        &access.client_token,
        &access.write_cap,
        "localhost",
        "delete",
        &serde_json::json!({
            "path": path,
            "recursive": false,
        }),
    )
    .await?;
    Ok(())
}

fn print_home_state_probe(snapshot: &HomeSnapshot) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "ElastOS Home State Probe")?;
    writeln!(out, "  Version:   {}", snapshot.version)?;
    writeln!(out, "  User:      {}", snapshot.user)?;
    writeln!(
        out,
        "  Nickname:  {}",
        snapshot.nickname.as_deref().unwrap_or("(not set)")
    )?;
    writeln!(
        out,
        "  Identity:  {}",
        snapshot.did.as_deref().unwrap_or("(not initialized yet)")
    )?;
    writeln!(out, "  Data dir:  {}", snapshot.data_dir)?;
    writeln!(
        out,
        "  Source:    {}",
        snapshot
            .source
            .as_ref()
            .map(|source| {
                format!(
                    "{}{}",
                    source.name,
                    source
                        .gateway
                        .as_ref()
                        .map(|gateway| format!(" via {}", gateway))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "no trusted source configured".to_string())
    )?;
    writeln!(out)?;

    writeln!(out, "Home")?;
    writeln!(
        out,
        "  Network:   {}",
        match snapshot.runtime.peer_count {
            Some(0) if snapshot.runtime.ticket.is_some() =>
                "Carrier bootstrap ready; waiting for another Home".to_string(),
            Some(0) => "starting up".to_string(),
            Some(1) => "1 Carrier peer reachable".to_string(),
            Some(peers) => format!("{} Carrier peers reachable", peers),
            None => "runtime not connected yet".to_string(),
        }
    )?;
    writeln!(
        out,
        "  MyWebSite: {} ({})",
        snapshot.site.root_uri,
        if snapshot.site.staged {
            "staged"
        } else {
            "not staged"
        }
    )?;
    if snapshot.room.pending_count > 0 {
        let latest = match (
            snapshot.room.latest_request_name.as_deref(),
            snapshot.room.latest_request_device.as_deref(),
        ) {
            (Some(name), Some(device)) => format!("{} on {}", name, device),
            (Some(name), None) => name.to_string(),
            _ => "web guest approval needed".to_string(),
        };
        writeln!(
            out,
            "  Chat join: {} pending ({})",
            snapshot.room.pending_count, latest
        )?;
    }
    if snapshot.room.active_session_count > 0 {
        writeln!(
            out,
            "  Chat:      {} web guest(s) active ({})",
            snapshot.room.active_session_count,
            format_room_participants(&snapshot.room.active_participants)
        )?;
    }
    if let Some(url) = snapshot.site.local_url.as_deref() {
        writeln!(out, "  Preview:   {}", url.trim_end_matches('/'))?;
    }
    writeln!(
        out,
        "  Capsules:  {} installed / {} running",
        snapshot.cached_capsules.len(),
        snapshot.runtime.running_capsules.len()
    )?;
    if let Some(version) = &snapshot.runtime.version {
        writeln!(out, "  Runtime:   {}", version)?;
    }
    if let Some(kind) = &snapshot.runtime.kind {
        writeln!(out, "  Mode:      {}", kind)?;
    }
    if let Some(note) = &snapshot.runtime.note {
        writeln!(out, "  Note:      {}", note)?;
    }
    writeln!(out)?;

    writeln!(out, "People")?;
    writeln!(
        out,
        "  Nick:      {}",
        snapshot.nickname.as_deref().unwrap_or("(not set)")
    )?;
    writeln!(
        out,
        "  Chat:      {}",
        snapshot
            .actions
            .iter()
            .find(|action| action.id == "chat")
            .map(|action| if action.ready { "ready" } else { "needs setup" })
            .unwrap_or("not available")
    )?;
    writeln!(
        out,
        "  Carrier:   {} reachable",
        snapshot.runtime.peer_count.unwrap_or_default()
    )?;
    writeln!(
        out,
        "  Delivery:  {}",
        if snapshot.runtime.peer_count.unwrap_or_default() == 0 {
            "local only until another Home joins Chat"
        } else {
            "open Chat and send a line to confirm delivery"
        }
    )?;
    writeln!(out, "  Roots:     localhost://Users, localhost://UsersAI")?;
    writeln!(out, "  Profile:   elastos identity nickname set")?;
    if let Some(ticket) = &snapshot.runtime.ticket {
        writeln!(out, "  Ticket:    {}", ticket)?;
    }
    writeln!(out)?;

    writeln!(out, "Spaces")?;
    for name in ["MyWebSite", "Public", "Local", "WebSpaces"] {
        if let Some(root) = snapshot.roots.iter().find(|root| root.name == name) {
            writeln!(
                out,
                "  {:<11} {:<11} {}",
                root.name,
                format!("[{}]", root.kind),
                root.path.as_deref().unwrap_or("(dynamic)")
            )?;
        }
    }
    writeln!(out)?;

    writeln!(out, "Apps")?;
    if snapshot.cached_capsules.is_empty() {
        writeln!(out, "  Installed: (none cached yet)")?;
    } else {
        writeln!(out, "  Installed: {}", snapshot.cached_capsules.join(", "))?;
    }
    if snapshot.runtime.running_capsules.is_empty() {
        writeln!(out, "  Running:   (none)")?;
    } else {
        writeln!(
            out,
            "  Running:   {}",
            snapshot.runtime.running_capsules.join(", ")
        )?;
    }
    writeln!(out)?;

    writeln!(out, "System")?;
    for layer in &snapshot.platform_layers {
        writeln!(out, "  {:<15} {}", layer.name, layer.role)?;
    }
    writeln!(out, "  Services:")?;
    for service in &snapshot.system_services {
        writeln!(
            out,
            "    {:<18} {:<5} {}",
            service.name,
            if service.ready { "[ok]" } else { "[no]" },
            service.state
        )?;
        writeln!(out, "      {}", service.role)?;
        writeln!(out, "      backing: {}", service.backing)?;
    }
    writeln!(out, "  Commands:")?;
    for group in &snapshot.command_groups {
        writeln!(out, "    {:<10} {}", group.name, group.commands.join(", "))?;
    }
    writeln!(out)?;

    writeln!(out, "Launch From Home")?;
    for action in &snapshot.actions {
        writeln!(
            out,
            "  {:<24} {:<5} {}",
            action.label,
            if action.ready { "[ok]" } else { "[no]" },
            action.reason.as_deref().unwrap_or(action.command.as_str())
        )?;
    }
    Ok(())
}

fn is_broken_pipe(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::BrokenPipe
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "anon".to_string())
}

fn root_descriptor(root: &str) -> (&'static str, &'static str) {
    match root {
        "AppCapsules" => (
            "Installed AppCapsules, including ordinary apps and special WebSpace-style capsules.",
            "localhost://AppCapsules/Ela.city",
        ),
        "ElastOS" => (
            "Local trust, update, service, and system registry state for this Home.",
            "localhost://ElastOS/SystemRegistry",
        ),
        "Local" => (
            "Scratch space for temporary work, session state, and things that are not public yet.",
            "localhost://Local/Shared",
        ),
        "MyWebSite" => (
            "Browser-facing site root for the current sovereign Home, with preview, releases, and live channels.",
            "localhost://MyWebSite/index.html",
        ),
        "Public" => (
            "Shared files root for things you want to open or pass around outside your private site.",
            "localhost://Public/manual.pdf",
        ),
        "Users" => (
            "Personal home directories, documents, settings, saved games, and per-user app data.",
            "localhost://Users/<principal-root>/.AppData/LocalHost/Chat",
        ),
        "UsersAI" => (
            "Resident AI home directories mirroring Users for sovereign agent surfaces in this Home.",
            "localhost://UsersAI/Codex",
        ),
        "WebSpaces" => (
            "Named handles that resolve into content, peers, identity, and AI surfaces without exposing raw provider details.",
            "localhost://WebSpaces/Elastos",
        ),
        _ => ("Local Home root.", "localhost://"),
    }
}

fn action_readiness(action_id: &str, snapshot: &HomeSnapshot) -> ActionReadiness {
    if let Some(request_id) = action_id.strip_prefix("room-approve-request:") {
        return if snapshot
            .room
            .pending_requests
            .iter()
            .any(|request| request.request_id == request_id)
        {
            ActionReadiness::Ready
        } else {
            ActionReadiness::Blocked("web guest request is no longer pending".to_string())
        };
    }
    if let Some(request_id) = action_id.strip_prefix("room-deny-request:") {
        return if snapshot
            .room
            .pending_requests
            .iter()
            .any(|request| request.request_id == request_id)
        {
            ActionReadiness::Ready
        } else {
            ActionReadiness::Blocked("web guest request is no longer pending".to_string())
        };
    }
    if let Some(token) = action_id.strip_prefix("room-revoke-session:") {
        return if snapshot
            .room
            .active_sessions
            .iter()
            .any(|session| session.token == token)
        {
            ActionReadiness::Ready
        } else {
            ActionReadiness::Blocked("web guest session is no longer active".to_string())
        };
    }

    match action_id {
        "identity-nickname-set" => require_components(
            snapshot,
            &["did-provider"],
            "run: elastos setup --with did-provider",
        ),
        "chat" => require_components(
            snapshot,
            &["shell", "localhost-provider", "did-provider"],
            "run: elastos setup",
        ),
        "room-approve" | "room-deny" => {
            if snapshot.room.pending_count == 0 {
                ActionReadiness::Blocked("no web guest requests pending".to_string())
            } else {
                ActionReadiness::Ready
            }
        }
        "room-revoke-all" => {
            if snapshot.room.active_session_count == 0 {
                ActionReadiness::Blocked("no active web guest sessions".to_string())
            } else {
                ActionReadiness::Ready
            }
        }
        "site-local" | "site-open" => {
            if !snapshot.site.staged {
                return ActionReadiness::Blocked(
                    "stage a site first with `elastos site stage <dir>`".to_string(),
                );
            }
            require_components(
                snapshot,
                &["site-provider"],
                "run: elastos setup --profile demo",
            )
        }
        "site-ephemeral" => {
            if !snapshot.site.staged {
                return ActionReadiness::Blocked(
                    "stage a site first with `elastos site stage <dir>`".to_string(),
                );
            }
            require_components(
                snapshot,
                &["site-provider", "tunnel-provider", "cloudflared"],
                "run: elastos setup --profile demo",
            )
        }
        "shares-list" => ActionReadiness::Ready,
        "room-policy-toggle-guests"
        | "room-policy-toggle-members"
        | "room-policy-toggle-member-hosts" => {
            if room_admin_role(snapshot) {
                ActionReadiness::Ready
            } else {
                ActionReadiness::Blocked(
                    "only conversation managers may change Chat access policy".to_string(),
                )
            }
        }
        _ if action_id.starts_with("room-revoke-invite:") => {
            if room_admin_role(snapshot) {
                ActionReadiness::Ready
            } else {
                ActionReadiness::Blocked(
                    "only conversation managers may cancel ElastOS user invites".to_string(),
                )
            }
        }
        _ if action_id.starts_with("room-remove-member:") => {
            if room_admin_role(snapshot) {
                ActionReadiness::Ready
            } else {
                ActionReadiness::Blocked(
                    "only conversation managers may remove trusted participants".to_string(),
                )
            }
        }
        _ if action_id.starts_with("room-accept-invite:") => {
            if snapshot.room.local_runtime_did.is_some() {
                ActionReadiness::Ready
            } else {
                ActionReadiness::Blocked("local ElastOS identity is not available yet".to_string())
            }
        }
        _ => ActionReadiness::Blocked("unknown action".to_string()),
    }
}

fn gather_room_actions(snapshot: &HomeSnapshot) -> Vec<ActionInfo> {
    let mut actions = Vec::new();
    if room_admin_role(snapshot) {
        actions.push(ActionInfo {
            id: "room-policy-toggle-guests".to_string(),
            label: if snapshot.room.allow_guest_invites {
                "Close public join requests".to_string()
            } else {
                "Open public join requests".to_string()
            },
            description: if snapshot.room.allow_guest_invites {
                "Stop new web guests from requesting access through the public Chat link."
                    .to_string()
            } else {
                "Allow new web guests to request access through the public Chat link.".to_string()
            },
            command: "home: toggle public Chat join requests".to_string(),
            ready: true,
            reason: None,
        });
        actions.push(ActionInfo {
            id: "room-policy-toggle-members".to_string(),
            label: if snapshot.room.allow_member_invites {
                "Close ElastOS user invites".to_string()
            } else {
                "Open ElastOS user invites".to_string()
            },
            description: if snapshot.room.allow_member_invites {
                "Stop issuing new invites for trusted ElastOS users.".to_string()
            } else {
                "Allow new invites for trusted ElastOS users.".to_string()
            },
            command: "home: toggle ElastOS user invites for Chat".to_string(),
            ready: true,
            reason: None,
        });
        actions.push(ActionInfo {
            id: "room-policy-toggle-member-hosts".to_string(),
            label: if snapshot.room.allow_members_to_host_guests {
                "Restrict web guest approvals".to_string()
            } else {
                "Allow trusted guest approvals".to_string()
            },
            description: if snapshot.room.allow_members_to_host_guests {
                "Limit web guest approvals to conversation managers.".to_string()
            } else {
                "Allow trusted ElastOS users to approve web guests from their Homes.".to_string()
            },
            command: "home: toggle whether trusted ElastOS users may approve web guests"
                .to_string(),
            ready: true,
            reason: None,
        });
    }
    if let Some(local_runtime_did) = snapshot.room.local_runtime_did.as_deref() {
        for invite in &snapshot.room.pending_invites {
            if invite.invited_did == local_runtime_did {
                actions.push(ActionInfo {
                    id: format!("room-accept-invite:{}", invite.invite_id),
                    label: "Join trusted conversation".to_string(),
                    description: "Accept this ElastOS user invite on the local Home.".to_string(),
                    command: "home: accept this ElastOS user invite on the local Home".to_string(),
                    ready: true,
                    reason: None,
                });
            }
        }
    }
    if room_admin_role(snapshot) {
        for invite in &snapshot.room.pending_invites {
            actions.push(ActionInfo {
                id: format!("room-revoke-invite:{}", invite.invite_id),
                label: format!("Revoke invite for {}", invite.invited_did),
                description: "Cancel this pending ElastOS user invite.".to_string(),
                command: "home: cancel this specific ElastOS user invite".to_string(),
                ready: true,
                reason: None,
            });
        }
        for member in &snapshot.room.members {
            if member.role == "owner" {
                continue;
            }
            if snapshot.room.local_runtime_did.as_deref() == Some(member.member_did.as_str()) {
                continue;
            }
            if can_manage_member(snapshot, member) {
                actions.push(ActionInfo {
                    id: format!("room-remove-member:{}", member.member_did),
                    label: format!("Remove {}", member.member_did),
                    description: "Remove this trusted participant from Chat.".to_string(),
                    command: "home: remove this trusted participant from Chat".to_string(),
                    ready: true,
                    reason: None,
                });
            }
        }
    }
    for request in &snapshot.room.pending_requests {
        actions.push(ActionInfo {
            id: format!("room-approve-request:{}", request.request_id),
            label: format!(
                "Approve {} on {}",
                request.display_name, request.device_label
            ),
            description: "Approve this specific web guest join request.".to_string(),
            command: "home: approve this specific Chat web guest request".to_string(),
            ready: true,
            reason: None,
        });
        actions.push(ActionInfo {
            id: format!("room-deny-request:{}", request.request_id),
            label: format!("Deny {} on {}", request.display_name, request.device_label),
            description: "Deny this specific web guest join request.".to_string(),
            command: "home: deny this specific Chat web guest request".to_string(),
            ready: true,
            reason: None,
        });
    }
    for session in &snapshot.room.active_sessions {
        actions.push(ActionInfo {
            id: format!("room-revoke-session:{}", session.token),
            label: format!(
                "Disconnect {} on {}",
                session.display_name, session.device_label
            ),
            description: "Disconnect this specific web guest session.".to_string(),
            command: "home: disconnect this specific Chat web guest session".to_string(),
            ready: true,
            reason: None,
        });
    }
    actions
}

fn room_admin_role(snapshot: &HomeSnapshot) -> bool {
    matches!(
        snapshot.room.local_runtime_role.as_deref(),
        Some("owner" | "admin")
    )
}

fn can_manage_member(snapshot: &HomeSnapshot, member: &RoomMemberStatus) -> bool {
    match snapshot.room.local_runtime_role.as_deref() {
        Some("owner") => member.role != "owner",
        Some("admin") => member.role == "member",
        _ => false,
    }
}

fn require_room_admin_actor(snapshot: &HomeSnapshot) -> anyhow::Result<String> {
    if !room_admin_role(snapshot) {
        anyhow::bail!("only conversation managers may change Chat access policy");
    }
    snapshot.room.local_runtime_did.clone().ok_or_else(|| {
        anyhow::anyhow!("local ElastOS identity is not available for Chat access policy changes")
    })
}

fn format_room_participants(participants: &[RoomParticipantStatus]) -> String {
    if participants.is_empty() {
        return "web guest session active".to_string();
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

fn require_components(snapshot: &HomeSnapshot, required: &[&str], hint: &str) -> ActionReadiness {
    let missing: Vec<String> = required
        .iter()
        .filter_map(|name| {
            let component = snapshot
                .components
                .iter()
                .find(|component| component.name == **name);
            match component {
                Some(component) if component.available => None,
                Some(component) if component.installed => {
                    Some(format!("{} (stale or unverifiable install)", name))
                }
                Some(_) | None => Some((*name).to_string()),
            }
        })
        .collect();
    if missing.is_empty() {
        ActionReadiness::Ready
    } else {
        ActionReadiness::Blocked(format!("missing {} -- {}", missing.join(", "), hint))
    }
}

fn component_available_in(components: &[ComponentStatus], name: &str) -> bool {
    components
        .iter()
        .find(|component| component.name == name)
        .is_some_and(|component| component.available)
}

#[cfg(test)]
mod tests {
    use super::*;

    static HOME_CMD_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn assert_text_contains_all(label: &str, text: &str, needles: &[&str]) {
        for needle in needles {
            assert!(text.contains(needle), "{label} missing text: {needle}");
        }
    }

    #[test]
    fn gateway_owned_home_cli_authority_helper_reads_gateway_env() {
        let _guard = HOME_CMD_ENV_LOCK.blocking_lock();
        let temp = tempfile::tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", temp.path().join("home"));
        let _xdg_data_home = ScopedEnvVar::set("XDG_DATA_HOME", temp.path().join("xdg"));
        let data_dir = default_data_dir();
        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "1");
        let _principal = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID_ENV,
            "principal-a",
        );
        let _session = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_SESSION_ID_ENV,
            "session-a",
        );
        let _proof = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID_ENV,
            "",
        );
        let _grant = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_GRANT_ID_ENV,
            "grant-a",
        );
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            "http://localhost:61180/",
        );

        assert!(
            elastos_server::api::gateway::gateway_owned_home_cli_authority_available(&data_dir)
        );
        let status = gather_home_cli_session_status();
        assert_eq!(status.mode, "browser_pty");
        assert_eq!(
            status.passkey_state,
            "launch-token authorized browser Home session"
        );
        assert_eq!(
            gateway_home_cli_api_url().as_deref(),
            Some("http://localhost:61180")
        );
    }

    #[tokio::test]
    async fn gateway_owned_home_summary_projection_requires_gateway_api_url() {
        let _guard = HOME_CMD_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "1");
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            "",
        );

        let err = gather_home_summary_projection(temp.path(), Some("http://127.0.0.1:1"))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("gateway-owned Home CLI summary unavailable"));
        assert!(err.contains("missing gateway API URL"));
    }

    #[tokio::test]
    async fn gateway_owned_home_snapshot_propagates_summary_failure() {
        let _guard = HOME_CMD_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", temp.path().join("home"));
        let _xdg_data_home = ScopedEnvVar::set("XDG_DATA_HOME", temp.path().join("xdg"));
        let _coords = ScopedEnvVar::set(
            "ELASTOS_RUNTIME_COORDS_FILE",
            temp.path().join("missing-runtime-coords.json"),
        );
        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "1");
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            "",
        );

        let err = gather_snapshot().await.unwrap_err().to_string();

        assert!(err.contains("gateway-owned Home CLI summary unavailable"));
        assert!(err.contains("missing gateway API URL"));
    }

    #[tokio::test]
    async fn gateway_owned_home_summary_projection_fails_on_gateway_http_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let _guard = HOME_CMD_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "1");
        let _principal = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID_ENV,
            "person:test-home-cli",
        );
        let _session = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_SESSION_ID_ENV,
            "session-test-home-cli",
        );
        let _proof = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID_ENV,
            "",
        );
        let _grant = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_AUTH_CONTEXT_GRANT_ID_ENV,
            "grant-test-home-cli",
        );
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            format!("http://{addr}"),
        );

        let err = gather_home_summary_projection(temp.path(), None)
            .await
            .unwrap_err()
            .to_string();

        server.await.unwrap();
        assert!(err.contains("gateway-owned Home CLI summary unavailable"));
        assert!(err.contains("returned 503 Service Unavailable"));
    }

    #[tokio::test]
    async fn native_home_summary_projection_is_optional_without_gateway() {
        let _guard = HOME_CMD_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "0");
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            "",
        );

        let projection = gather_home_summary_projection(temp.path(), None)
            .await
            .unwrap();

        assert!(projection.is_none());
    }

    fn sample_snapshot_with_components(names: &[&str]) -> HomeSnapshot {
        let components = names
            .iter()
            .map(|name| ComponentStatus {
                name: (*name).to_string(),
                kind: "provider".to_string(),
                installed: true,
                available: true,
                source: "installed".to_string(),
                installed_path: format!("/tmp/{}", name),
                resolved_path: Some(format!("/tmp/{}", name)),
            })
            .collect();

        HomeSnapshot {
            version: "test".to_string(),
            user: "tester".to_string(),
            nickname: Some("tester".to_string()),
            did: None,
            session: HomeCliSessionStatus {
                mode: "native_terminal".to_string(),
                passkey_state: "local operator session; no browser passkey active".to_string(),
            },
            data_dir: "/tmp/elastos".to_string(),
            source: None,
            runtime: RuntimeStatus {
                running: false,
                kind: None,
                version: None,
                api_url: None,
                pid: None,
                peer_count: None,
                ticket: None,
                running_capsules: Vec::new(),
                note: None,
            },
            platform_layers: Vec::new(),
            system_services: Vec::new(),
            services: None,
            active_shell: serde_json::json!({
                "schema": "elastos.home.active-shell/v1",
                "active": "home-cli",
                "candidates": []
            }),
            targets: Vec::new(),
            site: SiteStatus {
                staged: false,
                root_uri: "localhost://MyWebSite".to_string(),
                path: "/tmp/MyWebSite".to_string(),
                local_url: None,
                active_release: None,
                active_channel: None,
                active_bundle_cid: None,
                release_count: 0,
            },
            shares: ShareStatus::default(),
            room: RoomStatus::default(),
            people: PeopleStatus::default(),
            notifications: NotificationStatus::default(),
            roots: Vec::new(),
            components,
            cached_capsules: Vec::new(),
            capsule_catalog: None,
            capsule_interfaces: None,
            command_groups: Vec::new(),
            actions: Vec::new(),
            notice: None,
        }
    }

    fn write_home_cmd_test_capsule(
        data_dir: &Path,
        name: &str,
        role: &str,
        interfaces: Vec<serde_json::Value>,
    ) {
        activate_home_cmd_test_capsule(data_dir, name);
        let capsule_dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(capsule_dir.join("browser")).unwrap();
        fs::write(capsule_dir.join(format!("{name}.wasm")), b"\0asm").unwrap();
        fs::write(
            capsule_dir.join("browser").join("index.html"),
            "<!doctype html><title>Test capsule</title>",
        )
        .unwrap();
        fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": format!("{name} test capsule"),
                "author": "elastos",
                "role": role,
                "type": "wasm",
                "entrypoint": format!("{name}.wasm"),
                "interfaces": interfaces
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn activate_home_cmd_test_capsule(data_dir: &Path, name: &str) {
        fs::create_dir_all(data_dir).unwrap();
        let path = data_dir.join("components.json");
        let mut components = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_else(|| {
                serde_json::json!({
                    "external": {},
                    "capsules": {},
                    "profiles": {}
                })
            });
        components["external"][name] = serde_json::json!({
            "install_path": format!("capsules/{name}"),
            "platforms": {}
        });
        fs::write(path, serde_json::to_vec_pretty(&components).unwrap()).unwrap();
    }

    fn write_home_cmd_active_shell_state(data_dir: &Path, principal_id: &str, active: &str) {
        let localhost_root = elastos_server::auth::principal_localhost_root(principal_id);
        let state_uri = format!("{localhost_root}/.AppData/ElastOS/Home/active-shell.json");
        let state_path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &state_uri)
            .expect("valid active shell state path");
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(
            state_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.home.active-shell/v1",
                "principal_id": principal_id,
                "localhost_root": localhost_root,
                "active": active
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn home_snapshot_uses_native_shell_catalog_and_interface_facts() {
        let _guard = HOME_CMD_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", temp.path().join("home"));
        let _xdg_data_home = ScopedEnvVar::set("XDG_DATA_HOME", temp.path().join("xdg"));
        let _coords = ScopedEnvVar::set(
            "ELASTOS_RUNTIME_COORDS_FILE",
            temp.path().join("missing-runtime-coords.json"),
        );
        let _owned =
            ScopedEnvVar::set(crate::runtime_control::GATEWAY_OWNED_HOME_TERMINAL_ENV, "0");
        let _gateway = ScopedEnvVar::set(
            elastos_server::api::gateway::HOME_CLI_GATEWAY_API_URL_ENV,
            "",
        );
        let data_dir = default_data_dir();
        write_home_cmd_test_capsule(&data_dir, "home-gui", "shell", Vec::new());
        write_home_cmd_test_capsule(
            &data_dir,
            "home-cli",
            "shell",
            vec![serde_json::json!({
                "id": "elastos.shell.cli",
                "version": "0.5.0",
                "methods": [{
                    "id": "facts.search",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "summary",
                    "resource": "elastos://home/facts",
                    "operation": "search"
                }]
            })],
        );
        write_home_cmd_active_shell_state(&data_dir, "person:test-home-cli", "home-cli");

        let snapshot = gather_snapshot().await.unwrap();

        assert_eq!(
            snapshot.active_shell["schema"],
            "elastos.home.active-shell/v1"
        );
        assert_eq!(snapshot.active_shell["active"], "home-gui");
        let shell_candidates = snapshot.active_shell["candidates"].as_array().unwrap();
        assert!(shell_candidates
            .iter()
            .any(|candidate| candidate["name"] == "home-cli"));
        assert!(shell_candidates
            .iter()
            .any(|candidate| candidate["name"] == "home-gui"));

        let catalog = snapshot.capsule_catalog.as_ref().unwrap();
        assert_eq!(catalog["schema"], "elastos.capsules.catalog/v1");
        let capsules = catalog["capsules"].as_array().unwrap();
        assert!(capsules
            .iter()
            .any(|capsule| capsule["name"] == "home-cli" && capsule["launchable"] == true));
        assert!(capsules
            .iter()
            .any(|capsule| capsule["name"] == "home-gui" && capsule["launchable"] == true));
        assert!(capsules.iter().all(|capsule| {
            capsule["name"] != "gba-emulator" && capsule["name"] != "gba-ucity"
        }));

        let interfaces = snapshot.capsule_interfaces.as_ref().unwrap();
        assert_eq!(interfaces["schema"], "elastos.capsules.interfaces/v1");
        assert!(interfaces["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["capsule"] == "home-cli"
                && entry["interface"]["id"] == "elastos.shell.cli"));

        assert_eq!(
            snapshot.services.as_ref().unwrap()["schema"],
            "elastos.runtime.services/v1"
        );
        assert_eq!(snapshot.people.contact_count, 0);
        assert_eq!(snapshot.notifications.unread_count, 0);
        assert!(!snapshot.room.room_slug.is_empty());
    }

    #[test]
    fn home_summary_projection_deserializes_gateway_home_facts() {
        let projection: HomeSummaryFactsProjection = serde_json::from_value(serde_json::json!({
            "runtime": {
                "running": true,
                "kind": "managed-home",
                "version": "0.5.0-dev",
                "api_url": "http://127.0.0.1:65097",
                "pid": 42,
                "running_capsules": [],
                "note": null
            },
            "active_shell": {
                "schema": "elastos.home.active-shell/v1",
                "active": "home-cli"
            },
            "people": {
                "schema": "elastos.people.contacts/v1",
                "contact_count": 1,
                "contacts": [{
                    "contact_id": "contact-a",
                    "display_name": "Anders",
                    "relationship": "accepted",
                    "route": "elastos://peer/contact-a",
                    "can_message": true
                }],
                "service_offer_count": 2,
                "service_offers": [{"offer_id": "offer-a"}, {"offer_id": "offer-b"}],
                "discovery": {
                    "schema": "elastos.people.discovery/v1",
                    "enabled": true,
                    "visibility": "trusted",
                    "status": "ready",
                    "status_message": "ready",
                    "topic": "__elastos_internal/people-discovery-v1"
                }
            },
            "services": {
                "schema": "elastos.runtime.services/v1",
                "local_offer_count": 1,
                "remote_offer_count": 1,
                "grant_model": "principal_scoped_provider_grant"
            },
            "notifications": {
                "unread_count": 1,
                "attention_count": 1,
                "entries": [{
                    "id": "notice-a",
                    "source_app": "wallet",
                    "kind": "wallet.approval",
                    "title": "Approve transaction",
                    "body": "Approve a pending request",
                    "severity": "attention",
                    "read": false,
                    "created_at": 1
                }]
            },
            "capsule_catalog": {
                "schema": "elastos.capsules.catalog/v1",
                "capsules": [{"name": "browser"}]
            },
            "capsule_interfaces": {
                "schema": "elastos.capsules.interfaces/v1",
                "interfaces": [{"capsule": "browser"}]
            },
            "targets": [{
                "target": "browser",
                "title": "Browser",
                "description": "Browse websites from this device.",
                "route": "/apps/browser/",
                "attach_kind": "iframe",
                "role": "app",
                "target_kind": "object",
                "viewer": "gba-emulator",
                "viewer_title": "GBA Emulator"
            }]
        }))
        .unwrap();

        assert_eq!(projection.people.contact_count, 1);
        assert_eq!(projection.people.service_offers.len(), 2);
        assert!(projection.runtime.as_ref().unwrap().running);
        assert_eq!(
            projection.active_shell.as_ref().unwrap()["active"],
            "home-cli"
        );
        assert_eq!(
            projection.capsule_catalog.as_ref().unwrap()["schema"],
            "elastos.capsules.catalog/v1"
        );
        assert_eq!(
            projection.capsule_interfaces.as_ref().unwrap()["schema"],
            "elastos.capsules.interfaces/v1"
        );
        assert_eq!(
            projection.services.unwrap()["schema"],
            "elastos.runtime.services/v1"
        );
        assert_eq!(projection.notifications.attention_count, 1);
        assert_eq!(projection.targets[0].target, "browser");
        assert_eq!(
            projection.targets[0].viewer.as_deref(),
            Some("gba-emulator")
        );
        assert_eq!(
            projection.notifications.entries[0].severity,
            "attention".to_string()
        );
    }

    #[test]
    fn notification_open_gui_refs_become_explicit_home_snapshot_actions() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot
            .notifications
            .entries
            .push(NotificationEntryStatus {
                id: "wallet-signing:tx-1".to_string(),
                source_app: "wallet".to_string(),
                kind: "wallet_signing_request".to_string(),
                title: "Wallet signature requested".to_string(),
                body: "ela.city wants Wallet to sign a transaction.".to_string(),
                action_ref: Some(NotificationActionRefStatus {
                    app: "wallet".to_string(),
                    action_id: "open-gui:wallet".to_string(),
                }),
                read: false,
                severity: "attention".to_string(),
            });

        let actions = gather_notification_host_actions(&snapshot);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "open-gui:wallet");
        assert_eq!(actions[0].command, "home: open Wallet");

        snapshot.actions.extend(actions);
        assert_eq!(
            home_terminal_host_intent_for_action("open-gui:wallet", &snapshot).unwrap(),
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "open-target",
                "action_id": "open-gui:wallet",
                "target": "wallet",
            })
        );
    }

    #[test]
    fn home_intent_accepts_structured_home_cli_invoke() {
        let intent: HomeIntent = serde_json::from_value(serde_json::json!({
            "action": "invoke",
            "invoke": {
                "capsule": "browser",
                "interface": "elastos.browser.page",
                "method": "page_status",
                "resource": "elastos://browser/page",
                "input": {}
            }
        }))
        .unwrap();
        let invoke = intent.invoke.expect("invoke payload");
        assert_eq!(intent.action, "invoke");
        assert_eq!(invoke.capsule, "browser");
        assert_eq!(invoke.interface_id, "elastos.browser.page");
        assert_eq!(invoke.method, "page_status");
        assert_eq!(invoke.resource, "elastos://browser/page");
    }

    #[test]
    fn people_actions_map_to_runtime_people_routes() {
        let cases = [
            (
                "people-discovery-enable",
                "/api/apps/people/discovery",
                serde_json::json!({ "enabled": true }),
            ),
            (
                "people-discovery-disable",
                "/api/apps/people/discovery",
                serde_json::json!({ "enabled": false }),
            ),
            (
                "people-discovery-refresh",
                "/api/apps/people/discovery/refresh",
                serde_json::json!({}),
            ),
            (
                "people-request-peer:peer-1",
                "/api/apps/people/discovery/requests",
                serde_json::json!({ "peer_id": "peer-1" }),
            ),
            (
                "people-accept-request:req 1",
                "/api/apps/people/discovery/requests/req%201/accept",
                serde_json::json!({}),
            ),
            (
                "people-remove-contact:contact-1",
                "/api/apps/people/contacts/remove",
                serde_json::json!({ "contact_id": "contact-1" }),
            ),
        ];

        for (action_id, path, body) in cases {
            let action = people_api_action(action_id)
                .unwrap_or_else(|| panic!("missing People action plan for {action_id}"))
                .unwrap();
            assert_eq!(action.path, path);
            assert_eq!(action.body, body);
        }
        assert!(people_api_action("people-message:contact-1").is_none());
        assert!(people_api_action("people-request-peer:")
            .expect("request action should parse")
            .is_err());
    }

    #[test]
    fn home_terminal_people_message_uses_runtime_contact_route() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot.people.contacts.push(PeopleContactStatus {
            contact_id: "contact-alice".to_string(),
            display_name: "Alice".to_string(),
            relationship: "conversation".to_string(),
            route: "/apps/chat-room/".to_string(),
            can_message: true,
            ..PeopleContactStatus::default()
        });

        let intent =
            home_terminal_host_intent_for_action("people-message:contact-alice", &snapshot)
                .unwrap();

        assert_eq!(
            intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "open-target",
                "action_id": "people-message:contact-alice",
                "target": "chat-room",
                "source": "people-contact",
                "contact_id": "contact-alice",
                "route": "/apps/chat-room/",
            })
        );

        snapshot.people.contacts[0].route = "elastos://peer/peer-alice".to_string();
        assert!(
            home_terminal_host_intent_for_action("people-message:contact-alice", &snapshot)
                .is_none()
        );
        snapshot.people.contacts[0].route = "/apps/chat-room/".to_string();
        snapshot.people.contacts[0].can_message = false;
        assert!(
            home_terminal_host_intent_for_action("people-message:contact-alice", &snapshot)
                .is_none()
        );
    }

    #[test]
    fn home_cli_invoke_notice_prefers_runtime_output_target() {
        let invoke = HomeInvokeIntent {
            capsule: "home-cli".to_string(),
            interface_id: "elastos.shell.cli".to_string(),
            method: "capsule.open".to_string(),
            resource: "elastos://capsules/*".to_string(),
            input: serde_json::json!({ "target": "browser" }),
        };
        let notice = format_home_cli_invoke_notice(
            &serde_json::json!({
                "status": "ok",
                "output": {
                    "target": "browser",
                    "route": "/apps/browser/"
                }
            }),
            &invoke,
        );
        assert_text_contains_all(
            "Home CLI invoke notice",
            &notice,
            &[
                "invoke: Runtime confirmed",
                "home-cli",
                "capsule.open",
                "browser",
            ],
        );
        assert!(!notice.contains("opened"));
        assert!(!notice.contains("completed"));
    }

    #[test]
    fn home_cli_requires_exact_invoke_result_binding() {
        let expected = elastos_server::esp_binding::esp_request_binding(
            "home-cli-invoke-test",
            "person:test",
            "marketplace",
            Some("elastos.marketplace.catalog"),
            "capsule.open",
            ["elastos://capsules/*".to_string()],
            &serde_json::json!({ "target": "browser" }),
        );
        let valid = serde_json::json!({
            "schema": "elastos.capsules.invoke-result/v1",
            "status": "ok",
            "request_id": expected.request_id,
            "capsule": expected.capsule,
            "interface": expected.interface,
            "method": expected.method,
            "request_binding": expected,
            "output": { "target": "browser", "route": "/apps/browser/" },
        });
        assert!(validate_home_cli_invoke_result(&valid, &expected).is_ok());

        for (field, replacement) in [
            (
                "schema",
                serde_json::json!("elastos.esp.request-binding/v999"),
            ),
            ("request_id", serde_json::json!("other-request")),
            ("principal", serde_json::json!("person:other")),
            ("capsule", serde_json::json!("browser")),
            ("interface", serde_json::json!("elastos.other")),
            ("method", serde_json::json!("catalog.list")),
            ("resources", serde_json::json!(["elastos://other/*"])),
            ("sha256", serde_json::json!("00".repeat(32))),
            ("bytes", serde_json::json!(999)),
            ("truncated", serde_json::json!(true)),
            ("preview", serde_json::json!({ "target": "other" })),
        ] {
            let mut mutated = valid.clone();
            mutated["request_binding"][field] = replacement;
            assert!(
                validate_home_cli_invoke_result(&mutated, &expected).is_err(),
                "accepted mutated {field} binding"
            );
        }

        for (field, replacement) in [
            ("request_id", serde_json::json!("other-request")),
            ("capsule", serde_json::json!("browser")),
            ("interface", serde_json::json!("elastos.other")),
            ("method", serde_json::json!("catalog.list")),
        ] {
            let mut mutated = valid.clone();
            mutated[field] = replacement;
            assert!(
                validate_home_cli_invoke_result(&mutated, &expected).is_err(),
                "accepted mutated top-level {field}"
            );
        }

        let mut http_only = valid.clone();
        http_only.as_object_mut().unwrap().remove("request_binding");
        assert!(validate_home_cli_invoke_result(&http_only, &expected).is_err());
        let mut missing_output = valid.clone();
        missing_output.as_object_mut().unwrap().remove("output");
        assert!(validate_home_cli_invoke_result(&missing_output, &expected).is_err());
    }

    #[test]
    fn gateway_home_terminal_capsule_actions_do_not_become_host_open_target_intents_by_default() {
        let snapshot = sample_snapshot_with_components(&[]);
        assert!(home_terminal_host_intent_for_action("chat", &snapshot).is_none());
        assert!(home_terminal_host_intent_for_action("capsule-browser", &snapshot).is_none());
        assert!(home_terminal_host_intent_for_action("capsule-gba-ucity", &snapshot).is_none());
        assert!(home_terminal_host_intent_for_action("capsule-home-cli", &snapshot).is_none());
        assert!(home_terminal_host_intent_for_action("open-gui:browser", &snapshot).is_none());

        let mut snapshot_with_open = snapshot.clone();
        snapshot_with_open.actions.push(ActionInfo {
            id: "open-gui:browser".to_string(),
            label: "Open Browser".to_string(),
            description: "Open Browser from a server-issued Home action.".to_string(),
            command: "home: open Browser".to_string(),
            ready: true,
            reason: None,
        });
        let intent =
            home_terminal_host_intent_for_action("open-gui:browser", &snapshot_with_open).unwrap();

        assert_eq!(
            intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "open-target",
                "action_id": "open-gui:browser",
                "target": "browser",
            })
        );
        for target in ["wallet", "system", "documents"] {
            let mut snapshot_with_open = snapshot.clone();
            let action_id = format!("open-gui:{target}");
            snapshot_with_open.actions.push(ActionInfo {
                id: action_id.clone(),
                label: format!("Open {target}"),
                description: "Open target from a server-issued Home action.".to_string(),
                command: format!("home: open {target}"),
                ready: true,
                reason: None,
            });
            let intent =
                home_terminal_host_intent_for_action(&action_id, &snapshot_with_open).unwrap();
            assert_eq!(
                intent,
                serde_json::json!({
                    "schema": "elastos.home.terminal-host-intent/v1",
                    "action": "open-target",
                    "action_id": action_id,
                    "target": target,
                })
            );
        }
        assert!(home_terminal_host_intent_for_action("open-gui:home-cli", &snapshot).is_none());

        let shell_intent =
            home_terminal_host_intent_for_action("shell-switch:home-gui", &snapshot).unwrap();
        assert_eq!(
            shell_intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "active-shell",
                "action_id": "shell-switch:home-gui",
                "target": "home-gui",
            })
        );
        assert!(home_terminal_host_intent_for_action("shell-switch:home-cli", &snapshot).is_none());
        assert!(home_terminal_host_intent_for_action("shell-switch:browser", &snapshot).is_none());

        let mut browser_snapshot = snapshot.clone();
        browser_snapshot.session.mode = "browser_pty".to_string();
        let sign_out_intent =
            home_terminal_host_intent_for_action("auth-sign-out", &browser_snapshot).unwrap();
        assert_eq!(
            sign_out_intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "sign-out",
                "action_id": "auth-sign-out",
                "target": "home",
            })
        );
        let mut native_snapshot = browser_snapshot;
        native_snapshot.session.mode = "native_terminal".to_string();
        assert!(home_terminal_host_intent_for_action("auth-sign-out", &native_snapshot).is_none());
    }

    #[test]
    fn dynamic_capsule_actions_use_only_canonical_cli_projection() {
        let catalog = serde_json::json!({
            "capsules": [
                {
                    "name": "chat",
                    "title": "Chat",
                    "description": "Chat from Home CLI.",
                    "role": "app",
                    "projection": { "cli": { "state": "available" } }
                },
                {
                    "name": "browser",
                    "title": "Browser",
                    "description": "Graphical browser.",
                    "role": "app",
                    "projection": { "cli": { "state": "facts-only" } }
                },
                {
                    "name": "calendar-provider",
                    "title": "Calendar",
                    "description": "Calendar service.",
                    "role": "provider",
                    "projection": { "cli": { "state": "available" } }
                },
                {
                    "name": "home-cli",
                    "title": "Home CLI",
                    "description": "Command shell.",
                    "role": "shell",
                    "projection": { "cli": { "state": "available" } }
                }
            ]
        });

        let actions = gather_capsule_actions(&catalog);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "capsule-chat");
        assert_eq!(actions[0].label, "Chat");
    }

    #[cfg(unix)]
    #[test]
    fn gateway_terminal_parent_watchdog_detects_reparenting() {
        assert!(!gateway_terminal_parent_lost(42, 42));
        assert!(gateway_terminal_parent_lost(42, 1));
        assert!(gateway_terminal_parent_lost(42, 99));
        assert!(gateway_terminal_parent_lost(1, 1));
    }

    #[test]
    fn chat_action_stays_native_when_fullscreen_chat_is_not_packaged() {
        let snapshot = sample_snapshot_with_components(&[
            "shell",
            "localhost-provider",
            "did-provider",
            "crosvm",
            "vmlinux",
        ]);
        let action = action_spec("chat").unwrap();

        assert_eq!(
            action_command_with_kvm(action, &snapshot, true),
            "home: open Chat"
        );
    }

    #[test]
    fn chat_action_stays_native_even_when_fullscreen_chat_prereqs_are_present() {
        let snapshot = sample_snapshot_with_components(&[
            "shell",
            "localhost-provider",
            "did-provider",
            "chat",
            "crosvm",
            "vmlinux",
        ]);
        let action = action_spec("chat").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, true),
            ActionLaunch::ManagedChat
        );
        assert_eq!(
            action_command_with_kvm(action, &snapshot, true),
            "home: open Chat"
        );
    }

    #[test]
    fn chat_action_stays_native_when_focus_chat_missing() {
        let snapshot =
            sample_snapshot_with_components(&["shell", "localhost-provider", "did-provider"]);
        let action = action_spec("chat").unwrap();

        assert_eq!(
            action_command_with_kvm(action, &snapshot, false),
            "home: open Chat"
        );
    }

    #[test]
    fn chat_action_launch_uses_managed_native_when_fullscreen_chat_is_not_packaged() {
        let snapshot = sample_snapshot_with_components(&[
            "shell",
            "localhost-provider",
            "did-provider",
            "crosvm",
            "vmlinux",
        ]);
        let action = action_spec("chat").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, true),
            ActionLaunch::ManagedChat
        );
    }

    #[test]
    fn chat_action_launch_uses_managed_native_when_focus_chat_missing() {
        let snapshot =
            sample_snapshot_with_components(&["shell", "localhost-provider", "did-provider"]);
        let action = action_spec("chat").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, false),
            ActionLaunch::ManagedChat
        );
    }

    #[test]
    fn shares_action_uses_managed_summary() {
        let snapshot = sample_snapshot_with_components(&[]);
        let action = action_spec("shares-list").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, false),
            ActionLaunch::ManagedSharesList
        );
    }

    #[test]
    fn site_local_action_uses_managed_preview() {
        let mut snapshot = sample_snapshot_with_components(&["site-provider"]);
        snapshot.site.staged = true;
        let action = action_spec("site-local").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, false),
            ActionLaunch::ManagedLocalSitePreview
        );
        assert_eq!(
            action_command(action, &snapshot),
            "home: start MyWebSite local preview"
        );
        assert!(!action_command(action, &snapshot).contains("browser"));
    }

    #[test]
    fn site_open_is_the_explicit_browser_open_action() {
        let mut snapshot = sample_snapshot_with_components(&["site-provider"]);
        snapshot.site.staged = true;
        let action = action_spec("site-open").unwrap();

        assert_eq!(
            action_launch_with_kvm(action, &snapshot, false),
            ActionLaunch::ManagedLocalSiteOpen
        );
        assert_eq!(
            action_command(action, &snapshot),
            "home: open MyWebSite preview in browser"
        );
    }

    #[test]
    fn core_actions_are_always_visible() {
        let core_ids: Vec<&str> = CORE_ACTIONS
            .iter()
            .filter(|a| a.core)
            .map(|a| a.id)
            .collect();
        assert!(core_ids.contains(&"identity-nickname-set"));
        assert!(core_ids.contains(&"chat"));
        assert!(core_ids.contains(&"site-local"));
        assert!(core_ids.contains(&"shares-list"));
        assert!(!core_ids.contains(&"update-check"));
    }

    #[test]
    fn conditional_actions_stay_hidden_when_blocked() {
        let non_core: Vec<&str> = CORE_ACTIONS
            .iter()
            .filter(|a| !a.core)
            .map(|a| a.id)
            .collect();
        assert!(non_core.contains(&"site-ephemeral"));
        assert!(non_core.contains(&"site-open"));
        assert!(non_core.contains(&"room-approve"));
        assert!(non_core.contains(&"room-deny"));
        assert!(non_core.contains(&"room-revoke-all"));
    }

    #[test]
    fn room_actions_require_pending_browser_access_request() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        assert!(matches!(
            action_readiness("room-approve", &snapshot),
            ActionReadiness::Blocked(_)
        ));

        snapshot.room.pending_count = 1;
        assert!(matches!(
            action_readiness("room-approve", &snapshot),
            ActionReadiness::Ready
        ));
    }

    #[test]
    fn room_revoke_requires_active_sessions() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        assert!(matches!(
            action_readiness("room-revoke-all", &snapshot),
            ActionReadiness::Blocked(_)
        ));

        snapshot.room.active_session_count = 1;
        assert!(matches!(
            action_readiness("room-revoke-all", &snapshot),
            ActionReadiness::Ready
        ));
    }

    #[test]
    fn room_policy_actions_require_admin_role() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot.room.local_runtime_role = Some("member".to_string());
        assert!(matches!(
            action_readiness("room-policy-toggle-guests", &snapshot),
            ActionReadiness::Blocked(_)
        ));

        snapshot.room.local_runtime_role = Some("owner".to_string());
        assert!(matches!(
            action_readiness("room-policy-toggle-guests", &snapshot),
            ActionReadiness::Ready
        ));
    }

    #[test]
    fn room_policy_actions_surface_for_room_admins() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot.room.local_runtime_role = Some("owner".to_string());
        snapshot.room.allow_guest_invites = true;
        snapshot.room.allow_member_invites = false;
        snapshot.room.allow_members_to_host_guests = true;

        let labels: Vec<String> = gather_room_actions(&snapshot)
            .into_iter()
            .map(|action| action.label)
            .collect();
        assert!(labels.contains(&"Close public join requests".to_string()));
        assert!(labels.contains(&"Open ElastOS user invites".to_string()));
        assert!(labels.contains(&"Restrict web guest approvals".to_string()));
    }

    #[test]
    fn blocked_local_site_notice_explains_stage_step() {
        let snapshot = sample_snapshot_with_components(&[]);
        let notice = render_site_local_blocked_notice(
            &snapshot,
            "stage a site first with `elastos site stage <dir>`",
        );
        assert_text_contains_all(
            "empty MyWebSite notice",
            &notice,
            &[
                "MyWebSite is empty",
                "elastos site stage <dir>",
                "preview",
                "go public",
            ],
        );
    }

    #[test]
    fn blocked_local_site_notice_explains_preview_prereq() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot.site.staged = true;
        let notice = render_site_local_blocked_notice(
            &snapshot,
            "missing site-provider -- run: elastos setup --profile demo",
        );
        assert_text_contains_all(
            "staged MyWebSite notice",
            &notice,
            &[
                "MyWebSite is staged",
                "localhost://MyWebSite",
                "elastos setup --profile demo",
                "site-provider",
            ],
        );
    }
}
