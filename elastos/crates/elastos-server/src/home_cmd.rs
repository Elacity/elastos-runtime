use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, IsTerminal, Write};
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
const GATEWAY_OWNED_HOME_TERMINAL_ENV: &str = "ELASTOS_GATEWAY_OWNED_HOME_TERMINAL";
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

/// Names of capsules that are service providers, not user-launchable apps.
/// These are hidden from the Home launch list even when installed.
const PROVIDER_CAPSULE_NAMES: &[&str] = &[
    "shell",
    "localhost-provider",
    "did-provider",
    "chain-provider",
    "wallet-provider",
    "object-provider",
    "drm-provider",
    "rights-provider",
    "key-provider",
    "decrypt-provider",
    "ipfs-provider",
    "tunnel-provider",
    "site-provider",
    "ai-provider",
    "llama-provider",
    "webspace-provider",
    "home",
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
    discovery: PeopleDiscoveryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PeopleProfileCardStatus {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
struct HomeSummaryPeopleProjection {
    #[serde(default)]
    people: PeopleStatus,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeStatus {
    running: bool,
    kind: Option<String>,
    version: Option<String>,
    api_url: Option<String>,
    pid: Option<u32>,
    peer_count: Option<usize>,
    ticket: Option<String>,
    running_capsules: Vec<String>,
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
            "quit" => break Ok(()),
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
                if emit_gateway_home_terminal_host_intent(action_id)? {
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
    std::env::var_os(GATEWAY_OWNED_HOME_TERMINAL_ENV).as_deref() == Some(OsStr::new("1"))
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

fn home_terminal_host_intent_for_action(action_id: &str) -> Option<serde_json::Value> {
    if let Some(target) = action_id.strip_prefix("open-gui:").map(str::trim) {
        if target.is_empty() || target == HOME_CLI_CAPSULE_NAME {
            return None;
        }
        return Some(serde_json::json!({
            "schema": "elastos.home.terminal-host-intent/v1",
            "action": "open-target",
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
        "target": "home-gui",
    }))
}

fn emit_gateway_home_terminal_host_intent(action_id: &str) -> anyhow::Result<bool> {
    if !gateway_owned_home_terminal() {
        return Ok(false);
    }
    let Some(intent) = home_terminal_host_intent_for_action(action_id) else {
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
    let token = elastos_server::api::gateway::issue_local_runtime_home_launch_token(
        data_dir,
        &invoke.capsule,
    )?;
    let url = format!(
        "{}/api/capsules/interfaces/invoke",
        access.api_url.trim_end_matches('/')
    );
    let response = access
        .client
        .post(url)
        .header("x-elastos-home-token", token)
        .json(&invoke)
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
    Ok(format_home_cli_invoke_notice(&body, &invoke))
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
        "invoke: ok {} {}{}",
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
    let runtime = gather_runtime_status(&data_dir).await;
    let people = gather_people_status(&data_dir, runtime.api_url.as_deref()).await;
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
        services: Some(elastos_server::api::gateway::home_services_snapshot(
            &data_dir,
        )),
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
        notifications: NotificationStatus {
            unread_count: notification_summary.unread_count,
            attention_count: notification_summary.attention_count,
            entries: notification_summary
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
                        elastos_server::notifications::NotificationSeverity::Attention => {
                            "attention"
                        }
                        elastos_server::notifications::NotificationSeverity::Critical => "critical",
                    }
                    .to_string(),
                })
                .collect(),
        },
        roots: gather_roots(&data_dir),
        components: gather_components(&data_dir),
        cached_capsules: gather_cached_capsules(&data_dir),
        capsule_catalog: None,
        capsule_interfaces: None,
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

    // Dynamically discover installed capsules and add launchable ones.
    snapshot.actions.extend(gather_capsule_actions(&data_dir));
    snapshot.actions.extend(gather_room_actions(&snapshot));

    Ok(snapshot)
}

fn gather_home_cli_session_status() -> HomeCliSessionStatus {
    if gateway_owned_home_terminal() {
        HomeCliSessionStatus {
            mode: "browser_pty".to_string(),
            passkey_state: "launch-token authorized browser Home session".to_string(),
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
    let runtime_storage = data_dir
        .join("Local")
        .join("Shared")
        .join("Home")
        .join("bootstrap-storage");
    fs::create_dir_all(&runtime_storage)?;

    let runtime = crate::create_runtime(&runtime_storage).await?;
    let api_url = api_url.to_string();
    let client_token = client_token.to_string();

    runtime.set_wasm_bridge_spawner(std::sync::Arc::new(move |pipes| {
        elastos_server::carrier_bridge::spawn_wasm_api_bridge(
            pipes,
            api_url.clone(),
            client_token.clone(),
        );
    }));

    let mut scoped_env = Vec::new();
    // The Home capsule owns startup-input settle logic for the front-door path.
    // Do not pre-flush stdin here, or Home and chat end up competing over input repair.
    let raw_mode = runtime_control::enable_host_raw_mode_pub();
    if raw_mode.is_some() {
        if let Some((cols, rows)) = current_terminal_size() {
            scoped_env.push(ScopedEnvVar::set("ELASTOS_TERM_COLS", cols.to_string()));
            scoped_env.push(ScopedEnvVar::set("ELASTOS_TERM_ROWS", rows.to_string()));
        }
        scoped_env.push(ScopedEnvVar::set("ELASTOS_HOME_TUI", "1"));
    } else {
        scoped_env.push(ScopedEnvVar::set("ELASTOS_HOME_TUI", "0"));
        if home_debug_tty() {
            eprintln!(
                "[home-tty] raw mode unavailable (stdin_tty={} stdout_tty={}); falling back to line dashboard",
                std::io::stdin().is_terminal(),
                std::io::stdout().is_terminal(),
            );
        }
    }
    let _saved_termios = raw_mode;

    runtime
        .run_local(&capsule_dir, vec![session.uri_root.clone()])
        .await
        .map_err(|e| anyhow::anyhow!("Home WASM dashboard failed: {}", e))?;

    Ok(())
}

fn resolve_home_capsule_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let dev = source_capsule_dir(HOME_CLI_CAPSULE_NAME);
    let dev_target = dev
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("home-cli.wasm");
    let dev_entry = dev.join("home-cli.wasm");
    if dev_target.is_file() {
        fs::copy(&dev_target, &dev_entry).with_context(|| {
            format!(
                "failed to stage local Home WASM artifact from {}",
                dev_target.display()
            )
        })?;
    }
    if dev.join("capsule.json").is_file()
        && dev.join("home-cli.wasm").is_file()
        && prefer_dev_home_capsule()
    {
        return Ok(dev);
    }

    let installed = data_dir.join("capsules").join(HOME_CLI_CAPSULE_NAME);
    if installed.join("capsule.json").is_file() && installed.join("home-cli.wasm").is_file() {
        return Ok(installed);
    }

    if dev.join("capsule.json").is_file() && dev.join("home-cli.wasm").is_file() {
        return Ok(dev);
    }

    if prefer_dev_home_capsule() {
        anyhow::bail!(
            "home capsule not built yet.\n\nBuild it first:\n\n  cd {}\n  cargo build --target wasm32-wasip1 --release\n\nOr install the published Home surface with:\n\n  elastos setup",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../capsules")
                .join(HOME_CLI_CAPSULE_NAME)
                .display()
        );
    }

    anyhow::bail!("Home is not installed yet.\n\nRun:\n\n  elastos setup");
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
        return dispatch_capsule_action(capsule_name, dashboard).await;
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
    let token = elastos_server::api::gateway::issue_local_runtime_home_launch_token(
        &data_dir,
        HOME_CLI_CAPSULE_NAME,
    )?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!(
        "{}/{}",
        coords.api_url.trim_end_matches('/'),
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
    dashboard: &mut DashboardContext,
) -> anyhow::Result<String> {
    if capsule_name == HOME_CLI_CAPSULE_NAME {
        return Ok("Home CLI is already active.".to_string());
    }
    let data_dir = default_data_dir();
    let Some(manifest) = load_capsule_manifest(&data_dir, capsule_name)? else {
        anyhow::bail!("Capsule {} is not installed.", capsule_name);
    };
    match capsule_launch_plan(&manifest) {
        CapsuleLaunchPlan::CliCapability => run_capsule_action(capsule_name, dashboard).await,
        CapsuleLaunchPlan::TypedAffordance {
            interface_id,
            method_id,
        } => Ok(format!(
            "{} exposes typed affordance {}.{}; use `invoke {} {} ...` from Home CLI. It was not opened through home-gui.",
            manifest.name, interface_id, method_id, manifest.name, method_id
        )),
        CapsuleLaunchPlan::ApprovalRequest {
            interface_id,
            method_id,
        } => Ok(format!(
            "{} requires approval for {}.{} before launch. Review the request through Inbox/Wallet or an explicit approval flow; Home CLI did not switch to home-gui.",
            manifest.name, interface_id, method_id
        )),
        CapsuleLaunchPlan::GuiOnlyExplicitOpen => Ok(format!(
            "{} is GUI-only from Home CLI. Switch to home-gui and open it there, or use an explicit `open-gui:{}` host action when one is offered.",
            manifest.name, manifest.name
        )),
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CapsuleLaunchPlan {
    CliCapability,
    TypedAffordance {
        interface_id: String,
        method_id: String,
    },
    ApprovalRequest {
        interface_id: String,
        method_id: String,
    },
    GuiOnlyExplicitOpen,
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

fn load_capsule_manifest(
    data_dir: &Path,
    capsule_name: &str,
) -> anyhow::Result<Option<elastos_common::CapsuleManifest>> {
    let manifest_path = data_dir
        .join("capsules")
        .join(capsule_name)
        .join("capsule.json");
    let Ok(bytes) = fs::read(&manifest_path) else {
        return Ok(None);
    };
    let manifest: elastos_common::CapsuleManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    manifest
        .validate()
        .map_err(|err| anyhow::anyhow!("invalid {}: {}", manifest_path.display(), err))?;
    Ok(Some(manifest))
}

fn capsule_launch_plan(manifest: &elastos_common::CapsuleManifest) -> CapsuleLaunchPlan {
    if capsule_has_cli_capability(manifest) {
        return CapsuleLaunchPlan::CliCapability;
    }
    if let Some((interface_id, method_id)) = first_runtime_policy_affordance(manifest) {
        return CapsuleLaunchPlan::TypedAffordance {
            interface_id,
            method_id,
        };
    }
    if let Some((interface_id, method_id)) = first_approval_affordance(manifest) {
        return CapsuleLaunchPlan::ApprovalRequest {
            interface_id,
            method_id,
        };
    }
    CapsuleLaunchPlan::GuiOnlyExplicitOpen
}

fn capsule_has_cli_capability(manifest: &elastos_common::CapsuleManifest) -> bool {
    manifest.interfaces.iter().any(|interface| {
        interface.id.contains(".terminal")
            || interface.id.ends_with(".cli")
            || interface.methods.iter().any(|method| {
                method.id == "session.open"
                    || method.id == "terminal.open"
                    || method.id == "cli.open"
            })
    })
}

fn first_runtime_policy_affordance(
    manifest: &elastos_common::CapsuleManifest,
) -> Option<(String, String)> {
    first_matching_affordance(manifest, |method| {
        matches!(
            method.approval,
            elastos_common::AffordanceApprovalMode::None
                | elastos_common::AffordanceApprovalMode::RuntimePolicy
        )
    })
}

fn first_approval_affordance(
    manifest: &elastos_common::CapsuleManifest,
) -> Option<(String, String)> {
    first_matching_affordance(manifest, |method| {
        matches!(
            method.approval,
            elastos_common::AffordanceApprovalMode::User
        )
    })
}

fn first_matching_affordance<F>(
    manifest: &elastos_common::CapsuleManifest,
    predicate: F,
) -> Option<(String, String)>
where
    F: Fn(&elastos_common::CapsuleAffordanceDescriptor) -> bool,
{
    for interface in &manifest.interfaces {
        for method in &interface.methods {
            if predicate(method) {
                return Some((interface.id.clone(), method.id.clone()));
            }
        }
    }
    None
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
        parts.join(" · ")
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

fn current_terminal_size() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ok == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

fn home_debug_tty() -> bool {
    matches!(
        std::env::var("ELASTOS_HOME_DEBUG_TTY").ok().as_deref(),
        Some("1" | "true" | "yes")
    )
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
/// Reads `capsule.json` from each directory under `<data_dir>/capsules/`.
/// Providers and internal capsules are excluded via `PROVIDER_CAPSULE_NAMES`.
fn gather_capsule_actions(data_dir: &Path) -> Vec<ActionInfo> {
    let cache_dir = data_dir.join("capsules");
    let Ok(read_dir) = fs::read_dir(&cache_dir) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    for entry in read_dir.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("capsule.json");
        let Ok(bytes) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<elastos_common::CapsuleManifest>(&bytes) else {
            continue;
        };
        if manifest.validate().is_err() {
            continue;
        }
        let name = manifest.name.clone();
        // Skip providers and internal capsules.
        if PROVIDER_CAPSULE_NAMES.contains(&name.as_str()) {
            continue;
        }
        let description = manifest.description.clone().unwrap_or_default();
        let command = format!(
            "elastos capsule {} --lifecycle interactive --interactive",
            name
        );
        actions.push(ActionInfo {
            id: format!("capsule-{}", name),
            label: name,
            description,
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
        "execute",
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

async fn gather_people_status(data_dir: &Path, api_url: Option<&str>) -> PeopleStatus {
    let Some(api_url) = api_url else {
        return PeopleStatus::default();
    };
    let token = match elastos_server::api::gateway::issue_local_runtime_home_launch_token(
        data_dir,
        HOME_CLI_CAPSULE_NAME,
    ) {
        Ok(token) => token,
        Err(_) => return PeopleStatus::default(),
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return PeopleStatus::default(),
    };
    let url = format!("{}/api/apps/home/summary", api_url.trim_end_matches('/'));
    let response = match client
        .get(url)
        .header("x-elastos-home-token", token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return PeopleStatus::default(),
    };
    if !response.status().is_success() {
        return PeopleStatus::default();
    }
    response
        .json::<HomeSummaryPeopleProjection>()
        .await
        .map(|projection| projection.people)
        .unwrap_or_default()
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
        ActionReadiness::Blocked(format!("missing {} — {}", missing.join(", "), hint))
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

    #[test]
    fn home_intent_accepts_structured_home_cli_invoke() {
        let intent: HomeIntent = serde_json::from_value(serde_json::json!({
            "action": "invoke",
            "invoke": {
                "capsule": "browser",
                "interface": "elastos.browser.page",
                "method": "page_status",
                "input": {}
            }
        }))
        .unwrap();
        let invoke = intent.invoke.expect("invoke payload");
        assert_eq!(intent.action, "invoke");
        assert_eq!(invoke.capsule, "browser");
        assert_eq!(invoke.interface_id, "elastos.browser.page");
        assert_eq!(invoke.method, "page_status");
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
    fn home_cli_invoke_notice_prefers_runtime_output_target() {
        let invoke = HomeInvokeIntent {
            capsule: "home-cli".to_string(),
            interface_id: "elastos.shell.cli".to_string(),
            method: "capsule.open".to_string(),
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
        assert_eq!(notice, "invoke: ok home-cli capsule.open -> browser");
    }

    #[test]
    fn gateway_home_terminal_capsule_actions_do_not_become_host_open_target_intents_by_default() {
        assert!(home_terminal_host_intent_for_action("chat").is_none());
        assert!(home_terminal_host_intent_for_action("capsule-browser").is_none());
        assert!(home_terminal_host_intent_for_action("capsule-gba-ucity").is_none());
        assert!(home_terminal_host_intent_for_action("capsule-home-cli").is_none());

        let intent = home_terminal_host_intent_for_action("open-gui:browser").unwrap();

        assert_eq!(
            intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "open-target",
                "target": "browser",
            })
        );
        for target in ["wallet", "system", "documents"] {
            let action_id = format!("open-gui:{target}");
            let intent = home_terminal_host_intent_for_action(&action_id).unwrap();
            assert_eq!(
                intent,
                serde_json::json!({
                    "schema": "elastos.home.terminal-host-intent/v1",
                    "action": "open-target",
                    "target": target,
                })
            );
        }
        assert!(home_terminal_host_intent_for_action("open-gui:home-cli").is_none());

        let shell_intent = home_terminal_host_intent_for_action("shell-switch:home-gui").unwrap();
        assert_eq!(
            shell_intent,
            serde_json::json!({
                "schema": "elastos.home.terminal-host-intent/v1",
                "action": "active-shell",
                "target": "home-gui",
            })
        );
        assert!(home_terminal_host_intent_for_action("shell-switch:home-cli").is_none());
        assert!(home_terminal_host_intent_for_action("shell-switch:browser").is_none());
    }

    #[test]
    fn capsule_launch_matrix_prefers_cli_then_affordance_then_approval_then_explicit_gui() {
        let chat = manifest_from_json(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": "chat-wasm",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "chat.wasm",
            "interfaces": [{
                "id": "elastos.chat.terminal",
                "version": "0.5.0",
                "methods": [{
                    "id": "session.open",
                    "risk": "launch",
                    "approval": "runtime_policy",
                    "audit": "event"
                }]
            }]
        }));
        assert_eq!(capsule_launch_plan(&chat), CapsuleLaunchPlan::CliCapability);

        let browser = manifest_from_json(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": "browser",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "browser.wasm",
            "interfaces": [{
                "id": "elastos.browser.page",
                "version": "0.5.0",
                "methods": [{
                    "id": "page.open",
                    "risk": "launch",
                    "approval": "runtime_policy",
                    "audit": "event"
                }]
            }]
        }));
        assert_eq!(
            capsule_launch_plan(&browser),
            CapsuleLaunchPlan::TypedAffordance {
                interface_id: "elastos.browser.page".to_string(),
                method_id: "page.open".to_string(),
            }
        );

        let wallet_connector = manifest_from_json(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": "wallet-metamask",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "wallet-metamask.wasm",
            "interfaces": [{
                "id": "elastos.wallet.approval-method",
                "version": "0.5.0",
                "methods": [{
                    "id": "method.connect",
                    "risk": "privileged",
                    "approval": "user",
                    "audit": "event"
                }]
            }]
        }));
        assert_eq!(
            capsule_launch_plan(&wallet_connector),
            CapsuleLaunchPlan::ApprovalRequest {
                interface_id: "elastos.wallet.approval-method".to_string(),
                method_id: "method.connect".to_string(),
            }
        );

        let gui_only = manifest_from_json(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": "paint",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "paint.wasm"
        }));
        assert_eq!(
            capsule_launch_plan(&gui_only),
            CapsuleLaunchPlan::GuiOnlyExplicitOpen
        );
    }

    fn manifest_from_json(value: serde_json::Value) -> elastos_common::CapsuleManifest {
        let manifest: elastos_common::CapsuleManifest = serde_json::from_value(value).unwrap();
        manifest.validate().unwrap();
        manifest
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
    fn chat_action_falls_back_to_native_when_focus_chat_missing() {
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
        assert_eq!(
            render_site_local_blocked_notice(
                &snapshot,
                "stage a site first with `elastos site stage <dir>`",
            ),
            "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`. Then reopen MyWebSite from Home to preview or go public."
        );
    }

    #[test]
    fn blocked_local_site_notice_explains_preview_prereq() {
        let mut snapshot = sample_snapshot_with_components(&[]);
        snapshot.site.staged = true;
        assert_eq!(
            render_site_local_blocked_notice(
                &snapshot,
                "missing site-provider — run: elastos setup --profile demo",
            ),
            "MyWebSite is staged at localhost://MyWebSite. Run `elastos setup --profile demo` to install site-provider, then reopen MyWebSite from Home."
        );
    }

    #[test]
    fn provider_capsules_excluded_from_dynamic_actions() {
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"shell"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"did-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"chain-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"wallet-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"object-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"drm-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"rights-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"key-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"decrypt-provider"));
        assert!(PROVIDER_CAPSULE_NAMES.contains(&"home"));
    }
}
