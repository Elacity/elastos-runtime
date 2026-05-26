//! Public ElastOS edge for site roots, publisher objects, and CID content.
//!
//! Owns the browser-facing HTTP application routes and resolves them from
//! runtime-owned state (`MyWebSite`, `ElastOS/SystemServices/Publisher`, and
//! `ElastOS/SystemServices/Edge`) plus read-only CID content.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::documents::DocumentsClient;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{
    header::{AUTHORIZATION, COOKIE, SET_COOKIE},
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use base64::Engine as _;
use elastos_common::localhost::{
    edge_binding_path, edge_site_head_path, my_website_root_path, publisher_artifacts_path,
    publisher_install_script_path, publisher_release_head_path, publisher_release_manifest_path,
    publisher_site_releases_dir, rooted_localhost_fs_path, MY_WEBSITE_URI,
};
use elastos_common::{CapsuleRole, CapsuleType};
use elastos_runtime::provider::ProviderRegistry;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use url::form_urlencoded;

/// Maximum size for a single file fetched through the gateway (100 MB).
const MAX_GATEWAY_FILE_SIZE: usize = 100 * 1024 * 1024;
const GATEWAY_VERSION: &str = env!("ELASTOS_VERSION");
pub(crate) const ROOM_SESSION_COOKIE: &str = "room-session";
pub(crate) const BROWSER_SESSION_COOKIE: &str = "browser-session";
pub(crate) const HOME_SESSION_COOKIE: &str = "home-session";
const ROOM_SYNC_CONSUMER_ID: &str = "room-sync";
const HOME_LAUNCH_TOKEN_DOMAIN: &str = "elastos.home.launch.v1";
const HOME_LAUNCH_TOKEN_TTL_SECS: u64 = 12 * 60 * 60;
const DOCUMENTS_CAPSULE_ID: &str = "documents";
const LIBRARY_CAPSULE_ID: &str = "library";
const INBOX_CAPSULE_ID: &str = "inbox";
const SYSTEM_CAPSULE_ID: &str = "system";
const SYSTEM_ROUTE: &str = "/apps/system/";
const CHAT_ROOM_CAPSULE_ID: &str = "chat-room";
pub(crate) const HOME_CAPSULE_ID: &str = "home";
const HOME_ROUTE: &str = "/apps/home/";

#[derive(Clone)]
pub struct GatewayState {
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub cache_dir: PathBuf,
    /// Runtime data directory backing rooted Publisher/Edge/MyWebSite state.
    pub data_dir: PathBuf,
}

pub fn gateway_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(serve_public_root))
        .route("/healthz", get(healthz))
        .route(
            "/api/browser/session/request",
            post(super::browser_sessions::browser_session_request),
        )
        .route(
            "/api/browser/session/request/:request_id",
            get(super::browser_sessions::browser_session_request_status),
        )
        .route("/api/provider/:scheme/:op", post(gateway_provider_proxy))
        .route("/release.json", get(serve_release_manifest))
        .route("/release-head.json", get(serve_release_head))
        .route("/install.sh", get(serve_install_script))
        .route(
            "/.well-known/elastos/site-head.json",
            get(serve_site_head_document),
        )
        .route("/artifacts/*path", get(serve_artifact_file))
        .route("/api/apps/system/summary", get(system_summary))
        .route(
            "/api/apps/system/identity/handle",
            post(system_handle_update),
        )
        .route(
            "/api/apps/system/appearance/background-image",
            post(system_background_image_update)
                .delete(system_background_image_reset)
                .layer(DefaultBodyLimit::max(
                    HOME_BACKGROUND_IMAGE_TRANSPORT_MAX_BYTES,
                )),
        )
        .route(
            "/api/apps/system/appearance/background-overlay",
            post(system_background_overlay_update),
        )
        .route("/api/apps/home/summary", get(home_summary))
        .route(
            "/api/apps/home/appearance/background-image",
            get(home_background_image),
        )
        .route("/api/apps/home/runtime/ensure", post(home_runtime_ensure))
        .route("/api/apps/home/launch", post(home_launch))
        .route("/api/apps/inbox/summary", get(inbox_summary))
        .route("/api/apps/inbox/actions", post(inbox_action))
        .route("/api/apps/chat-room/summary", get(chat_room_summary))
        .route(
            "/api/apps/chat-room/requests/:request_id/approve",
            post(chat_room_request_approve),
        )
        .route(
            "/api/apps/chat-room/requests/:request_id/deny",
            post(chat_room_request_deny),
        )
        .route(
            "/api/apps/chat-room/guests/:session_id/kick",
            post(chat_room_guest_kick),
        )
        .route(
            "/api/apps/chat-room/access-policy",
            post(chat_room_access_policy_update),
        )
        .route(
            "/api/apps/chat-room/members/invite",
            post(chat_room_member_invite),
        )
        .route(
            "/api/apps/chat-room/members/remove",
            post(chat_room_member_remove),
        )
        .route(
            "/api/apps/chat-room/invites/revoke",
            post(chat_room_invite_revoke),
        )
        .route(
            "/api/apps/chat-room/session/start",
            post(chat_room_session_start),
        )
        .route(
            "/api/apps/chat-room/session/leave",
            post(room_service_session_leave),
        )
        .route("/api/apps/chat-room/poll", post(room_service_poll))
        .route(
            "/api/apps/chat-room/objects/send",
            post(room_service_objects_send),
        )
        .route(
            "/api/apps/chat-room/upload/start",
            post(room_service_upload_start),
        )
        .route(
            "/api/apps/chat-room/upload/:upload_id/chunk",
            post(room_service_upload_chunk),
        )
        .route(
            "/api/apps/chat-room/upload/:upload_id/finish",
            post(room_service_upload_finish),
        )
        .route(
            "/api/apps/chat-room/attachments/:attachment_id",
            get(room_service_attachment_get),
        )
        .route(
            "/api/viewers/:viewer/library",
            get(super::viewer_gateway::viewer_library_summary),
        )
        .route(
            "/api/viewers/:viewer/content/:capsule",
            get(super::viewer_gateway::viewer_content),
        )
        .route(
            "/api/viewers/:viewer/storage/:capsule/:scope/:name",
            get(super::viewer_gateway::viewer_storage_get)
                .put(super::viewer_gateway::viewer_storage_put),
        )
        .route(
            "/apps/:app",
            get(super::browser_capsules::serve_browser_app_root),
        )
        .route(
            "/apps/:app/",
            get(super::browser_capsules::serve_browser_app_index),
        )
        .route(
            "/apps/:app/*path",
            get(super::browser_capsules::serve_browser_app_asset),
        )
        .route("/s/:cid", get(redirect_cid_root))
        .route("/s/:cid/", get(serve_cid_root))
        .route("/s/:cid/*path", get(serve_cid_file))
        // IPFS-compatible paths so install.sh can use this gateway like ipfs.io
        .route("/ipfs/:cid", get(serve_ipfs_cid_root))
        .route("/ipfs/:cid/", get(serve_cid_root))
        .route("/ipfs/:cid/*path", get(serve_cid_file))
        .route("/*path", get(serve_public_site_path))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn landing_page() -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>ElastOS Gateway</title>
  <style>
    :root {{
      --bg: #0c1217;
      --panel: #111a21;
      --text: #e9f0f4;
      --muted: #9bb1bf;
      --accent: #29b6f6;
      --ok: #54d66d;
      --border: #203241;
    }}
    body {{
      margin: 0;
      font-family: "Segoe UI", "SF Pro Text", system-ui, sans-serif;
      background: radial-gradient(circle at 20% 0%, #122130 0%, var(--bg) 45%);
      color: var(--text);
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 1.25rem;
    }}
    main {{
      width: min(820px, 100%);
      background: linear-gradient(180deg, #111a21 0%, #0f171f 100%);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 1.25rem;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.35);
    }}
    h1 {{
      margin: 0 0 0.25rem;
      font-size: 1.5rem;
    }}
    .muted {{
      color: var(--muted);
      margin: 0.125rem 0 0.75rem;
    }}
    .version {{
      font-size: 0.875rem;
      color: var(--ok);
      margin-bottom: 1rem;
    }}
    form {{
      display: flex;
      gap: 0.5rem;
      margin: 1rem 0 0.75rem;
      flex-wrap: wrap;
    }}
    input[type="text"] {{
      flex: 1 1 420px;
      min-width: 220px;
      background: #0d141b;
      color: var(--text);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 0.7rem 0.8rem;
      font-size: 0.95rem;
    }}
    button {{
      background: var(--accent);
      color: #05131d;
      border: 0;
      border-radius: 10px;
      padding: 0.7rem 1rem;
      font-weight: 700;
      cursor: pointer;
    }}
    code {{
      background: #0c141b;
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 0.1rem 0.35rem;
    }}
    ul {{
      margin-top: 0.5rem;
      color: var(--muted);
    }}
    a {{
      color: var(--accent);
    }}
  </style>
</head>
<body>
  <main>
    <h1>ElastOS Gateway</h1>
    <p class="muted">Public ElastOS edge for MyWebSite, publisher objects, and read-only CID content.</p>
    <p class="version">Version {}</p>

    <form id="cid-form">
      <input id="cid-input" type="text" placeholder="Paste CID, elastos://CID, or gateway URL" autocomplete="off" />
      <button type="submit">Open</button>
    </form>

    <p class="muted">URL format: <code>/s/&lt;cid&gt;/</code></p>
    <ul>
      <li>Health check: <a href="/healthz">/healthz</a></li>
      <li>Site root: <code>/</code> from <code>localhost://MyWebSite</code> or a bound Edge target.</li>
      <li>Publisher objects: <code>/release-head.json</code>, <code>/release.json</code>, <code>/install.sh</code>, <code>/artifacts/...</code></li>
      <li>Content example: <code>/s/bafy.../</code></li>
    </ul>
  </main>

  <script>
    (function () {{
      function extractCid(input) {{
        var s = (input || "").trim().replace(/\/+$/, "");
        if (!s) return "";
        if (s.startsWith("elastos://")) return s.slice("elastos://".length).split("/")[0];
        var m1 = s.match(/\/ipfs\/([^\/?#]+)/);
        if (m1 && m1[1]) return m1[1];
        var m2 = s.match(/^https?:\/\/([^./]+)\.ipfs\./i);
        if (m2 && m2[1]) return m2[1];
        return s;
      }}

      var form = document.getElementById("cid-form");
      var input = document.getElementById("cid-input");
      if (!form || !input) return;
      form.addEventListener("submit", function (e) {{
        e.preventDefault();
        var cid = extractCid(input.value);
        if (!cid) return;
        window.location.href = "/s/" + encodeURIComponent(cid) + "/";
      }});
    }})();
  </script>
</body>
</html>"#,
        GATEWAY_VERSION
    ))
}

#[derive(Serialize)]
struct HomeSummaryResponse {
    home: HomeRouteInfo,
    app: HomeCapsuleIdentity,
    identity: HomeIdentitySummary,
    appearance: HomeAppearanceSummary,
    runtime: HomeRuntimeSummary,
    site: HomeSiteSummary,
    room: HomeRoomSummary,
    notifications: HomeNotificationsSummary,
    targets: Vec<HomeTargetSummary>,
}

#[derive(Serialize)]
struct HomeRouteInfo {
    route: String,
    attach_kind: String,
}

#[derive(Serialize)]
struct HomeIdentitySummary {
    device_did: Option<String>,
    handle: Option<String>,
}

#[derive(Deserialize)]
struct SystemHandleUpdateRequest {
    handle: String,
}

#[derive(Deserialize)]
struct SystemBackgroundOverlayRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "home_background_overlay_opacity_default")]
    opacity: f64,
}

#[derive(Serialize)]
struct HomeCapsuleIdentity {
    id: String,
    route: String,
}

#[derive(Serialize)]
struct SystemSummaryResponse {
    identity: HomeIdentitySummary,
    home: HomeCapsuleIdentity,
    app: SystemCapsuleIdentity,
    appearance: HomeAppearanceSummary,
    runtime: HomeRuntimeSummary,
    storage: SystemStorageSummary,
    runtime_log: SystemRuntimeLogSummary,
}

#[derive(Serialize)]
struct SystemCapsuleIdentity {
    id: String,
    route: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct HomeAppearanceSummary {
    #[serde(default)]
    background_image_url: Option<String>,
    background_overlay_enabled: bool,
    background_overlay_opacity: f64,
}

#[derive(Serialize)]
struct InboxSummaryResponse {
    app: HomeCapsuleIdentity,
    notifications: HomeNotificationsSummary,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemStorageSummary {
    available: bool,
    #[serde(default)]
    documents_count: usize,
    #[serde(default)]
    drafts_count: usize,
    #[serde(default)]
    published_count: usize,
    #[serde(default)]
    objects_root: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemRuntimeLogSummary {
    available: bool,
    #[serde(default)]
    total_in_memory: Option<usize>,
    #[serde(default)]
    current_epoch: Option<u64>,
    #[serde(default)]
    events: Vec<SystemRuntimeEventSummary>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SystemRuntimeEventSummary {
    kind: String,
    #[serde(default)]
    at: Option<u64>,
    summary: String,
}

const SYSTEM_RUNTIME_ACTIVITY_FETCH_LIMIT: usize = 32;
const SYSTEM_RUNTIME_ACTIVITY_DISPLAY_LIMIT: usize = 4;
const HOME_BACKGROUND_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
const HOME_BACKGROUND_IMAGE_TRANSPORT_MAX_BYTES: usize = 8 * 1024 * 1024;
const HOME_BACKGROUND_OVERLAY_FILE: &str = "background-overlay.json";
const HOME_BACKGROUND_OVERLAY_DEFAULT: bool = false;
const HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT: f64 = 0.55;
const HOME_BACKGROUND_OVERLAY_OPACITY_MAX: f64 = 0.8;
const HOME_BACKGROUND_IMAGE_FILES: &[(&str, &str)] = &[
    ("background-image.png", "image/png"),
    ("background-image.jpg", "image/jpeg"),
    ("background-image.webp", "image/webp"),
    ("background-image.gif", "image/gif"),
];

#[derive(Debug, Clone, Default)]
struct GatewayRuntimeLaunchOutcome {
    status: String,
    capsule_id: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRuntimeLaunchResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAuditLogResponse {
    events: Vec<elastos_runtime::primitives::audit::AuditEvent>,
    total_in_memory: usize,
    current_epoch: u64,
}

#[derive(Default, Serialize)]
struct HomeRuntimeSummary {
    running: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    running_capsules: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Serialize)]
struct HomeSiteSummary {
    staged: bool,
    root_uri: String,
    path: String,
    #[serde(default)]
    active_release: Option<String>,
    #[serde(default)]
    active_channel: Option<String>,
    #[serde(default)]
    active_bundle_cid: Option<String>,
    release_count: usize,
}

impl Default for HomeSiteSummary {
    fn default() -> Self {
        Self {
            staged: false,
            root_uri: MY_WEBSITE_URI.to_string(),
            path: String::new(),
            active_release: None,
            active_channel: None,
            active_bundle_cid: None,
            release_count: 0,
        }
    }
}

#[derive(Default, Serialize)]
struct HomePendingRequestSummary {
    request_id: String,
    display_name: String,
    device_label: String,
    requested_at: u64,
}

#[derive(Default, Serialize)]
struct HomeActiveSessionSummary {
    display_name: String,
    device_label: String,
    approved_at: u64,
    last_seen_at: u64,
}

#[derive(Serialize)]
struct HomeRoomSummary {
    room_slug: String,
    title: String,
    member_count: usize,
    active_member_count: usize,
    pending_count: usize,
    active_session_count: usize,
    #[serde(default)]
    latest_request_name: Option<String>,
    #[serde(default)]
    latest_request_device: Option<String>,
    #[serde(default)]
    local_runtime_did: Option<String>,
    #[serde(default)]
    local_runtime_role: Option<String>,
    #[serde(default)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    ephemeral_hosted_guest_url: Option<String>,
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    pending_requests: Vec<HomePendingRequestSummary>,
    #[serde(default)]
    active_sessions: Vec<HomeActiveSessionSummary>,
}

impl Default for HomeRoomSummary {
    fn default() -> Self {
        Self {
            room_slug: crate::room_service::room_slug().to_string(),
            title: String::new(),
            member_count: 0,
            active_member_count: 0,
            pending_count: 0,
            active_session_count: 0,
            latest_request_name: None,
            latest_request_device: None,
            local_runtime_did: None,
            local_runtime_role: None,
            canonical_hosted_guest_url: None,
            ephemeral_hosted_guest_url: None,
            browser_access_allowed: true,
            browser_access_block_reason: None,
            pending_requests: Vec::new(),
            active_sessions: Vec::new(),
        }
    }
}

#[derive(Default, Serialize)]
struct HomeNotificationsSummary {
    unread_count: usize,
    attention_count: usize,
    #[serde(default)]
    entries: Vec<HomeNotificationEntrySummary>,
}

#[derive(Default, Serialize)]
struct HomeNotificationEntrySummary {
    id: String,
    source_app: String,
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    action_ref: Option<HomeNotificationActionSummary>,
    severity: String,
    read: bool,
    created_at: u64,
}

#[derive(Default, Serialize)]
struct HomeNotificationActionSummary {
    app: String,
    action_id: String,
}

#[derive(Default)]
struct HomeState {
    site: HomeSiteSummary,
    room: HomeRoomSummary,
    notifications: HomeNotificationsSummary,
}

#[derive(Clone, Serialize)]
struct HomeTargetSummary {
    target: String,
    title: String,
    description: String,
    route: String,
    attach_kind: String,
    role: CapsuleRole,
    target_kind: HomeTargetKind,
}

#[derive(Deserialize)]
struct HomeLaunchRequest {
    target: String,
    #[serde(default)]
    query: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct InboxActionRequest {
    action_id: String,
}

#[derive(Serialize)]
struct InboxActionResponse {
    message: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum HomeTargetKind {
    App,
    Object,
}

#[derive(Serialize)]
struct HomeLaunchResponse {
    target: String,
    title: String,
    route: String,
    attach_kind: String,
    role: CapsuleRole,
    target_kind: HomeTargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capsule_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomeLaunchTokenPayload {
    schema: String,
    app: String,
    iat: u64,
    exp: u64,
    principal_id: String,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_binding_id: Option<String>,
    grant_id: String,
    non_delegatable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomeLaunchTokenEnvelope {
    payload: HomeLaunchTokenPayload,
    signature: String,
    signer_did: String,
}

#[derive(Clone)]
pub(super) struct HomeLaunchTokenContext {
    pub principal_id: String,
    pub session_id: String,
    pub proof_binding_id: Option<String>,
    pub grant_id: String,
}

struct RequiredHomeLaunchToken {
    app: String,
    context: HomeLaunchTokenContext,
}

#[derive(Debug, Serialize)]
struct ChatRoomSessionStartResponse {
    status: String,
    display_name: String,
    expires_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomSummary {
    room_slug: String,
    pending_count: usize,
    active_session_count: usize,
    #[serde(default)]
    latest_request_name: Option<String>,
    #[serde(default)]
    latest_request_device: Option<String>,
    #[serde(default)]
    active_participants: Vec<crate::room_service::ParticipantView>,
    #[serde(default)]
    pending_requests: Vec<crate::room_service::PendingRequestView>,
    #[serde(default)]
    active_sessions: Vec<GatewayActiveSessionSummary>,
    #[serde(default)]
    room_control: crate::room_service::RoomControlSummary,
    #[serde(default)]
    local_runtime_did: Option<String>,
    #[serde(default)]
    local_runtime_role: Option<crate::room_service::RoomRole>,
    #[serde(default)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    ephemeral_hosted_guest_url: Option<String>,
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    transport: crate::room_service::RoomTransportView,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayActiveSessionSummary {
    session_id: String,
    display_name: String,
    device_label: String,
    approved_at: u64,
    expires_at: u64,
    last_seen_at: u64,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    member_did: Option<String>,
}

impl From<crate::room_service::RoomSummary> for GatewayRoomSummary {
    fn from(summary: crate::room_service::RoomSummary) -> Self {
        Self {
            room_slug: summary.room_slug,
            pending_count: summary.pending_count,
            active_session_count: summary.active_session_count,
            latest_request_name: summary.latest_request_name,
            latest_request_device: summary.latest_request_device,
            active_participants: summary.active_participants,
            pending_requests: summary.pending_requests,
            active_sessions: summary
                .active_sessions
                .into_iter()
                .map(GatewayActiveSessionSummary::from)
                .collect(),
            room_control: summary.room_control,
            local_runtime_did: summary.local_runtime_did,
            local_runtime_role: summary.local_runtime_role,
            canonical_hosted_guest_url: summary.canonical_hosted_guest_url,
            ephemeral_hosted_guest_url: summary.ephemeral_hosted_guest_url,
            browser_access_allowed: summary.browser_access_allowed,
            browser_access_block_reason: summary.browser_access_block_reason,
            transport: summary.transport,
        }
    }
}

impl From<crate::room_service::ActiveSessionView> for GatewayActiveSessionSummary {
    fn from(session: crate::room_service::ActiveSessionView) -> Self {
        Self {
            session_id: session.session_id,
            display_name: session.display_name,
            device_label: session.device_label,
            approved_at: session.approved_at,
            expires_at: session.expires_at,
            last_seen_at: session.last_seen_at,
            capabilities: session.capabilities,
            member_did: session.member_did,
        }
    }
}

async fn home_summary(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return home_error_response(err);
    }

    let identity = load_gateway_identity_summary(&state.data_dir);
    let data_dir = state.data_dir.clone();
    let (runtime, home_state) = tokio::join!(home_runtime_summary(&state.data_dir), async move {
        tokio::task::spawn_blocking(move || home_state(&data_dir))
            .await
            .unwrap_or_default()
    });

    Json(HomeSummaryResponse {
        home: HomeRouteInfo {
            route: HOME_ROUTE.to_string(),
            attach_kind: "iframe".to_string(),
        },
        app: HomeCapsuleIdentity {
            id: HOME_CAPSULE_ID.to_string(),
            route: HOME_ROUTE.to_string(),
        },
        identity,
        appearance: home_appearance_summary(&state.data_dir),
        runtime,
        site: home_state.site,
        room: home_state.room,
        notifications: home_state.notifications,
        targets: home_targets(&state.data_dir),
    })
    .into_response()
}

async fn home_runtime_ensure(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return home_error_response(err);
    }

    Json(ensure_home_runtime(&state.data_dir).await).into_response()
}

async fn system_summary(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    let (runtime, storage, runtime_log) = tokio::join!(
        home_runtime_summary(&state.data_dir),
        system_storage_summary(
            state.provider_registry.as_ref().cloned(),
            &context.principal_id
        ),
        system_runtime_log(&state.data_dir)
    );
    Json(SystemSummaryResponse {
        identity: load_gateway_identity_summary(&state.data_dir),
        home: HomeCapsuleIdentity {
            id: HOME_CAPSULE_ID.to_string(),
            route: HOME_ROUTE.to_string(),
        },
        app: SystemCapsuleIdentity {
            id: SYSTEM_CAPSULE_ID.to_string(),
            route: SYSTEM_ROUTE.to_string(),
        },
        appearance: home_appearance_summary(&state.data_dir),
        runtime,
        storage,
        runtime_log,
    })
    .into_response()
}

async fn system_storage_summary(
    provider_registry: Option<Arc<ProviderRegistry>>,
    principal_id: &str,
) -> SystemStorageSummary {
    let Some(registry) = provider_registry else {
        return SystemStorageSummary {
            available: false,
            note: Some("Document provider unavailable.".to_string()),
            ..SystemStorageSummary::default()
        };
    };
    match DocumentsClient::for_principal(registry, principal_id)
        .summary()
        .await
    {
        Ok(documents) => {
            let published_count = documents
                .iter()
                .filter(|item| {
                    !item
                        .latest_published_cid
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                })
                .count();
            let documents_count = documents.len();
            SystemStorageSummary {
                available: true,
                documents_count,
                drafts_count: documents_count.saturating_sub(published_count),
                published_count,
                objects_root: Some("localhost://ElastOS/Documents/".to_string()),
                note: Some("Documents stay local until published.".to_string()),
            }
        }
        Err(err) => SystemStorageSummary {
            available: false,
            note: Some(err.to_string()),
            ..SystemStorageSummary::default()
        },
    }
}

fn home_appearance_summary(data_dir: &std::path::Path) -> HomeAppearanceSummary {
    let (overlay_enabled, overlay_opacity) = home_background_overlay_settings(data_dir);
    HomeAppearanceSummary {
        background_image_url: home_background_image_entry(data_dir).map(
            |(_path, _content_type, modified)| {
                format!("/api/apps/home/appearance/background-image?v={modified}")
            },
        ),
        background_overlay_enabled: overlay_enabled,
        background_overlay_opacity: overlay_opacity,
    }
}

fn home_appearance_root(data_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, "ElastOS/System/Appearance")
        .ok_or_else(|| anyhow::anyhow!("invalid appearance root"))
}

fn home_background_image_entry(data_dir: &std::path::Path) -> Option<(PathBuf, &'static str, u64)> {
    let root = home_appearance_root(data_dir).ok()?;
    for (file_name, content_type) in HOME_BACKGROUND_IMAGE_FILES {
        let path = root.join(file_name);
        if !path.is_file() {
            continue;
        }
        let modified = path
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_else(now_ts);
        return Some((path, *content_type, modified));
    }
    None
}

fn home_background_overlay_opacity_default() -> f64 {
    HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT
}

fn home_clamp_background_overlay_opacity(opacity: f64) -> f64 {
    if !opacity.is_finite() {
        return HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT;
    }
    opacity.clamp(0.0, HOME_BACKGROUND_OVERLAY_OPACITY_MAX)
}

fn home_background_overlay_settings(data_dir: &std::path::Path) -> (bool, f64) {
    let Ok(root) = home_appearance_root(data_dir) else {
        return (
            HOME_BACKGROUND_OVERLAY_DEFAULT,
            HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
        );
    };
    let path = root.join(HOME_BACKGROUND_OVERLAY_FILE);
    if !path.is_file() {
        return (
            HOME_BACKGROUND_OVERLAY_DEFAULT,
            HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
        );
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (
            HOME_BACKGROUND_OVERLAY_DEFAULT,
            HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
        );
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (
            HOME_BACKGROUND_OVERLAY_DEFAULT,
            HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
        );
    };
    let enabled = payload
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(HOME_BACKGROUND_OVERLAY_DEFAULT);
    let opacity = payload
        .get("opacity")
        .and_then(|value| value.as_f64())
        .map(home_clamp_background_overlay_opacity)
        .unwrap_or(HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT);
    (enabled, opacity)
}

fn home_save_background_overlay(
    data_dir: &std::path::Path,
    enabled: bool,
    opacity: f64,
) -> anyhow::Result<HomeAppearanceSummary> {
    let root = home_appearance_root(data_dir)?;
    std::fs::create_dir_all(&root)?;
    let payload = serde_json::json!({
        "enabled": enabled,
        "opacity": home_clamp_background_overlay_opacity(opacity),
    });
    std::fs::write(
        root.join(HOME_BACKGROUND_OVERLAY_FILE),
        serde_json::to_vec_pretty(&payload)?,
    )?;
    Ok(home_appearance_summary(data_dir))
}

fn home_save_background_image(
    data_dir: &std::path::Path,
    file_name: &'static str,
    bytes: Vec<u8>,
) -> anyhow::Result<HomeAppearanceSummary> {
    let root = home_appearance_root(data_dir)?;
    std::fs::create_dir_all(&root)?;
    remove_home_background_images(&root)?;
    std::fs::write(root.join(file_name), bytes)?;
    Ok(home_appearance_summary(data_dir))
}

fn home_reset_background_image(
    data_dir: &std::path::Path,
) -> anyhow::Result<HomeAppearanceSummary> {
    let root = home_appearance_root(data_dir)?;
    remove_home_background_images(&root)?;
    Ok(home_appearance_summary(data_dir))
}

fn remove_home_background_images(root: &std::path::Path) -> anyhow::Result<()> {
    for (file_name, _content_type) in HOME_BACKGROUND_IMAGE_FILES {
        let path = root.join(file_name);
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn parse_background_image_upload(
    headers: &HeaderMap,
    body: &Bytes,
) -> anyhow::Result<(&'static str, Vec<u8>)> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    let file_name = match content_type {
        "image/png" => "background-image.png",
        "image/jpeg" => "background-image.jpg",
        "image/webp" => "background-image.webp",
        "image/gif" => "background-image.gif",
        _ => anyhow::bail!("background image must be PNG, JPEG, WebP, or GIF"),
    };
    if body.is_empty() {
        anyhow::bail!("background image is empty");
    }
    if body.len() > HOME_BACKGROUND_IMAGE_MAX_BYTES {
        anyhow::bail!("background image is larger than 5 MB");
    }
    Ok((file_name, body.to_vec()))
}

async fn system_handle_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemHandleUpdateRequest>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        return system_error_response(err);
    }

    match elastos_identity::save_nickname(&state.data_dir, &req.handle) {
        Ok(()) => Json(load_gateway_identity_summary(&state.data_dir)).into_response(),
        Err(err) => system_error_response(err),
    }
}

async fn system_background_image_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        return system_error_response(err);
    }

    let upload = match parse_background_image_upload(&headers, &body) {
        Ok(upload) => upload,
        Err(err) => return system_error_response(err),
    };

    match home_save_background_image(&state.data_dir, upload.0, upload.1) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

async fn system_background_image_reset(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        return system_error_response(err);
    }

    match home_reset_background_image(&state.data_dir) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

async fn system_background_overlay_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemBackgroundOverlayRequest>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        return system_error_response(err);
    }

    match home_save_background_overlay(&state.data_dir, req.enabled, req.opacity) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

async fn home_background_image(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return home_error_response(err);
    }

    let Some((path, content_type, _modified)) = home_background_image_entry(&state.data_dir) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut response = bytes.into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(content_type),
            );
            response
        }
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

async fn gateway_provider_proxy(
    State(state): State<GatewayState>,
    Path((scheme, op)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if scheme != "documents" {
        return (StatusCode::NOT_FOUND, "Gateway provider not found").into_response();
    }
    let allowed_apps: &[&str] = match op.as_str() {
        "summary" | "get" => &[DOCUMENTS_CAPSULE_ID, LIBRARY_CAPSULE_ID],
        _ => &[DOCUMENTS_CAPSULE_ID],
    };
    let context =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, allowed_apps) {
            Ok(context) => context,
            Err(err) => return documents_error_response(err),
        };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => return documents_error_response(anyhow::anyhow!("documents provider unavailable")),
    };
    let mut request = if body.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "provider request body must be a JSON object",
                )
                    .into_response();
            }
            Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
        }
    };
    request["op"] = serde_json::Value::String(op.clone());
    request["principal_id"] = serde_json::Value::String(context.principal_id);

    let response = match registry.send_raw("documents", &request).await {
        Ok(value) => value,
        Err(err) => serde_json::json!({
            "status": "error",
            "code": "provider_error",
            "message": err.to_string(),
        }),
    };

    Json(response).into_response()
}

async fn inbox_summary(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, INBOX_CAPSULE_ID) {
        return inbox_error_response(err);
    }

    let home_state = home_state(&state.data_dir);
    Json(InboxSummaryResponse {
        app: HomeCapsuleIdentity {
            id: INBOX_CAPSULE_ID.to_string(),
            route: "/apps/inbox/".to_string(),
        },
        notifications: home_state.notifications,
    })
    .into_response()
}

async fn inbox_action(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, INBOX_CAPSULE_ID) {
        return inbox_error_response(err);
    }

    match parse_inbox_action_request(&headers, &body)
        .map_err(anyhow::Error::msg)
        .and_then(|req| dispatch_inbox_action(&state.data_dir, &req.action_id))
    {
        Ok(message) => Json(InboxActionResponse { message }).into_response(),
        Err(err) => inbox_error_response(err),
    }
}

fn parse_inbox_action_request(
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<InboxActionRequest, String> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") {
        serde_json::from_slice(body).map_err(|err| format!("invalid inbox action body: {err}"))
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        let action_id = form_urlencoded::parse(body.as_ref())
            .find_map(|(key, value)| (key == "action_id").then(|| value.into_owned()))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "missing action_id".to_string())?;
        Ok(InboxActionRequest { action_id })
    } else {
        Err("unsupported inbox action content type".to_string())
    }
}

fn dispatch_inbox_action(data_dir: &std::path::Path, action_id: &str) -> anyhow::Result<String> {
    if let Some(notification_id) = action_id.strip_prefix("notification-read:") {
        return Ok(
            match crate::notifications::mark_read(data_dir, notification_id)? {
                true => "Marked inbox entry read.".to_string(),
                false => "That inbox entry was already read or is no longer present.".to_string(),
            },
        );
    }
    if let Some(notification_id) = action_id.strip_prefix("notification-dismiss:") {
        return Ok(
            match crate::notifications::dismiss(data_dir, notification_id)? {
                true => "Dismissed inbox entry.".to_string(),
                false => "That inbox entry is already gone.".to_string(),
            },
        );
    }
    if let Some(request_id) = action_id.strip_prefix("room-approve-request:") {
        let message = match crate::room_service::approve_request(data_dir, request_id)? {
            Some(outcome) => format!(
                "Approved Chat Room browser access for {} on {}.",
                outcome.display_name, outcome.device_label
            ),
            None => "That browser access request is no longer pending.".to_string(),
        };
        let summary = crate::room_service::load_summary(data_dir)?;
        let _ = crate::notifications::sync_room_notifications(data_dir, &summary);
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        return Ok(message);
    }
    if let Some(request_id) = action_id.strip_prefix("room-deny-request:") {
        let message =
            match crate::room_service::deny_request(data_dir, request_id, "Denied from Inbox.")? {
                Some(outcome) => format!(
                    "Denied Chat Room browser access for {} on {}.",
                    outcome.display_name, outcome.device_label
                ),
                None => "That browser access request is no longer pending.".to_string(),
            };
        let summary = crate::room_service::load_summary(data_dir)?;
        let _ = crate::notifications::sync_room_notifications(data_dir, &summary);
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        return Ok(message);
    }
    anyhow::bail!("unknown inbox action");
}

async fn home_launch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<HomeLaunchRequest>,
) -> Result<Json<HomeLaunchResponse>, (StatusCode, Json<serde_json::Value>)> {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        ));
    }

    let target = req.target.trim();
    if target.is_empty() || target == HOME_CAPSULE_ID {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid Home target" })),
        ));
    }

    let Some(target_summary) = home_targets(&state.data_dir)
        .into_iter()
        .find(|candidate| candidate.target == target)
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Home target not found" })),
        ));
    };

    let launch =
        launch_runtime_backed_home_target(&state.data_dir, target_summary.target.as_str()).await;
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
        &req.query,
    )
    .map_err(gateway_internal_error)?;

    Ok(Json(HomeLaunchResponse {
        target: target_summary.target,
        title: target_summary.title,
        route,
        attach_kind: target_summary.attach_kind,
        role: target_summary.role,
        target_kind: target_summary.target_kind,
        launch_status: launch.as_ref().map(|summary| summary.status.clone()),
        launch_detail: launch.as_ref().and_then(|summary| summary.detail.clone()),
        capsule_id: launch.and_then(|summary| summary.capsule_id),
    }))
}

fn append_home_launch_token(
    data_dir: &std::path::Path,
    route: &str,
    target: &str,
    query: &BTreeMap<String, String>,
) -> anyhow::Result<String> {
    let token = issue_home_launch_token(data_dir, target)?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("home_token", &token);
    for (key, value) in query {
        if key.trim().is_empty() {
            continue;
        }
        serializer.append_pair(key, value);
    }
    let encoded = serializer.finish();
    let separator = if route.contains('?') { '&' } else { '?' };
    Ok(format!("{route}{separator}{encoded}"))
}

fn home_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let mut targets: Vec<_> = super::browser_capsules::list_launchable_browser_capsules(data_dir)
        .into_iter()
        .filter(|app| app.name != HOME_CAPSULE_ID)
        .map(|app| {
            let target_kind = home_target_kind(&app.name);
            HomeTargetSummary {
                route: format!("/apps/{}/", app.name),
                title: app_shell_title(&app.name),
                description: app_shell_description(&app.name, app.description),
                target: app.name,
                attach_kind: "iframe".to_string(),
                role: app.role,
                target_kind,
            }
        })
        .collect();
    targets.extend(
        super::browser_capsules::list_all_viewer_bound_capsules(data_dir)
            .into_iter()
            .map(|capsule| HomeTargetSummary {
                route: format!("/apps/{}/?capsule={}", capsule.viewer, capsule.name),
                title: viewer_object_shell_title(&capsule.name, capsule.description.as_deref()),
                description: viewer_object_shell_description(
                    &capsule.viewer,
                    capsule.description.as_deref(),
                ),
                target: capsule.name,
                attach_kind: "iframe".to_string(),
                role: CapsuleRole::Content,
                target_kind: HomeTargetKind::Object,
            }),
    );
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

fn home_target_kind(name: &str) -> HomeTargetKind {
    match name {
        LIBRARY_CAPSULE_ID => HomeTargetKind::Object,
        _ => HomeTargetKind::App,
    }
}

fn load_gateway_identity_summary(data_dir: &std::path::Path) -> HomeIdentitySummary {
    let device_did = elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_, did)| did)
        .filter(|did| !did.trim().is_empty());
    let handle = elastos_identity::load_nickname(data_dir)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty());

    HomeIdentitySummary { device_did, handle }
}

fn apply_room_access(
    summary: &mut crate::room_service::RoomSummary,
    access: crate::room_service::LocalRuntimeAccess,
) {
    summary.local_runtime_did = access.runtime_did;
    summary.local_runtime_role = access.member_role;
    summary.browser_access_allowed = access.browser_access_allowed;
    summary.browser_access_block_reason = access.block_reason;
}

async fn launch_runtime_backed_home_target(
    data_dir: &FsPath,
    target: &str,
) -> Option<GatewayRuntimeLaunchOutcome> {
    let capsule_dir = resolve_capsule_dir(data_dir, target)?;
    let manifest = super::capsule_inventory::load_capsule_manifest(&capsule_dir, target)?;
    if !manifest.role.is_shell_launchable() || manifest.capsule_type == CapsuleType::Data {
        return None;
    }

    Some(match launch_runtime_capsule(data_dir, &capsule_dir).await {
        Ok(outcome) => outcome,
        Err(err) => GatewayRuntimeLaunchOutcome {
            status: "failed".to_string(),
            capsule_id: None,
            detail: Some(err.to_string()),
        },
    })
}

async fn launch_runtime_capsule(
    data_dir: &FsPath,
    capsule_dir: &FsPath,
) -> anyhow::Result<GatewayRuntimeLaunchOutcome> {
    let coords = load_live_runtime_coords(data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let home_token =
        gateway_attach_runtime_token(&client, &coords.api_url, &coords.attach_secret, "shell")
            .await?;
    let response = client
        .post(format!("{}/api/capsules", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .json(&serde_json::json!({
            "path": capsule_dir.display().to_string(),
        }))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("runtime launch failed ({}): {}", status, text.trim());
    }
    let payload = response.json::<GatewayRuntimeLaunchResponse>().await?;
    Ok(GatewayRuntimeLaunchOutcome {
        status: "launched".to_string(),
        capsule_id: Some(payload.id),
        detail: None,
    })
}

async fn system_runtime_log(data_dir: &FsPath) -> SystemRuntimeLogSummary {
    let Some(coords) = load_live_runtime_coords(data_dir).await else {
        return SystemRuntimeLogSummary {
            available: false,
            total_in_memory: None,
            current_epoch: None,
            events: Vec::new(),
            note: Some("Local runtime is not running.".to_string()),
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime client unavailable: {err}")),
            }
        }
    };

    let home_token = match gateway_attach_runtime_token(
        &client,
        &coords.api_url,
        &coords.attach_secret,
        "shell",
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime attach failed: {err}")),
            }
        }
    };

    let response = match client
        .get(format!(
            "{}/api/audit?limit={}",
            coords.api_url, SYSTEM_RUNTIME_ACTIVITY_FETCH_LIMIT
        ))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime log unavailable: {err}")),
            }
        }
    };

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return SystemRuntimeLogSummary {
            available: false,
            total_in_memory: None,
            current_epoch: None,
            events: Vec::new(),
            note: Some(format!(
                "Runtime log unavailable ({}): {}",
                status,
                text.trim()
            )),
        };
    }

    let payload = match response.json::<GatewayAuditLogResponse>().await {
        Ok(payload) => payload,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime log could not be decoded: {err}")),
            }
        }
    };

    SystemRuntimeLogSummary {
        available: true,
        total_in_memory: Some(payload.total_in_memory),
        current_epoch: Some(payload.current_epoch),
        events: system_runtime_activity_summaries(payload.events),
        note: None,
    }
}

fn system_runtime_activity_summaries(
    events: Vec<elastos_runtime::primitives::audit::AuditEvent>,
) -> Vec<SystemRuntimeEventSummary> {
    let mut summaries = events
        .into_iter()
        .filter_map(system_runtime_event_summary)
        .collect::<Vec<_>>();
    summaries.sort_by_key(|event| std::cmp::Reverse(event.at.unwrap_or_default()));
    summaries.truncate(SYSTEM_RUNTIME_ACTIVITY_DISPLAY_LIMIT);
    summaries
}

fn system_runtime_event_summary(
    event: elastos_runtime::primitives::audit::AuditEvent,
) -> Option<SystemRuntimeEventSummary> {
    use elastos_runtime::primitives::audit::{AuditEvent, StopReason};

    let kind = event.event_type_name().to_string();
    let at = match &event {
        AuditEvent::RuntimeStart { timestamp, .. }
        | AuditEvent::RuntimeStop { timestamp }
        | AuditEvent::CapsuleLaunch { timestamp, .. }
        | AuditEvent::CapsuleStop { timestamp, .. }
        | AuditEvent::CapabilityGrant { timestamp, .. }
        | AuditEvent::CapabilityRevoke { timestamp, .. }
        | AuditEvent::CapabilityUse { timestamp, .. }
        | AuditEvent::ContentFetch { timestamp, .. }
        | AuditEvent::AuthAttempt { timestamp, .. }
        | AuditEvent::EpochAdvance { timestamp, .. }
        | AuditEvent::ConfigChange { timestamp, .. }
        | AuditEvent::SecurityWarning { timestamp, .. }
        | AuditEvent::SessionCreated { timestamp, .. }
        | AuditEvent::SessionDestroyed { timestamp, .. }
        | AuditEvent::CapabilityRequested { timestamp, .. }
        | AuditEvent::CapabilityDenied { timestamp, .. }
        | AuditEvent::IdentityRegistered { timestamp, .. }
        | AuditEvent::StorageAccess { timestamp, .. }
        | AuditEvent::MessageSent { timestamp, .. }
        | AuditEvent::PolicyProposal { timestamp, .. }
        | AuditEvent::PolicyDecisionMade { timestamp, .. }
        | AuditEvent::PolicyDivergence { timestamp, .. } => Some(timestamp.unix_secs),
        AuditEvent::Custom { .. } => None,
    };

    let summary = match event {
        AuditEvent::RuntimeStart { version, .. } => format!("Runtime started ({version})"),
        AuditEvent::RuntimeStop { .. } => "Runtime stopped".to_string(),
        AuditEvent::CapsuleLaunch { capsule_name, .. } => {
            format!("Opened {capsule_name}")
        }
        AuditEvent::CapsuleStop {
            capsule_id, reason, ..
        } => match reason {
            StopReason::Requested | StopReason::Completed => {
                format!("Stopped {capsule_id}")
            }
            StopReason::Error(detail) => format!("Stopped {capsule_id} — error: {detail}"),
            StopReason::ResourceLimit(detail) => {
                format!("Stopped {capsule_id} — resource limit: {detail}")
            }
            StopReason::SecurityViolation(detail) => {
                format!("Stopped {capsule_id} — security violation: {detail}")
            }
        },
        AuditEvent::CapabilityGrant { .. } => return None,
        AuditEvent::CapabilityRevoke { reason, .. } => format!("Capability revoked — {reason}"),
        AuditEvent::CapabilityUse { .. } => return None,
        AuditEvent::ContentFetch { cid, success, .. } => {
            if success {
                return None;
            }
            format!("Content fetch failed — {cid}")
        }
        AuditEvent::AuthAttempt {
            identity,
            success,
            method,
            ..
        } => {
            if success {
                return None;
            }
            format!("Authentication failed for {identity} via {method}")
        }
        AuditEvent::EpochAdvance {
            new_epoch, reason, ..
        } => format!("Capability epoch advanced to {new_epoch} — {reason}"),
        AuditEvent::ConfigChange { setting, .. } => format!("Changed {setting}"),
        AuditEvent::SecurityWarning {
            warning_type,
            details,
            ..
        } => format!("Security warning — {warning_type}: {details}"),
        AuditEvent::SessionCreated { .. } => return None,
        AuditEvent::SessionDestroyed { .. } => return None,
        AuditEvent::CapabilityRequested { .. } => return None,
        AuditEvent::CapabilityDenied { reason, .. } => format!("Capability denied — {reason}"),
        AuditEvent::IdentityRegistered {
            user_id, method, ..
        } => format!("Registered identity {user_id} via {method}"),
        AuditEvent::StorageAccess {
            uri,
            action,
            success,
            ..
        } => {
            if success {
                return None;
            }
            format!("Storage access failed — {action} {uri}")
        }
        AuditEvent::MessageSent { .. } => return None,
        AuditEvent::PolicyProposal { .. } => return None,
        AuditEvent::PolicyDecisionMade { .. } => return None,
        AuditEvent::PolicyDivergence {
            real_outcome,
            shadow_outcome,
            ..
        } => format!("Policy divergence — real {real_outcome}, shadow {shadow_outcome}"),
        AuditEvent::Custom { event_type, .. } => format!("Custom event — {event_type}"),
    };

    Some(SystemRuntimeEventSummary { kind, at, summary })
}

fn resolve_capsule_dir(data_dir: &FsPath, app: &str) -> Option<PathBuf> {
    for candidate in super::capsule_inventory::capsule_dir_candidates(data_dir, app) {
        if let Some(manifest) = super::capsule_inventory::load_capsule_manifest(&candidate, app) {
            if manifest.name == app {
                return Some(candidate);
            }
        }
    }
    None
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn title_case_capsule_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn app_shell_title(name: &str) -> String {
    match name {
        DOCUMENTS_CAPSULE_ID => "Documents".to_string(),
        CHAT_ROOM_CAPSULE_ID => "Chat Room".to_string(),
        LIBRARY_CAPSULE_ID => "Library".to_string(),
        INBOX_CAPSULE_ID => "Inbox".to_string(),
        SYSTEM_CAPSULE_ID => "System".to_string(),
        "gba-emulator" => "GBA Emulator".to_string(),
        _ => title_case_capsule_name(name),
    }
}

fn app_shell_description(name: &str, manifest_description: Option<String>) -> String {
    match name {
        DOCUMENTS_CAPSULE_ID => {
            "Create, edit, and publish markdown documents from this device.".to_string()
        }
        CHAT_ROOM_CAPSULE_ID => {
            "Open the local sovereign room from this runtime inside ElastOS.".to_string()
        }
        LIBRARY_CAPSULE_ID => "Browse documents and open them in Documents.".to_string(),
        INBOX_CAPSULE_ID => "Review requests and approvals for this Home.".to_string(),
        SYSTEM_CAPSULE_ID => {
            "Open System to view this device identity and runtime state.".to_string()
        }
        "gba-emulator" => "Launch the browser-based mGBA frontend.".to_string(),
        _ => manifest_description
            .unwrap_or_else(|| format!("Open {} from Home.", app_shell_title(name))),
    }
}

pub(super) fn viewer_object_shell_title(name: &str, description: Option<&str>) -> String {
    let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) else {
        return title_case_capsule_name(name);
    };
    for separator in [" - ", " — ", ": "] {
        if let Some((title, _)) = description.split_once(separator) {
            let title = title.trim();
            if !title.is_empty() && title.len() <= 48 {
                return title.to_string();
            }
        }
    }
    title_case_capsule_name(name)
}

pub(super) fn viewer_object_shell_description(viewer: &str, description: Option<&str>) -> String {
    description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Open this object in {}.", app_shell_title(viewer)))
}

fn home_state(data_dir: &std::path::Path) -> HomeState {
    let site = home_site_summary(data_dir);

    let identity = room_service_runtime_identity_profile(data_dir);
    let mut room_summary = crate::room_service::load_summary(data_dir).unwrap_or_default();
    if let Ok(hosted) = crate::browser_app_hosts::load_browser_app_hosted_endpoint(
        data_dir,
        crate::room_service::room_slug(),
    ) {
        room_summary.canonical_hosted_guest_url = hosted.canonical_url;
        room_summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    }
    if let Ok(access) = crate::room_service::local_runtime_access(data_dir, identity.did.as_deref())
    {
        apply_room_access(&mut room_summary, access);
    }
    let _ = crate::notifications::sync_room_notifications(data_dir, &room_summary);
    let notifications = crate::notifications::load_summary(data_dir).unwrap_or_default();

    HomeState {
        site,
        room: home_room_summary(room_summary),
        notifications: home_notifications_summary(notifications),
    }
}

fn home_site_summary(data_dir: &std::path::Path) -> HomeSiteSummary {
    let site_root = my_website_root_path(data_dir);
    let active_head = std::fs::read(edge_site_head_path(data_dir, MY_WEBSITE_URI))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SiteHeadEnvelope>(&bytes).ok());
    let release_count = std::fs::read_dir(publisher_site_releases_dir(data_dir, MY_WEBSITE_URI))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count();

    HomeSiteSummary {
        staged: site_root.join("index.html").exists(),
        root_uri: MY_WEBSITE_URI.to_string(),
        path: site_root.display().to_string(),
        active_release: active_head
            .as_ref()
            .and_then(|head| head.payload.release_name.clone()),
        active_channel: active_head
            .as_ref()
            .and_then(|head| head.payload.channel_name.clone()),
        active_bundle_cid: active_head
            .as_ref()
            .and_then(|head| head.payload.bundle_cid.clone()),
        release_count,
    }
}

fn home_room_summary(summary: crate::room_service::RoomSummary) -> HomeRoomSummary {
    HomeRoomSummary {
        room_slug: summary.room_slug,
        title: summary.room_control.title,
        member_count: summary.room_control.member_count,
        active_member_count: summary.room_control.active_member_count,
        pending_count: summary.pending_count,
        active_session_count: summary.active_session_count,
        latest_request_name: summary.latest_request_name,
        latest_request_device: summary.latest_request_device,
        local_runtime_did: summary.local_runtime_did,
        local_runtime_role: summary.local_runtime_role.map(home_room_role_label),
        canonical_hosted_guest_url: summary.canonical_hosted_guest_url,
        ephemeral_hosted_guest_url: summary.ephemeral_hosted_guest_url,
        browser_access_allowed: summary.browser_access_allowed,
        browser_access_block_reason: summary.browser_access_block_reason,
        pending_requests: summary
            .pending_requests
            .into_iter()
            .map(|request| HomePendingRequestSummary {
                request_id: request.request_id,
                display_name: request.display_name,
                device_label: request.device_label,
                requested_at: request.requested_at,
            })
            .collect(),
        active_sessions: summary
            .active_sessions
            .into_iter()
            .map(|session| HomeActiveSessionSummary {
                display_name: session.display_name,
                device_label: session.device_label,
                approved_at: session.approved_at,
                last_seen_at: session.last_seen_at,
            })
            .collect(),
    }
}

fn home_room_role_label(role: crate::room_service::RoomRole) -> String {
    match role {
        crate::room_service::RoomRole::Owner => "owner",
        crate::room_service::RoomRole::Admin => "admin",
        crate::room_service::RoomRole::Member => "member",
    }
    .to_string()
}

fn home_notifications_summary(
    summary: crate::notifications::NotificationSummary,
) -> HomeNotificationsSummary {
    HomeNotificationsSummary {
        unread_count: summary.unread_count,
        attention_count: summary.attention_count,
        entries: summary
            .entries
            .into_iter()
            .map(|entry| HomeNotificationEntrySummary {
                id: entry.id,
                source_app: entry.source_app,
                kind: entry.kind,
                title: entry.title,
                body: entry.body,
                action_ref: entry
                    .action_ref
                    .map(|action_ref| HomeNotificationActionSummary {
                        app: action_ref.app,
                        action_id: action_ref.action_id,
                    }),
                severity: home_notification_severity(entry.severity).to_string(),
                read: entry.read,
                created_at: entry.created_at,
            })
            .collect(),
    }
}

fn home_notification_severity(
    severity: crate::notifications::NotificationSeverity,
) -> &'static str {
    match severity {
        crate::notifications::NotificationSeverity::Info => "info",
        crate::notifications::NotificationSeverity::Attention => "attention",
        crate::notifications::NotificationSeverity::Critical => "critical",
    }
}

#[derive(Deserialize)]
struct HomeAttachResponse {
    token: String,
}

#[derive(Deserialize)]
struct HomeCapsulesResponse {
    capsules: Vec<HomeCapsuleInfo>,
}

#[derive(Deserialize)]
struct HomeCapsuleInfo {
    name: String,
}

async fn home_runtime_summary(data_dir: &std::path::Path) -> HomeRuntimeSummary {
    let Some(coords) = load_live_runtime_coords(data_dir).await else {
        return HomeRuntimeSummary {
            running: false,
            note: Some("No active local runtime".to_string()),
            ..HomeRuntimeSummary::default()
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return HomeRuntimeSummary {
                running: true,
                kind: Some(coords.runtime_kind.clone()),
                api_url: Some(coords.api_url.clone()),
                pid: Some(coords.pid),
                note: Some(format!("Runtime client unavailable: {err}")),
                ..HomeRuntimeSummary::default()
            };
        }
    };

    let mut runtime = HomeRuntimeSummary {
        running: true,
        kind: Some(coords.runtime_kind.clone()),
        version: home_fetch_runtime_version(&client, &coords.api_url).await,
        api_url: Some(coords.api_url.clone()),
        pid: Some(coords.pid),
        running_capsules: Vec::new(),
        note: None,
    };

    let home_token = match home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await
    {
        Ok(token) => token,
        Err(err) => {
            runtime.note = Some(format!("Runtime attach failed: {err}"));
            return runtime;
        }
    };

    match home_list_runtime_capsules(&client, &coords.api_url, &home_token).await {
        Ok(capsules) => runtime.running_capsules = capsules,
        Err(err) => {
            runtime.note = Some(format!(
                "Runtime attached, but capsule list is unavailable: {err}"
            ))
        }
    }

    runtime
}

async fn ensure_home_runtime(data_dir: &std::path::Path) -> HomeRuntimeSummary {
    match crate::runtime_control::ensure_runtime_for_home(data_dir).await {
        Ok(_) => home_runtime_summary(data_dir).await,
        Err(err) => HomeRuntimeSummary {
            running: false,
            note: Some(format!("Managed local runtime could not start: {err}")),
            ..HomeRuntimeSummary::default()
        },
    }
}

async fn load_live_runtime_coords(
    data_dir: &std::path::Path,
) -> Option<crate::runtime_control::RuntimeCoords> {
    let path = crate::runtime_control::runtime_coord_path(data_dir);
    crate::runtime_control::read_runtime_coords(&path).await
}

async fn home_fetch_runtime_version(client: &reqwest::Client, api_url: &str) -> Option<String> {
    client
        .get(format!("{}/api/health", api_url))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?
        .get("version")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

async fn home_attach_shell(
    client: &reqwest::Client,
    api_url: &str,
    attach_secret: &str,
) -> anyhow::Result<String> {
    gateway_attach_runtime_token(client, api_url, attach_secret, "shell").await
}

async fn gateway_attach_runtime_token(
    client: &reqwest::Client,
    api_url: &str,
    attach_secret: &str,
    scope: &str,
) -> anyhow::Result<String> {
    Ok(client
        .post(format!("{}/api/auth/attach", api_url))
        .json(&serde_json::json!({
            "secret": attach_secret,
            "scope": scope,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<HomeAttachResponse>()
        .await?
        .token)
}

async fn home_list_runtime_capsules(
    client: &reqwest::Client,
    api_url: &str,
    home_token: &str,
) -> anyhow::Result<Vec<String>> {
    let response = client
        .get(format!("{}/api/capsules", api_url))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .send()
        .await?
        .error_for_status()?
        .json::<HomeCapsulesResponse>()
        .await?;
    let mut names = response
        .capsules
        .into_iter()
        .map(|capsule| capsule.name)
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

#[derive(Debug, Deserialize)]
struct EdgeBinding {
    target: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadPayload {
    schema: String,
    target: String,
    #[serde(default)]
    bundle_cid: Option<String>,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
    content_digest: String,
    entry_count: u64,
    total_bytes: u64,
    activated_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadEnvelope {
    payload: SiteHeadPayload,
    signature: String,
    signer_did: String,
}

struct ResolvedSiteRoot {
    target: String,
    explicit_binding: bool,
}

#[derive(Debug, Deserialize)]
struct RoomPollBody {
    #[serde(default)]
    since: u64,
}

#[derive(Debug, Deserialize)]
struct RoomSendBody {
    body: String,
}

#[derive(Debug, Deserialize)]
struct ChatRoomAccessPolicyBody {
    allow_guest_invites: bool,
    allow_member_invites: bool,
    allow_members_to_host_guests: bool,
}

#[derive(Debug, Deserialize)]
struct ChatRoomMemberInviteBody {
    member_did: String,
    #[serde(default)]
    role: Option<crate::room_service::RoomRole>,
}

#[derive(Debug, Deserialize)]
struct ChatRoomMemberRemoveBody {
    member_did: String,
}

#[derive(Debug, Deserialize)]
struct ChatRoomInviteRevokeBody {
    invite_id: String,
}

#[derive(Debug, Serialize)]
struct ChatRoomGuestKickResponse {
    status: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Deserialize)]
struct RoomUploadStartBody {
    file_name: String,
    #[serde(default)]
    mime_type: String,
    size_bytes: u64,
}

async fn serve_public_root(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, "").await {
        Ok(response) => response,
        Err(status) if resolved.explicit_binding => (status, "Not found").into_response(),
        Err(_) => landing_page().await.into_response(),
    }
}

async fn room_service_summary(State(state): State<GatewayState>) -> Response {
    let data_dir = state.data_dir.clone();
    let summary_result =
        tokio::task::spawn_blocking(move || load_room_summary_with_identity(&data_dir)).await;
    match summary_result {
        Ok(Ok(mut summary)) => {
            summary.transport = room_transport_view(&state, None).await;
            Json(GatewayRoomSummary::from(summary)).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_summary(State(state): State<GatewayState>) -> Response {
    room_service_summary(State(state)).await
}

async fn chat_room_session_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let secure = request_uses_tls(&headers);
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || start_chat_room_session(&data_dir)).await {
        Ok(Ok(output)) => {
            let mut response = Json(ChatRoomSessionStartResponse {
                status: "connected".to_string(),
                display_name: output.display_name,
                expires_at: output.expires_at,
            })
            .into_response();
            match set_room_session_cookie_header(&output.token, output.max_age_secs, secure) {
                Ok(cookie) => {
                    response.headers_mut().append(SET_COOKIE, cookie);
                    response
                }
                Err(err) => room_service_error_response(err),
            }
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_request_approve(
    State(state): State<GatewayState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome = crate::room_service::approve_request(&data_dir, &request_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "browser access request not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_request_deny(
    State(state): State<GatewayState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome =
            crate::room_service::deny_request(&data_dir, &request_id, "Denied from Chat Room.")?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "browser access request not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_guest_kick(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome = crate::room_service::revoke_guest_session_by_id(&data_dir, &session_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(ChatRoomGuestKickResponse {
            status: "kicked".to_string(),
            display_name: output.display_name,
            device_label: output.device_label,
        })
        .into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "guest session not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_access_policy_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomAccessPolicyBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::update_room_access_policy(
            &data_dir,
            crate::room_service::RoomAccessPolicyUpdateInput {
                actor_did,
                allow_guest_invites: body.allow_guest_invites,
                allow_member_invites: body.allow_member_invites,
                allow_members_to_host_guests: body.allow_members_to_host_guests,
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_member_invite(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomMemberInviteBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::invite_room_member(
            &data_dir,
            crate::room_service::RoomInviteInput {
                actor_did,
                invited_did: body.member_did,
                role: body.role.unwrap_or(crate::room_service::RoomRole::Member),
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_member_remove(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomMemberRemoveBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::remove_room_member(
            &data_dir,
            crate::room_service::RoomMemberRemoveInput {
                actor_did,
                member_did: body.member_did,
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "room member not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn chat_room_invite_revoke(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomInviteRevokeBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output =
            crate::room_service::revoke_room_invite(&data_dir, &actor_did, &body.invite_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "room invite not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

fn load_room_summary_with_identity(
    data_dir: &std::path::Path,
) -> anyhow::Result<crate::room_service::RoomSummary> {
    let identity = room_service_runtime_identity_profile(data_dir);
    let mut summary = crate::room_service::load_summary(data_dir)?;
    if let Ok(hosted) = crate::browser_app_hosts::load_browser_app_hosted_endpoint(
        data_dir,
        crate::room_service::room_slug(),
    ) {
        summary.canonical_hosted_guest_url = hosted.canonical_url;
        summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    }
    let access = crate::room_service::local_runtime_access(data_dir, identity.did.as_deref())?;
    apply_room_access(&mut summary, access);
    Ok(summary)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayIdentityProfile {
    pub did: Option<String>,
}

pub(crate) fn room_service_runtime_identity_profile(
    data_dir: &std::path::Path,
) -> GatewayIdentityProfile {
    let did = load_existing_gateway_runtime_did(data_dir);
    GatewayIdentityProfile { did }
}

fn load_existing_gateway_runtime_did(data_dir: &std::path::Path) -> Option<String> {
    let device_key = data_dir.join("identity").join("device.key");
    if !device_key.exists() {
        return None;
    }
    elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_signing_key, did)| did)
        .filter(|did| !did.trim().is_empty())
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRuntimeCoords {
    api_url: String,
    attach_secret: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAttachResponse {
    token: String,
}

fn load_runtime_coords(data_dir: &std::path::Path) -> Option<GatewayRuntimeCoords> {
    let path = data_dir.join("runtime-coords.json");
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn gateway_internal_error(err: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
}

fn documents_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") {
        StatusCode::FORBIDDEN
    } else if text.contains("documents provider unavailable") {
        StatusCode::SERVICE_UNAVAILABLE
    } else if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid")
        || text.contains("empty")
        || text.contains("must not")
        || text.contains("missing")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn inbox_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") {
        StatusCode::FORBIDDEN
    } else if text.contains("unknown inbox action") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn system_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") {
        StatusCode::FORBIDDEN
    } else if text.contains("nickname must")
        || text.contains("missing")
        || text.contains("background image")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn home_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") || text.contains("gateway identity") {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

pub(crate) fn home_session_cookie_header(
    data_dir: &std::path::Path,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let token = issue_home_launch_token(data_dir, HOME_CAPSULE_ID)?;
    home_launch_cookie_header(
        HOME_SESSION_COOKIE,
        &token,
        HOME_LAUNCH_TOKEN_TTL_SECS,
        "/api/apps/home",
        secure,
    )
}

fn home_launch_cookie_header(
    name: &str,
    token: &str,
    max_age_secs: u64,
    path: &str,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value =
        format!("{name}={token}; Max-Age={max_age_secs}; Path={path}; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn issue_home_launch_token(data_dir: &std::path::Path, app: &str) -> anyhow::Result<String> {
    let context = local_home_launch_token_context(data_dir)?;
    issue_home_launch_token_with_context(data_dir, app, &context)
}

fn local_home_launch_token_context(
    data_dir: &std::path::Path,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let (_signing_key, did) = elastos_identity::load_or_create_did(data_dir)?;
    Ok(HomeLaunchTokenContext {
        principal_id: elastos_runtime::auth::PrincipalId::device_did(&did)
            .as_str()
            .to_string(),
        session_id: format!("local:{}", uuid_like_token()),
        proof_binding_id: None,
        grant_id: format!("grant:local:{}", uuid_like_token()),
    })
}

fn uuid_like_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub(super) fn issue_home_launch_token_with_context(
    data_dir: &std::path::Path,
    app: &str,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    let (signing_key, _did) = elastos_identity::load_or_create_did(data_dir)?;
    let now = now_ts();
    let envelope = HomeLaunchTokenEnvelope {
        payload: HomeLaunchTokenPayload {
            schema: "elastos.home.launch-token/v2".to_string(),
            app: app.to_string(),
            iat: now,
            exp: now + HOME_LAUNCH_TOKEN_TTL_SECS,
            principal_id: context.principal_id.clone(),
            session_id: context.session_id.clone(),
            proof_binding_id: context.proof_binding_id.clone(),
            grant_id: context.grant_id.clone(),
            non_delegatable: true,
        },
        signature: String::new(),
        signer_did: String::new(),
    };
    let canonical = serde_json::to_string(&serde_json::to_value(&envelope.payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        HOME_LAUNCH_TOKEN_DOMAIN,
        canonical.as_bytes(),
    );
    let envelope = HomeLaunchTokenEnvelope {
        signature,
        signer_did,
        ..envelope
    };
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope)?))
}

pub(super) fn require_home_launch_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<()> {
    require_home_launch_token_for_any(data_dir, headers, &[expected_app]).map(|_| ())
}

pub(super) fn require_home_launch_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_context(data_dir, headers, &[expected_app])
}

pub(super) fn require_home_launch_token_for_any(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<String> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| required.app)
}

pub(super) fn require_home_launch_token_for_any_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| required.context)
}

pub(super) fn require_home_launch_token_for_any_app_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<(String, HomeLaunchTokenContext)> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| (required.app, required.context))
}

fn require_home_token(data_dir: &std::path::Path, headers: &HeaderMap) -> anyhow::Result<()> {
    require_home_launch_token_for_any_from(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID],
        Some(HOME_SESSION_COOKIE),
    )
    .map(|_| ())
}

fn require_home_launch_token_for_any_from(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let token = headers
        .get("x-elastos-home-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| cookie_name.and_then(|name| cookie_value_from_headers(headers, name)))
        .ok_or_else(|| anyhow::anyhow!("missing home launch token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_str())
        .map_err(|_| anyhow::anyhow!("invalid home launch token encoding"))?;
    let envelope: HomeLaunchTokenEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid home launch token payload"))?;
    if envelope.payload.schema != "elastos.home.launch-token/v2" {
        anyhow::bail!("unsupported home launch token schema");
    }
    let local_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    let expected_dids = vec![local_did];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        HOME_LAUNCH_TOKEN_DOMAIN,
        &expected_dids,
    )
    .map_err(|err| anyhow::anyhow!("invalid home launch token: {}", err))?;
    if !allowed_apps.iter().any(|app| envelope.payload.app == *app) {
        anyhow::bail!("home launch token is not authorized for this provider");
    }
    if envelope.payload.exp <= now_ts() {
        anyhow::bail!("home launch token expired");
    }
    if !envelope.payload.non_delegatable {
        anyhow::bail!("home launch token must be non-delegatable");
    }
    if envelope.payload.session_id.trim().is_empty()
        || envelope.payload.principal_id.trim().is_empty()
        || envelope.payload.grant_id.trim().is_empty()
    {
        anyhow::bail!("home launch token is missing authority context");
    }
    if envelope.payload.proof_binding_id.is_some()
        && !crate::auth::is_auth_session_active(data_dir, &envelope.payload.session_id, now_ts())?
    {
        anyhow::bail!("home launch token auth session is not active");
    }
    Ok(RequiredHomeLaunchToken {
        app: envelope.payload.app,
        context: HomeLaunchTokenContext {
            principal_id: envelope.payload.principal_id,
            session_id: envelope.payload.session_id,
            proof_binding_id: envelope.payload.proof_binding_id,
            grant_id: envelope.payload.grant_id,
        },
    })
}

struct ChatRoomSessionGrant {
    token: String,
    display_name: String,
    expires_at: u64,
    max_age_secs: u64,
}

fn start_chat_room_session(data_dir: &std::path::Path) -> anyhow::Result<ChatRoomSessionGrant> {
    let identity = load_gateway_identity_summary(data_dir);
    let did = identity
        .device_did
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    let handle = identity
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Local runtime");
    let session =
        crate::room_service::start_local_runtime_session(data_dir, did, handle, "ElastOS shell")?;
    Ok(ChatRoomSessionGrant {
        max_age_secs: session.expires_at.saturating_sub(now_ts()),
        token: session.token,
        display_name: session.display_name,
        expires_at: session.expires_at,
    })
}

fn ensure_local_room_owner_or_actor(data_dir: &std::path::Path) -> anyhow::Result<String> {
    let identity = load_gateway_identity_summary(data_dir);
    let did = identity
        .device_did
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    let control = crate::room_service::load_room_control(data_dir)?;
    if control.owner_did.is_none() {
        let _ = crate::room_service::seed_room_owner(
            data_dir,
            crate::room_service::RoomOwnerSeedInput {
                owner_did: did.clone(),
                title: "Chat Room".to_string(),
            },
        )?;
    }
    Ok(did)
}

fn attach_client_token_blocking(
    client: &reqwest::blocking::Client,
    coords: &GatewayRuntimeCoords,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/auth/attach", coords.api_url))
        .json(&serde_json::json!({
            "secret": coords.attach_secret,
            "scope": "client",
        }))
        .send()
        .ok()?
        .json()
        .ok()?;
    serde_json::from_value::<GatewayAttachResponse>(body)
        .ok()
        .map(|resp| resp.token)
}

fn request_attached_capability_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    resource: &str,
    action: &str,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/capability/request", api))
        .header("Authorization", format!("Bearer {}", client_token))
        .json(&serde_json::json!({
            "resource": resource,
            "action": action,
        }))
        .send()
        .ok()?
        .json()
        .ok()?;

    if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
        return Some(token.to_string());
    }

    let request_id = body.get("request_id").and_then(|r| r.as_str())?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let status: serde_json::Value = client
            .get(format!("{}/api/capability/request/{}", api, request_id))
            .header("Authorization", format!("Bearer {}", client_token))
            .send()
            .ok()?
            .json()
            .ok()?;
        if let Some(token) = status.get("token").and_then(|t| t.as_str()) {
            return Some(token.to_string());
        }
        match status.get("status").and_then(|s| s.as_str()) {
            Some("denied") | Some("expired") => return None,
            _ => {}
        }
    }
    None
}

fn did_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    did_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let body: serde_json::Value = client
        .post(format!("{}/api/provider/did/{}", api, op))
        .header("Authorization", format!("Bearer {}", client_token))
        .header("X-Capability-Token", did_cap)
        .json(&body)
        .send()?
        .json()?;
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown did-provider error")
        );
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct GatewayGossipMessage {
    sender_id: String,
    content: String,
    ts: u64,
    #[serde(default)]
    signature: Option<String>,
}

struct AttachedRoomRuntimeBlocking {
    client: reqwest::blocking::Client,
    api_url: String,
    client_token: String,
    did_cap: String,
    peer_cap: String,
    did: String,
}

async fn room_transport_view(
    state: &GatewayState,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> crate::room_service::RoomTransportView {
    let data_dir = state.data_dir.clone();
    let topic = room_transport_topic(crate::room_service::room_slug());
    match tokio::task::spawn_blocking(move || sync_room_transport_blocking(&data_dir, outbound))
        .await
    {
        Ok(Ok(view)) => view,
        Ok(Err(err)) => room_transport_error_view(&topic, &err.to_string()),
        Err(err) => {
            room_transport_error_view(&topic, &format!("room transport task failed: {err}"))
        }
    }
}

fn room_transport_topic(room_slug: &str) -> String {
    format!("__elastos_internal/room-sync-v1/{room_slug}")
}

fn room_transport_error_view(topic: &str, detail: &str) -> crate::room_service::RoomTransportView {
    crate::room_service::RoomTransportView {
        available: false,
        connected_peer_count: 0,
        topic: Some(topic.to_string()),
        status: Some(format!("Carrier room sync unavailable: {detail}")),
    }
}

fn sync_room_transport_blocking(
    data_dir: &std::path::Path,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> anyhow::Result<crate::room_service::RoomTransportView> {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let runtime = attach_room_runtime_blocking(data_dir)?;
    let access = crate::room_service::local_runtime_access(data_dir, Some(&runtime.did))?;
    let Some(_member_role) = access.member_role else {
        return Ok(crate::room_service::RoomTransportView {
            available: false,
            connected_peer_count: 0,
            topic: Some(topic),
            status: Some(access.block_reason.unwrap_or_else(|| {
                "Carrier room sync inactive: this runtime is not an active room member.".to_string()
            })),
        });
    };

    let outbound_envelopes =
        crate::room_service::room_transport_backlog(data_dir, &runtime.did, outbound.as_ref())?;
    join_room_transport_topic_blocking(&runtime, &topic)?;
    for envelope in &outbound_envelopes {
        send_room_object_envelope_blocking(&runtime, &topic, envelope)?;
    }

    let mut imported = 0usize;
    let mut dropped = 0usize;
    for message in recv_room_transport_messages_blocking(&runtime, &topic)? {
        match verify_and_decode_room_message_blocking(&runtime, &message) {
            Some(envelope) => {
                match crate::room_service::ingest_room_object_envelope(data_dir, &envelope) {
                    Ok(Some(_)) => imported += 1,
                    Ok(None) => {}
                    Err(_) => dropped += 1,
                }
            }
            None => dropped += 1,
        }
    }

    let connected_peer_count = list_room_transport_peers_blocking(&runtime, &topic)?.len();
    let mut status = if connected_peer_count > 0 {
        format!(
            "Carrier room sync connected to {} runtime{}.",
            connected_peer_count,
            if connected_peer_count == 1 { "" } else { "s" }
        )
    } else {
        "Carrier room sync ready; waiting for another room member runtime.".to_string()
    };
    if imported > 0 {
        status.push_str(&format!(
            " Imported {} new message{}.",
            imported,
            if imported == 1 { "" } else { "s" }
        ));
    }
    if dropped > 0 {
        status.push_str(&format!(
            " Ignored {} invalid item{}.",
            dropped,
            if dropped == 1 { "" } else { "s" }
        ));
    }

    Ok(crate::room_service::RoomTransportView {
        available: true,
        connected_peer_count,
        topic: Some(topic),
        status: Some(status),
    })
}

fn attach_room_runtime_blocking(
    data_dir: &std::path::Path,
) -> anyhow::Result<AttachedRoomRuntimeBlocking> {
    let coords = load_runtime_coords(data_dir)
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let client_token = attach_client_token_blocking(&client, &coords)
        .ok_or_else(|| anyhow::anyhow!("failed to attach to local runtime"))?;
    let did_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://did/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire DID capability"))?;
    let peer_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://peer/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire Carrier peer capability"))?;
    let did = did_provider_request_blocking(
        &client,
        &coords.api_url,
        &client_token,
        &did_cap,
        "get_did",
        serde_json::json!({}),
    )?
    .get("data")
    .and_then(|d| d.get("did"))
    .and_then(|value| value.as_str())
    .map(str::trim)
    .filter(|did| !did.is_empty())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    Ok(AttachedRoomRuntimeBlocking {
        client,
        api_url: coords.api_url,
        client_token,
        did_cap,
        peer_cap,
        did,
    })
}

fn peer_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .post(format!("{api}/api/provider/peer/{op}"))
        .header(AUTHORIZATION, format!("Bearer {client_token}"))
        .header("X-Capability-Token", peer_cap)
        .json(&body)
        .send()?;
    let body: serde_json::Value = response.json()?;
    if body.get("status").and_then(|status| status.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("unknown peer-provider error")
        );
    }
    Ok(body)
}

fn join_room_transport_topic_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<()> {
    match peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join",
        serde_json::json!({ "topic": topic }),
    ) {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("already joined") => Ok(()),
        Err(err) => Err(err),
    }
}

fn list_room_transport_peers_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<Vec<String>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "list_topic_peers",
        serde_json::json!({ "topic": topic }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("peers"))
        .and_then(|value| value.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn recv_room_transport_messages_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<Vec<GatewayGossipMessage>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": topic,
            "limit": 64,
            "consumer_id": ROOM_SYNC_CONSUMER_ID,
            "skip_sender_id": runtime.did,
        }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    serde_json::from_value::<GatewayGossipMessage>(message.clone()).ok()
                })
                .collect()
        })
        .unwrap_or_default())
}

fn send_room_object_envelope_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    envelope: &crate::room_service::RoomObjectEnvelope,
) -> anyhow::Result<()> {
    if envelope.sender_member_did != runtime.did {
        anyhow::bail!(
            "room object signer {} does not match local runtime DID {}",
            envelope.sender_member_did,
            runtime.did
        );
    }
    let message = serde_json::to_string(envelope)?;
    let signature = sign_room_message_blocking(
        runtime,
        &envelope.sender_member_did,
        envelope.created_at,
        &message,
    )?;
    let _ = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": topic,
            "message": message,
            "sender": envelope.sender,
            "sender_id": envelope.sender_member_did,
            "ts": envelope.created_at,
            "signature": signature,
        }),
    )?;
    Ok(())
}

fn verify_and_decode_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    message: &GatewayGossipMessage,
) -> Option<crate::room_service::RoomObjectEnvelope> {
    if message.sender_id.trim().is_empty() || message.ts == 0 {
        return None;
    }
    let signature = message.signature.as_deref().unwrap_or_default();
    if !verify_room_message_blocking(
        runtime,
        &message.sender_id,
        message.ts,
        &message.content,
        signature,
    ) {
        return None;
    }
    let envelope: crate::room_service::RoomObjectEnvelope =
        serde_json::from_str(&message.content).ok()?;
    if envelope.sender_member_did != message.sender_id || envelope.created_at != message.ts {
        return None;
    }
    Some(envelope)
}

fn sign_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> anyhow::Result<String> {
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "sign",
        serde_json::json!({ "data": payload_hex }),
    )?
    .get("data")
    .and_then(|data| data.get("signature"))
    .and_then(|value| value.as_str())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("did-provider sign response missing signature"))
}

fn verify_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
    signature: &str,
) -> bool {
    if sender_id.trim().is_empty() || signature.trim().is_empty() || ts == 0 {
        return false;
    }
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "verify",
        serde_json::json!({
            "did": sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    )
    .ok()
    .and_then(|body| {
        body.get("data")
            .and_then(|data| data.get("valid"))
            .and_then(|value| value.as_bool())
    })
    .unwrap_or(false)
}

async fn room_service_session_leave(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let secure = request_uses_tls(&headers);
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || crate::room_service::leave_session(&data_dir, &token))
        .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, None).await;
            let mut response = Json(output).into_response();
            let clear_room_cookie = match clear_room_session_cookie_header(secure) {
                Ok(value) => value,
                Err(err) => return room_service_error_response(err),
            };
            let clear_browser_cookie = match clear_browser_session_cookie_header(secure) {
                Ok(value) => value,
                Err(err) => return room_service_error_response(err),
            };
            response.headers_mut().append(SET_COOKIE, clear_room_cookie);
            response
                .headers_mut()
                .append(SET_COOKIE, clear_browser_cookie);
            response
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_poll(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomPollBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let transport = room_transport_view(&state, None).await;
    match tokio::task::spawn_blocking(move || {
        crate::room_service::room_poll(&data_dir, &token, body.since)
    })
    .await
    {
        Ok(Ok(mut output)) => {
            output.transport = transport;
            Json(output).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_objects_send(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomSendBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_object_with_transport(&data_dir, &token, &body.body)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomUploadStartBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::start_attachment_upload(
            &data_dir,
            &token,
            &body.file_name,
            &body.mime_type,
            body.size_bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_chunk(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let offset = match upload_offset_from_headers(&headers) {
        Ok(offset) => offset,
        Err(err) => return room_service_error_response(err),
    };
    let bytes = body.to_vec();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_attachment_upload_chunk(
            &data_dir, &token, &upload_id, offset, &bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_finish(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::finish_attachment_upload(&data_dir, &token, &upload_id)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_attachment_get(
    State(state): State<GatewayState>,
    Path(attachment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::read_attachment(&data_dir, &token, &attachment_id)
    })
    .await
    {
        Ok(Ok((attachment, bytes))) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_str(&attachment.mime_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            let disposition = if attachment.is_image || attachment.is_audio || attachment.is_video {
                "inline"
            } else {
                "attachment"
            };
            let content_disposition = format!(
                "{}; filename=\"{}\"",
                disposition,
                attachment.file_name.replace('"', "")
            );
            headers.insert(
                "content-disposition",
                HeaderValue::from_str(&content_disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            headers.insert("cache-control", HeaderValue::from_static("no-store"));
            (StatusCode::OK, headers, bytes).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(crate) fn request_uses_tls(headers: &HeaderMap) -> bool {
    if headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
    {
        return true;
    }

    if headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("proto=https"))
    {
        return true;
    }

    request_host(headers).is_some_and(|host| !request_host_is_local(&host))
}

fn request_host_is_local(host: &str) -> bool {
    let host = host
        .trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let host = if host == "::1" {
        host.as_str()
    } else {
        host.split(':').next().unwrap_or(host.as_str())
    };
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

pub(crate) fn set_room_session_cookie_header(
    token: &str,
    max_age_secs: u64,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{ROOM_SESSION_COOKIE}={token}; Max-Age={max_age_secs}; Path=/; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn set_browser_session_cookie_header(
    token: &str,
    max_age_secs: u64,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{BROWSER_SESSION_COOKIE}={token}; Max-Age={max_age_secs}; Path=/; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn clear_room_session_cookie_header(secure: bool) -> anyhow::Result<HeaderValue> {
    let mut value = format!("{ROOM_SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn clear_browser_session_cookie_header(secure: bool) -> anyhow::Result<HeaderValue> {
    let mut value = format!("{BROWSER_SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn room_session_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    if let Ok(token) = bearer_token_from_headers(headers) {
        return Ok(token);
    }
    if let Some(token) = cookie_value_from_headers(headers, ROOM_SESSION_COOKIE) {
        return Ok(token);
    }
    if let Some(token) = cookie_value_from_headers(headers, BROWSER_SESSION_COOKIE) {
        return Ok(token);
    }
    anyhow::bail!(
        "missing room session. Expected Authorization: Bearer <token> or room/browser session cookie"
    )
}

fn chat_room_access_token_from_headers(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<String> {
    if headers.contains_key("x-elastos-home-token") {
        require_home_launch_token(data_dir, headers, CHAT_ROOM_CAPSULE_ID)?;
        return Ok(start_chat_room_session(data_dir)?.token);
    }
    room_session_token_from_headers(headers)
}

pub(crate) fn cookie_value_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie_header| {
            cookie_header.split(';').map(str::trim).find_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                if key.trim() == name {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            })
        })
        .filter(|value| !value.is_empty())
}

fn bearer_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing Authorization header. Expected: Bearer <token>"))
}

fn upload_offset_from_headers(headers: &HeaderMap) -> anyhow::Result<u64> {
    headers
        .get("x-elastos-upload-offset")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("missing x-elastos-upload-offset header"))?
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid x-elastos-upload-offset header"))
}

fn room_service_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid or expired session")
        || text.contains("missing room session")
        || text.contains("home launch token")
    {
        StatusCode::UNAUTHORIZED
    } else if text.contains("not an active member") || text.contains("cannot pair") {
        StatusCode::FORBIDDEN
    } else if text.contains("must not be empty")
        || text.contains("characters or fewer")
        || text.contains("exceeds")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn room_service_join_error_response(err: tokio::task::JoinError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("group chat task failed: {}", err),
    )
        .into_response()
}

async fn resolve_bound_site_root(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<ResolvedSiteRoot, StatusCode> {
    let Some(host) = request_host(headers) else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding_path = edge_binding_path(&state.data_dir, &host);
    let Ok(bytes) = tokio::fs::read(&binding_path).await else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding: EdgeBinding =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
    if rooted_localhost_fs_path(&state.data_dir, &binding.target).is_none() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(ResolvedSiteRoot {
        target: binding.target,
        explicit_binding: true,
    })
}

fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))?
        .to_str()
        .ok()?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix('[') {
        let end = stripped.find(']')?;
        return Some(stripped[..end].to_string());
    }
    Some(raw.split(':').next().unwrap_or("").to_string())
}

async fn healthz() -> &'static str {
    "OK"
}

async fn serve_release_manifest(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_manifest_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release.json not found").into_response(),
    }
}

async fn serve_release_head(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_head_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release-head.json not found").into_response(),
    }
}

async fn serve_artifact_file(
    State(state): State<GatewayState>,
    Path(path): Path<String>,
) -> Response {
    if let Err(msg) = validate_file_path(&path) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let artifacts_root = publisher_artifacts_path(&state.data_dir);
    let requested = artifacts_root.join(&path);
    let Ok(root_canonical) = tokio::fs::canonicalize(&artifacts_root).await else {
        return (StatusCode::NOT_FOUND, "artifacts not found").into_response();
    };
    let Ok(requested_canonical) = tokio::fs::canonicalize(&requested).await else {
        return (StatusCode::NOT_FOUND, "artifact not found").into_response();
    };
    if !requested_canonical.starts_with(&root_canonical) {
        return (StatusCode::BAD_REQUEST, "Path traversal not allowed").into_response();
    }

    match tokio::fs::read(&requested_canonical).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", content_type(&path))],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "artifact not found").into_response(),
    }
}

async fn serve_install_script(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let path = publisher_install_script_path(&state.data_dir);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        // Dynamically stamp the publisher gateway URL so `curl <gw>/install.sh | bash`
        // automatically embeds this gateway for future `elastos update`.
        let script = String::from_utf8_lossy(&bytes);
        let stamped = if script.contains("__PUBLISHER_GATEWAY__") {
            if let Some(host) = headers
                .get("x-forwarded-host")
                .or_else(|| headers.get("host"))
                .and_then(|v| v.to_str().ok())
            {
                let scheme = headers
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("https");
                let gateway_url = format!("{}://{}", scheme, host.trim_end_matches('/'));
                script
                    .replace("__PUBLISHER_GATEWAY__", &gateway_url)
                    .into_bytes()
            } else {
                bytes
            }
        } else {
            bytes
        };
        return (
            StatusCode::OK,
            [("content-type", "text/x-shellscript")],
            stamped,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "install.sh not found").into_response()
}

async fn serve_site_head_document(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    let Some(site_head) = load_site_head(&state, &resolved.target).await else {
        return (StatusCode::NOT_FOUND, "site head not found").into_response();
    };
    match serde_json::to_vec(&site_head) {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "site head encode failed").into_response(),
    }
}

async fn serve_public_site_path(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, &path).await {
        Ok(response) => response,
        Err(status) => (status, "Not found").into_response(),
    }
}

async fn load_site_head(state: &GatewayState, site_root_uri: &str) -> Option<SiteHeadEnvelope> {
    let path = edge_site_head_path(&state.data_dir, site_root_uri);
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn redirect_cid_root(Path(cid): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/s/{}/", cid))
}

async fn serve_cid_root(State(state): State<GatewayState>, Path(cid): Path<String>) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    serve_directory_root(&state, &cid).await
}

async fn serve_ipfs_cid_root(
    State(state): State<GatewayState>,
    Path(cid): Path<String>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    let raw_cache = state.cache_dir.join(format!("{}.raw", cid));
    if let Ok(bytes) = tokio::fs::read(&raw_cache).await {
        return (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response();
    }

    let cached_index = state.cache_dir.join(&cid).join("index.html");
    if cached_index.is_file() {
        return serve_directory_root(&state, &cid).await;
    }

    match fetch_file_inline(&state, &cid, "").await {
        Ok(bytes) => {
            let _ = tokio::fs::create_dir_all(&state.cache_dir).await;
            let _ = tokio::fs::write(&raw_cache, &bytes).await;
            (
                StatusCode::OK,
                [("content-type", "application/octet-stream")],
                bytes,
            )
                .into_response()
        }
        Err(_) => serve_directory_root(&state, &cid).await,
    }
}

async fn serve_directory_root(state: &GatewayState, cid: &str) -> Response {
    match serve_cid_path_result(state, cid, "index.html").await {
        Ok(response) => response,
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found in CID bundle").into_response(),
    }
}

async fn serve_cid_path_result(
    state: &GatewayState,
    cid: &str,
    file_path: &str,
) -> Result<Response, StatusCode> {
    validate_file_path(file_path).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Fast path: check local cache.
    let cid_dir = state.cache_dir.join(cid);
    let requested = cid_dir.join(file_path);
    if cid_dir.is_dir() {
        let canonical_cid_dir = cid_dir.canonicalize().unwrap_or_else(|_| cid_dir.clone());
        let canonical_requested = requested
            .canonicalize()
            .unwrap_or_else(|_| cid_dir.join(file_path));
        if canonical_requested.starts_with(&canonical_cid_dir) {
            if let Ok(bytes) = tokio::fs::read(&requested).await {
                let ct = content_type(file_path);
                return Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response());
            }
        }
    }

    // Cache miss — fetch the individual file inline via cat.
    match fetch_file_inline(state, cid, file_path).await {
        Ok(bytes) => {
            let cache_path = state.cache_dir.join(cid).join(file_path);
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &bytes).await;
            let ct = content_type(file_path);
            Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response())
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

fn stamp_site_headers(
    response: &mut Response,
    site_root_uri: &str,
    site_head: Option<&SiteHeadEnvelope>,
) -> Result<(), StatusCode> {
    let site_origin = HeaderValue::from_str(site_root_uri).map_err(|_| StatusCode::BAD_GATEWAY)?;
    response
        .headers_mut()
        .insert("X-Elastos-Site-Origin", site_origin);
    if let Some(site_head) = site_head {
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Schema",
            HeaderValue::from_str(&site_head.payload.schema)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Digest",
            HeaderValue::from_str(&site_head.payload.content_digest)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Signer",
            HeaderValue::from_str(&site_head.signer_did).map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Cid",
                HeaderValue::from_str(bundle_cid).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(release_name) = site_head.payload.release_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Release",
                HeaderValue::from_str(release_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(channel_name) = site_head.payload.channel_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Channel",
                HeaderValue::from_str(channel_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
    }
    Ok(())
}

async fn serve_site_file(
    state: &GatewayState,
    site_root_uri: &str,
    request_path: &str,
) -> Result<Response, StatusCode> {
    let requested = request_path.trim_start_matches('/');
    if !requested.is_empty() {
        validate_file_path(requested).map_err(|_| StatusCode::BAD_REQUEST)?;
    }

    let site_head = load_site_head(state, site_root_uri).await;
    if let Some(site_head) = site_head.as_ref() {
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            let bundle_candidates: Vec<String> = if requested.is_empty() {
                vec!["index.html".to_string()]
            } else {
                vec![requested.to_string(), format!("{}/index.html", requested)]
            };
            for bundle_path in bundle_candidates {
                if let Ok(mut response) =
                    serve_cid_path_result(state, bundle_cid, &bundle_path).await
                {
                    stamp_site_headers(&mut response, site_root_uri, Some(site_head))?;
                    return Ok(response);
                }
            }
            return Err(StatusCode::NOT_FOUND);
        }
    }

    let site_root =
        rooted_localhost_fs_path(&state.data_dir, site_root_uri).ok_or(StatusCode::BAD_GATEWAY)?;
    let mut candidates = Vec::new();
    if requested.is_empty() {
        candidates.push(site_root.join("index.html"));
    } else {
        candidates.push(site_root.join(requested));
        candidates.push(site_root.join(requested).join("index.html"));
    }

    let root_canonical = tokio::fs::canonicalize(&site_root)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    for candidate in candidates {
        let Ok(metadata) = tokio::fs::metadata(&candidate).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(candidate_canonical) = tokio::fs::canonicalize(&candidate).await else {
            continue;
        };
        if !candidate_canonical.starts_with(&root_canonical) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = tokio::fs::read(&candidate_canonical)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let path_for_type = candidate_canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index.html");
        let mut response = (
            StatusCode::OK,
            [("content-type", content_type(path_for_type))],
            bytes,
        )
            .into_response();
        stamp_site_headers(&mut response, site_root_uri, site_head.as_ref())?;
        return Ok(response);
    }

    Err(StatusCode::NOT_FOUND)
}

async fn serve_cid_file(
    State(state): State<GatewayState>,
    Path((cid, file_path)): Path<(String, String)>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    match serve_cid_path_result(&state, &cid, &file_path).await {
        Ok(response) => response,
        Err(StatusCode::BAD_REQUEST) => {
            (StatusCode::BAD_REQUEST, "Invalid file path").into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "File not found").into_response(),
    }
}

/// Fetch a single file from IPFS inline via the `cat` operation.
/// Returns raw bytes. Works with VM-based providers (data returned over vsock).
async fn fetch_file_inline(state: &GatewayState, cid: &str, path: &str) -> anyhow::Result<Vec<u8>> {
    let req = serde_json::json!({
        "op": "cat",
        "cid": cid,
        "path": path,
    });
    let resp = send_ipfs_raw(state, &req).await?;
    let status = resp
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("error");
    if status != "ok" {
        let msg = resp
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("{}", msg);
    }
    let data_b64 = resp
        .get("data")
        .and_then(|d| d.get("data"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("no data in cat response"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64)?;
    if bytes.len() > MAX_GATEWAY_FILE_SIZE {
        anyhow::bail!("file exceeds size limit");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------------

/// Validate a request file path — reject traversal, absolute paths, backslashes.
pub(super) fn validate_file_path(path: &str) -> Result<(), &'static str> {
    // Reject absolute paths
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("Absolute paths not allowed");
    }
    // Reject backslashes (Windows-style)
    if path.contains('\\') {
        return Err("Backslashes not allowed");
    }
    // Reject traversal (raw and URL-encoded)
    if path.contains("..") {
        return Err("Path traversal not allowed");
    }
    // Check URL-encoded traversal variants
    let decoded = path.replace("%2e", ".").replace("%2E", ".");
    if decoded.contains("..") {
        return Err("Encoded path traversal not allowed");
    }
    let decoded_slash = path.replace("%2f", "/").replace("%2F", "/");
    if decoded_slash.contains("..") {
        return Err("Encoded path traversal not allowed");
    }

    Ok(())
}

/// Send a raw request to ipfs-provider.
async fn send_ipfs_raw(
    state: &GatewayState,
    request: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("gateway provider registry unavailable"))?;
    registry
        .send_raw("ipfs", request)
        .await
        .map_err(|e| anyhow::anyhow!("provider registry ipfs request failed: {}", e))
}

// ---------------------------------------------------------------------------
// MIME types
// ---------------------------------------------------------------------------

pub(super) fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("txt" | "sh") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// CID validation (one-liner, avoids depending on main.rs)
// ---------------------------------------------------------------------------

fn is_valid_cid(s: &str) -> bool {
    cid::Cid::try_from(s).is_ok()
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

pub async fn start_gateway_server(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    let state = GatewayState {
        provider_registry,
        cache_dir,
        data_dir,
    };
    let app = gateway_router(state);
    let listener = TcpListener::bind(addr).await?;
    let advertised = advertised_gateway_urls(addr);
    println!("ElastOS Gateway v{}", GATEWAY_VERSION);
    println!("  Bind:      http://{}", addr);
    if let Some(primary) = advertised.first() {
        println!("  Open:      {}", primary);
        println!("  Room:      {}apps/chat-room/", primary);
        println!("  Content:   {}s/<cid>/", primary);
        for extra in advertised.iter().skip(1) {
            println!("  Also:      {}", extra);
        }
    } else {
        println!("  Open:      http://{}", addr);
        println!("  Room:      http://{}/apps/chat-room/", addr);
        println!("  Content:   http://{}/s/<cid>/", addr);
    }
    println!();
    println!("  Cache is unbounded (Tier 1) — delete cache dir to reclaim space");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            println!("\nShutting down gateway...");
        })
        .await?;
    Ok(())
}

pub(crate) fn advertised_gateway_urls(addr: &str) -> Vec<String> {
    let Ok(socket_addr) = addr.parse::<SocketAddr>() else {
        return vec![format!("http://{}/", addr.trim_end_matches('/'))];
    };

    let port = socket_addr.port();
    let host = socket_addr.ip();

    let mut urls = Vec::new();
    match host {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            urls.push(format!("http://127.0.0.1:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(format!("http://{}:{}/", ip, port));
            }
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            urls.push(format!("http://[::1]:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(match ip {
                    IpAddr::V4(ip) => format!("http://{}:{}/", ip, port),
                    IpAddr::V6(ip) => format!("http://[{}]:{}/", ip, port),
                });
            }
        }
        IpAddr::V4(ip) => {
            urls.push(format!("http://{}:{}/", ip, port));
        }
        IpAddr::V6(ip) => {
            urls.push(format!("http://[{}]:{}/", ip, port));
        }
    }

    dedupe_urls(urls)
}

fn detect_advertisable_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for part in stdout.split_whitespace() {
                if let Ok(ip) = part.parse::<IpAddr>() {
                    ips.push(ip);
                }
            }
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".parse().unwrap());
    }
    ips
}

fn dedupe_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for url in urls {
        if seen.insert(url.clone()) {
            deduped.push(url);
        }
    }
    deduped
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate.recv() => {},
            }
        } else {
            ctrl_c.await;
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "gateway_tests.rs"]
mod gateway_tests;
