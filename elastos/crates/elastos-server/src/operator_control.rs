use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use futures_lite::future::Boxed;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, SecretKey, Watcher};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

use crate::carrier::decode_ticket_endpoints;
use crate::crypto::{
    domain_separated_sign, verify_release_envelope, verify_release_envelope_against_dids,
};
use crate::local_http::LoopbackHttpBaseUrl;
use crate::sources::{load_trusted_sources, TrustedSource};
use crate::{
    browser_app_hosts::load_browser_app_hosted_endpoint,
    notifications::sync_room_notifications,
    room_service::{
        approve_next_request, approve_request, deny_next_request, deny_request, load_summary,
        local_runtime_access, room_slug, ApprovalOutcome, DenyOutcome, RoomSummary,
    },
};

pub const OPERATOR_ALPN: &[u8] = b"elastos/operator/1";
pub const OPERATOR_ACTION_STATUS_READ: &str = "status.read";
pub const OPERATOR_ACTION_UPDATE_CHECK: &str = "update.check";
pub const OPERATOR_ACTION_UPDATE_APPLY: &str = "update.apply";
pub const OPERATOR_ACTION_ROOM_READ: &str = "room.read";
pub const OPERATOR_ACTION_ROOM_APPROVE: &str = "room.approve";
pub const OPERATOR_ACTION_ROOM_DENY: &str = "room.deny";
pub const OPERATOR_ACTION_ROOM_OPEN: &str = "room.open";

const OPERATOR_CONFIG_SCHEMA: &str = "elastos.operator-control/v1";
const OPERATOR_REQUEST_DOMAIN: &str = "elastos.operator.request.v1";
const OPERATOR_RESPONSE_DOMAIN: &str = "elastos.operator.response.v1";
const OPERATOR_AUDIT_FILE: &str = "operator-control-audit.jsonl";
const OPERATOR_REQUEST_LOG_FILE: &str = "operator-control-requests.jsonl";
const OPERATOR_CONNECT_TIMEOUT_SECS: u64 = 10;
const OPERATOR_TS_SKEW_SECS: u64 = 5 * 60;
const REQUEST_STATE_RESERVED: &str = "reserved";
const REQUEST_STATE_COMPLETED: &str = "completed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorControlConfig {
    pub schema: String,
    #[serde(default)]
    pub peers: Vec<OperatorPeer>,
}

impl OperatorControlConfig {
    pub fn empty() -> Self {
        Self {
            schema: OPERATOR_CONFIG_SCHEMA.to_string(),
            peers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorPeer {
    pub did: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub connect_ticket: String,
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorSourceStatus {
    pub name: String,
    pub installed_version: String,
    pub channel: String,
    #[serde(default)]
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorNodeStatus {
    pub did: String,
    pub runtime_running: bool,
    #[serde(default)]
    pub runtime_kind: Option<String>,
    #[serde(default)]
    pub runtime_version: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub source: Option<OperatorSourceStatus>,
    #[serde(default)]
    pub peer_count: Option<usize>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub connect_ticket: Option<String>,
    #[serde(default)]
    pub running_capsules: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorUpdateCheck {
    pub source_name: String,
    pub channel: String,
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub discovery: String,
    #[serde(default)]
    pub working_gateway: Option<String>,
    #[serde(default)]
    pub head_cid: Option<String>,
    #[serde(default)]
    pub release_cid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorUpdateApply {
    pub source_name: String,
    pub channel: String,
    pub previous_version: String,
    pub installed_version: String,
    pub changed: bool,
    pub runtime_running: bool,
    #[serde(default)]
    pub runtime_version: Option<String>,
    pub restart_required: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorRoomOpen {
    pub bind_addr: String,
    #[serde(default)]
    pub open_urls: Vec<String>,
    #[serde(default)]
    pub room_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OperatorRoomApproveArgs {
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OperatorRoomDenyArgs {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperatorRoomOpenArgs {
    addr: String,
}

impl Default for OperatorRoomOpenArgs {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:8090".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedEnvelope<T> {
    payload: T,
    signature: String,
    signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperatorRequestPayload {
    version: u8,
    request_id: String,
    requester_did: String,
    target_did: String,
    ts: u64,
    action: String,
    #[serde(default)]
    args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperatorResponsePayload {
    version: u8,
    request_id: String,
    accepted: bool,
    target_did: String,
    ts: u64,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: u64,
    request_id: String,
    requester_did: String,
    action: String,
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestLogEntry {
    ts: u64,
    request_id: String,
    requester_did: String,
    action: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AttachResponse {
    token: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CapsulesResponse {
    capsules: Vec<CapsuleInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct CapsuleInfo {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SupervisorStartGatewayOutput {
    status: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PeerProviderEnvelope {
    data: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct OperatorRuntimeContext {
    pub data_dir: PathBuf,
    pub local_did: String,
    pub endpoint: Endpoint,
    pub peers: Arc<Mutex<Vec<String>>>,
    pub request_serial: Arc<Mutex<()>>,
}

#[derive(Debug, Clone)]
pub struct OperatorHandler {
    ctx: OperatorRuntimeContext,
}

impl OperatorHandler {
    pub fn new(ctx: OperatorRuntimeContext) -> Self {
        Self { ctx }
    }
}

#[allow(refining_impl_trait)]
impl ProtocolHandler for OperatorHandler {
    fn accept(
        &self,
        conn: iroh::endpoint::Connection,
    ) -> Boxed<std::result::Result<(), AcceptError>> {
        let ctx = self.ctx.clone();
        Box::pin(async move {
            handle_operator_connection(conn, ctx)
                .await
                .map_err(|e| AcceptError::from(std::io::Error::other(e.to_string())))
        })
    }
}

pub fn operator_control_path(data_dir: &Path) -> PathBuf {
    data_dir.join("operator-control.json")
}

pub fn supported_actions() -> &'static [&'static str] {
    &[
        OPERATOR_ACTION_STATUS_READ,
        OPERATOR_ACTION_UPDATE_CHECK,
        OPERATOR_ACTION_UPDATE_APPLY,
        OPERATOR_ACTION_ROOM_READ,
        OPERATOR_ACTION_ROOM_APPROVE,
        OPERATOR_ACTION_ROOM_DENY,
        OPERATOR_ACTION_ROOM_OPEN,
    ]
}

pub fn load_operator_control(data_dir: &Path) -> Result<OperatorControlConfig> {
    let path = operator_control_path(data_dir);
    if !path.exists() {
        return Ok(OperatorControlConfig::empty());
    }

    let data = std::fs::read_to_string(path)?;
    let mut config: OperatorControlConfig = serde_json::from_str(&data)?;
    if config.schema.is_empty() {
        config.schema = OPERATOR_CONFIG_SCHEMA.to_string();
    }
    for peer in &mut config.peers {
        peer.allow = normalize_actions(&peer.allow)?;
    }
    config.peers.sort_by(|a, b| a.did.cmp(&b.did));
    Ok(config)
}

pub fn save_operator_control(data_dir: &Path, config: &OperatorControlConfig) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = operator_control_path(data_dir);
    let data = serde_json::to_vec_pretty(config)?;
    std::fs::write(&path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn upsert_peer(data_dir: &Path, mut peer: OperatorPeer) -> Result<OperatorPeer> {
    validate_peer_did(&peer.did)?;
    peer.allow = normalize_actions(&peer.allow)?;

    let mut config = load_operator_control(data_dir)?;
    if let Some(existing) = config.peers.iter_mut().find(|entry| entry.did == peer.did) {
        if !peer.label.is_empty() {
            existing.label = peer.label.clone();
        }
        if !peer.connect_ticket.is_empty() {
            existing.connect_ticket = peer.connect_ticket.clone();
        }
        if !peer.allow.is_empty() {
            existing.allow = peer.allow.clone();
        }
        peer = existing.clone();
    } else {
        config.peers.push(peer.clone());
    }

    config.peers.sort_by(|a, b| a.did.cmp(&b.did));
    save_operator_control(data_dir, &config)?;
    Ok(peer)
}

pub fn remove_peer(data_dir: &Path, did: &str) -> Result<Option<OperatorPeer>> {
    validate_peer_did(did)?;

    let mut config = load_operator_control(data_dir)?;
    let removed = config
        .peers
        .iter()
        .position(|entry| entry.did == did)
        .map(|index| config.peers.remove(index));
    save_operator_control(data_dir, &config)?;
    Ok(removed)
}

pub async fn gather_local_node_status(data_dir: &Path) -> Result<OperatorNodeStatus> {
    let (_signing_key, did) = elastos_identity::load_or_create_did(data_dir)?;
    let source = load_default_source_status(data_dir);
    let Some(coords) = read_runtime_coords(data_dir).await else {
        return Ok(OperatorNodeStatus {
            did,
            runtime_running: false,
            source,
            note: Some("No active local runtime".to_string()),
            ..OperatorNodeStatus::default()
        });
    };

    let client = build_http_client(3)?;
    let runtime_version = fetch_runtime_version(&client, &coords.api_url).await;
    let mut status = OperatorNodeStatus {
        did,
        runtime_running: true,
        runtime_kind: Some(coords.runtime_kind.clone()),
        runtime_version,
        api_url: Some(coords.api_url.clone()),
        pid: Some(coords.pid),
        source,
        ..OperatorNodeStatus::default()
    };

    let shell_token = match attach_shell_token(&client, &coords).await {
        Ok(token) => token,
        Err(err) => {
            status.note = Some(format!("attach failed: {}", err));
            return Ok(status);
        }
    };

    if let Ok(capsules) = list_runtime_capsules(&client, &coords.api_url, &shell_token).await {
        status.running_capsules = capsules;
    }
    if let Ok(peer_count) = fetch_peer_count(&client, &coords.api_url, &shell_token).await {
        status.peer_count = Some(peer_count);
    }
    if let Ok((ticket, node_id)) =
        fetch_ticket_and_node_id(&client, &coords.api_url, &shell_token).await
    {
        status.connect_ticket = Some(ticket);
        status.node_id = Some(node_id);
    }

    Ok(status)
}

pub async fn gather_local_update_check(data_dir: &Path) -> Result<OperatorUpdateCheck> {
    let source = load_default_trusted_source(data_dir)?;
    let primary_publisher =
        source.publisher_dids.first().cloned().ok_or_else(|| {
            anyhow::anyhow!("Trusted source '{}' has no publisher DID", source.name)
        })?;

    let current_version = source.installed_version.clone();
    let client = crate::carrier::CarrierClient::connect_trusted_source(
        &source,
        OPERATOR_CONNECT_TIMEOUT_SECS,
    )
    .await
    .with_context(|| {
        format!(
            "Carrier connection to trusted source '{}' failed. Remote operator update check stays Carrier-only.",
            source.name
        )
    })?;
    let release = client.release_head().await.with_context(|| {
        format!(
            "Carrier discovery from trusted source '{}' failed to return a release head.",
            source.name
        )
    })?;
    let release = release.ok_or_else(|| {
        anyhow::anyhow!(
            "Carrier discovery from trusted source '{}' returned no release head.",
            source.name
        )
    })?;
    let head_cid = release
        .get("head_cid")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());
    let head_bytes = client
        .fetch_file("release-head.json")
        .await
        .with_context(|| {
            format!(
                "Carrier fetch of release-head.json from trusted source '{}' failed.",
                source.name
            )
        })?;

    let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &primary_publisher)?;
    let latest_version = head["payload"]["version"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let release_cid = head["payload"]["latest_release_cid"]
        .as_str()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());

    let channel = normalized_channel(&source);
    Ok(OperatorUpdateCheck {
        source_name: source.name,
        channel,
        current_version,
        latest_version: latest_version.clone(),
        update_available: latest_version != source.installed_version,
        discovery: "Carrier".to_string(),
        working_gateway: None,
        head_cid,
        release_cid,
    })
}

pub async fn apply_local_update(data_dir: &Path) -> Result<OperatorUpdateApply> {
    let source = load_default_trusted_source(data_dir)?;
    let source_name = source.name.clone();
    let channel = normalized_channel(&source);
    let previous_version = source.installed_version.clone();

    let carrier_client = Arc::new(
        crate::carrier::CarrierClient::connect_trusted_source(
            &source,
            OPERATOR_CONNECT_TIMEOUT_SECS,
        )
        .await
        .with_context(|| {
            format!(
                "Carrier connection to trusted source '{}' failed. Remote operator apply stays Carrier-only; use a local explicit transport override if you intentionally need web transport.",
                source.name
            )
        })?,
    );

    let platform = crate::update::detect_release_platform().to_string();
    let fetch_counter = Arc::new(AtomicUsize::new(0));
    let carrier_for_fetch = carrier_client.clone();
    let fetch_fn: crate::update::FetchFn = Box::new(move |_cid, _gateways| {
        let client = carrier_for_fetch.clone();
        let counter = fetch_counter.clone();
        let platform = platform.clone();
        Box::pin(async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let path = match n {
                0 => "release-head.json".to_string(),
                1 => "release.json".to_string(),
                2 => format!("elastos-{}", platform),
                3 => format!("components-{}.json", platform),
                _ => return Err(anyhow::anyhow!("unexpected operator update fetch #{}", n)),
            };
            client.fetch_file(&path).await
        })
    });

    let try_p2p: crate::update::TryP2pFn = Box::new(|source, publisher_did| {
        Box::pin(async move { try_operator_p2p_discovery(&source, &publisher_did).await })
    });

    crate::update::run_update_for_data_dir(
        data_dir,
        &fetch_fn,
        Some(&try_p2p),
        false,
        None,
        false,
        Vec::new(),
        env!("ELASTOS_VERSION"),
        true,
        false,
    )
    .await?;

    let installed_source = load_default_trusted_source(data_dir)?;
    let installed_version = installed_source.installed_version.clone();
    let runtime_status = gather_local_node_status(data_dir).await.ok();
    let runtime_running = runtime_status
        .as_ref()
        .map(|status| status.runtime_running)
        .unwrap_or(false);
    let runtime_version = runtime_status.and_then(|status| status.runtime_version);
    let restart_required =
        runtime_running && runtime_version.as_deref() != Some(installed_version.as_str());
    let changed = installed_version != previous_version;
    let note = if restart_required {
        Some("Update installed. Restart the remote runtime to activate the new binary.".to_string())
    } else if !changed {
        Some("Installed release was already current.".to_string())
    } else {
        None
    };

    Ok(OperatorUpdateApply {
        source_name,
        channel,
        previous_version,
        installed_version,
        changed,
        runtime_running,
        runtime_version,
        restart_required,
        note,
    })
}

pub async fn request_remote_status(
    data_dir: &Path,
    peer: &OperatorPeer,
) -> Result<OperatorNodeStatus> {
    request_remote_typed(data_dir, peer, OPERATOR_ACTION_STATUS_READ).await
}

pub async fn request_remote_update_check(
    data_dir: &Path,
    peer: &OperatorPeer,
) -> Result<OperatorUpdateCheck> {
    request_remote_typed(data_dir, peer, OPERATOR_ACTION_UPDATE_CHECK).await
}

pub async fn request_remote_update_apply(
    data_dir: &Path,
    peer: &OperatorPeer,
) -> Result<OperatorUpdateApply> {
    request_remote_typed(data_dir, peer, OPERATOR_ACTION_UPDATE_APPLY).await
}

pub async fn request_remote_room_read(data_dir: &Path, peer: &OperatorPeer) -> Result<RoomSummary> {
    request_remote_typed(data_dir, peer, OPERATOR_ACTION_ROOM_READ).await
}

pub async fn request_remote_room_approve(
    data_dir: &Path,
    peer: &OperatorPeer,
    request_id: Option<&str>,
) -> Result<Option<ApprovalOutcome>> {
    request_remote_typed_with_args(
        data_dir,
        peer,
        OPERATOR_ACTION_ROOM_APPROVE,
        serde_json::json!({ "request_id": request_id }),
    )
    .await
}

pub async fn request_remote_room_deny(
    data_dir: &Path,
    peer: &OperatorPeer,
    request_id: Option<&str>,
    reason: &str,
) -> Result<Option<DenyOutcome>> {
    request_remote_typed_with_args(
        data_dir,
        peer,
        OPERATOR_ACTION_ROOM_DENY,
        serde_json::json!({
            "request_id": request_id,
            "reason": reason,
        }),
    )
    .await
}

pub async fn request_remote_room_open(
    data_dir: &Path,
    peer: &OperatorPeer,
    addr: &str,
) -> Result<OperatorRoomOpen> {
    request_remote_typed_with_args(
        data_dir,
        peer,
        OPERATOR_ACTION_ROOM_OPEN,
        serde_json::json!({ "addr": addr }),
    )
    .await
}

fn validate_action(action: &str) -> Result<()> {
    if supported_actions().contains(&action) {
        Ok(())
    } else {
        anyhow::bail!(
            "unsupported operator action '{}'. Supported actions: {}",
            action,
            supported_actions().join(", ")
        );
    }
}

fn normalize_actions(actions: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for action in actions {
        let trimmed = action.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_action(trimmed)?;
        if !normalized.iter().any(|entry| entry == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    normalized.sort();
    Ok(normalized)
}

fn validate_peer_did(did: &str) -> Result<()> {
    crate::crypto::decode_did_key(did)
        .map(|_| ())
        .with_context(|| format!("invalid peer DID '{}'", did))
}

fn action_allowed(peer: &OperatorPeer, action: &str) -> bool {
    peer.allow.iter().any(|entry| entry == action)
}

fn is_mutating_action(action: &str) -> bool {
    matches!(
        action,
        OPERATOR_ACTION_UPDATE_APPLY
            | OPERATOR_ACTION_ROOM_APPROVE
            | OPERATOR_ACTION_ROOM_DENY
            | OPERATOR_ACTION_ROOM_OPEN
    )
}

fn build_http_client(timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(Into::into)
}

async fn read_runtime_coords(data_dir: &Path) -> Option<crate::runtime_control::RuntimeCoords> {
    crate::runtime_control::read_attachable_runtime_coords(
        data_dir,
        crate::runtime_control::AttachableRuntimeKind::Operator,
    )
    .await
}

async fn read_runtime_coords_for_kind(
    data_dir: &Path,
    runtime_kind: crate::runtime_control::AttachableRuntimeKind,
) -> Option<crate::runtime_control::RuntimeCoords> {
    crate::runtime_control::read_attachable_runtime_coords(data_dir, runtime_kind).await
}

async fn attach_shell_token(
    client: &reqwest::Client,
    coords: &crate::runtime_control::RuntimeCoords,
) -> Result<String> {
    let api_base = LoopbackHttpBaseUrl::parse(&coords.api_url)?;
    let response = client
        .post(api_base.join("/api/auth/attach")?)
        .json(&serde_json::json!({
            "secret": coords.attach_secret,
            "scope": "shell",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<AttachResponse>()
        .await?;
    Ok(response.token)
}

async fn fetch_runtime_version(client: &reqwest::Client, api_url: &str) -> Option<String> {
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
        .map(|value| value.to_string())
}

async fn list_runtime_capsules(
    client: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
) -> Result<Vec<String>> {
    let response = client
        .get(format!("{}/api/capsules", api_url))
        .header("Authorization", format!("Bearer {}", shell_token))
        .send()
        .await?
        .error_for_status()?
        .json::<CapsulesResponse>()
        .await?;
    let mut names: Vec<String> = response
        .capsules
        .into_iter()
        .map(|capsule| capsule.name)
        .collect();
    names.sort();
    Ok(names)
}

async fn call_peer_provider(
    client: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
    op: &str,
) -> Result<serde_json::Value> {
    let response = client
        .post(format!("{}/api/provider/peer/{}", api_url, op))
        .header("Authorization", format!("Bearer {}", shell_token))
        .json(&serde_json::json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<PeerProviderEnvelope>()
        .await?;
    Ok(response.data)
}

async fn fetch_peer_count(
    client: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
) -> Result<usize> {
    let data = call_peer_provider(client, api_url, shell_token, "list_peers").await?;
    Ok(data
        .get("peers")
        .and_then(|value| value.as_array())
        .map(|peers| peers.len())
        .unwrap_or(0))
}

async fn fetch_ticket_and_node_id(
    client: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
) -> Result<(String, String)> {
    let data = call_peer_provider(client, api_url, shell_token, "get_ticket").await?;
    let ticket = data
        .get("ticket")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("ticket response missing ticket"))?;
    let node_id = data
        .get("node_id")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("ticket response missing node_id"))?;
    Ok((ticket, node_id))
}

pub(crate) async fn fetch_local_carrier_bootstrap(
    data_dir: &Path,
    runtime_kind: crate::runtime_control::AttachableRuntimeKind,
) -> Result<(String, String)> {
    let coords = read_runtime_coords_for_kind(data_dir, runtime_kind)
        .await
        .ok_or_else(|| {
            let message = match runtime_kind {
                crate::runtime_control::AttachableRuntimeKind::Operator => {
                    crate::runtime_control::OPERATOR_RUNTIME_REQUIRED_MESSAGE
                }
                crate::runtime_control::AttachableRuntimeKind::Gateway => {
                    "This command requires a running gateway in the explicit data root."
                }
            };
            anyhow::anyhow!(message)
        })?;
    let client = build_http_client(5)
        .map_err(|_| anyhow::anyhow!("failed to create local Runtime operator client"))?;
    let shell_token = attach_shell_token(&client, &coords)
        .await
        .map_err(|_| anyhow::anyhow!("failed to attach to the local Runtime"))?;
    fetch_ticket_and_node_id(&client, &coords.api_url, &shell_token)
        .await
        .map_err(|_| anyhow::anyhow!("local Carrier Provider get_ticket failed"))
}

fn load_default_trusted_source(data_dir: &Path) -> Result<TrustedSource> {
    let sources = load_trusted_sources(data_dir)?;
    sources.default_source().cloned().ok_or_else(|| {
        anyhow::anyhow!("No trusted source configured. Run `elastos source add ...` first.")
    })
}

fn load_default_source_status(data_dir: &Path) -> Option<OperatorSourceStatus> {
    let sources = load_trusted_sources(data_dir).ok()?;
    let source = sources.default_source()?;
    Some(OperatorSourceStatus {
        name: source.name.clone(),
        installed_version: source.installed_version.clone(),
        channel: normalized_channel(source),
        gateway: source.gateways.first().cloned(),
    })
}

fn normalized_channel(source: &TrustedSource) -> String {
    if source.channel.trim().is_empty() {
        "stable".to_string()
    } else {
        source.channel.trim().to_string()
    }
}

async fn try_operator_p2p_discovery(
    source: &TrustedSource,
    _publisher_did: &str,
) -> Option<String> {
    let client = crate::carrier::CarrierClient::connect_trusted_source(
        source,
        OPERATOR_CONNECT_TIMEOUT_SECS + 5,
    )
    .await
    .ok()?;
    let release = client.release_head().await.ok()??;
    release["head_cid"].as_str().map(|value| value.to_string())
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn within_ts_skew(ts: u64) -> bool {
    let now = now_ts();
    ts <= now.saturating_add(OPERATOR_TS_SKEW_SECS)
        && ts.saturating_add(OPERATOR_TS_SKEW_SECS) >= now
}

fn random_request_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS entropy unavailable for request id");
    hex::encode(bytes)
}

fn sign_envelope<T: Serialize + Clone>(
    signing_key: &ed25519_dalek::SigningKey,
    domain: &str,
    payload: &T,
) -> Result<SignedEnvelope<T>> {
    let canonical = serde_json::to_string(&serde_json::to_value(payload)?)?;
    let (signature, signer_did) = domain_separated_sign(signing_key, domain, canonical.as_bytes());
    Ok(SignedEnvelope {
        payload: payload.clone(),
        signature,
        signer_did,
    })
}

fn verify_request_envelope(
    envelope_bytes: &[u8],
    expected_requesters: &[String],
) -> Result<OperatorRequestPayload> {
    let (value, signer_did) = verify_release_envelope_against_dids(
        envelope_bytes,
        OPERATOR_REQUEST_DOMAIN,
        expected_requesters,
    )?;
    let envelope: SignedEnvelope<OperatorRequestPayload> = serde_json::from_value(value)?;
    if envelope.signer_did != signer_did || envelope.payload.requester_did != signer_did {
        anyhow::bail!("request signer DID does not match requester DID");
    }
    if envelope.payload.version != 1 {
        anyhow::bail!(
            "unsupported operator request version {}",
            envelope.payload.version
        );
    }
    Ok(envelope.payload)
}

fn verify_response_envelope(
    envelope_bytes: &[u8],
    expected_target_did: &str,
) -> Result<OperatorResponsePayload> {
    let (value, signer_did) = verify_release_envelope_against_dids(
        envelope_bytes,
        OPERATOR_RESPONSE_DOMAIN,
        &[expected_target_did.to_string()],
    )?;
    let envelope: SignedEnvelope<OperatorResponsePayload> = serde_json::from_value(value)?;
    if envelope.signer_did != signer_did || envelope.payload.target_did != signer_did {
        anyhow::bail!("response signer DID does not match target DID");
    }
    if envelope.payload.version != 1 {
        anyhow::bail!(
            "unsupported operator response version {}",
            envelope.payload.version
        );
    }
    if !within_ts_skew(envelope.payload.ts) {
        anyhow::bail!("operator response timestamp is outside the allowed skew window");
    }
    Ok(envelope.payload)
}

async fn request_remote_typed<T: DeserializeOwned>(
    data_dir: &Path,
    peer: &OperatorPeer,
    action: &str,
) -> Result<T> {
    request_remote_typed_with_args(data_dir, peer, action, serde_json::Value::Null).await
}

async fn request_remote_typed_with_args<T: DeserializeOwned>(
    data_dir: &Path,
    peer: &OperatorPeer,
    action: &str,
    args: serde_json::Value,
) -> Result<T> {
    validate_action(action)?;
    let client = OperatorClient::connect(peer, OPERATOR_CONNECT_TIMEOUT_SECS).await?;
    let (signing_key, requester_did) = elastos_identity::load_or_create_did(data_dir)?;
    let request_payload = OperatorRequestPayload {
        version: 1,
        request_id: random_request_id(),
        requester_did,
        target_did: peer.did.clone(),
        ts: now_ts(),
        action: action.to_string(),
        args,
    };
    let request = sign_envelope(&signing_key, OPERATOR_REQUEST_DOMAIN, &request_payload)?;
    let response_bytes = client.send_request(&request).await?;
    let response = verify_response_envelope(&response_bytes, &peer.did)?;

    if response.request_id != request_payload.request_id {
        anyhow::bail!(
            "operator response request_id mismatch: expected {}, got {}",
            request_payload.request_id,
            response.request_id
        );
    }
    if !response.accepted {
        anyhow::bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "operator request was denied".to_string())
        );
    }

    serde_json::from_value(response.result)
        .map_err(|err| anyhow::anyhow!("invalid operator response payload: {}", err))
}

async fn handle_operator_connection(
    conn: iroh::endpoint::Connection,
    ctx: OperatorRuntimeContext,
) -> Result<()> {
    loop {
        let (mut send, recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(_) => break,
        };
        let stream_ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_operator_stream(&mut send, recv, &stream_ctx).await {
                tracing::debug!("operator control stream error: {:#}", err);
            }
        });
    }
    Ok(())
}

async fn handle_operator_stream(
    send: &mut iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    ctx: &OperatorRuntimeContext,
) -> Result<()> {
    let mut reader = BufReader::new(recv);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let request_meta = extract_request_meta(line.as_bytes());
    let request_id = request_meta
        .as_ref()
        .map(|meta| meta.request_id.clone())
        .unwrap_or_default();
    let requester_did = request_meta
        .as_ref()
        .map(|meta| meta.requester_did.clone())
        .unwrap_or_default();
    let action = request_meta
        .as_ref()
        .map(|meta| meta.action.clone())
        .unwrap_or_default();

    let _request_guard = ctx.request_serial.lock().await;

    let config =
        load_operator_control(&ctx.data_dir).unwrap_or_else(|_| OperatorControlConfig::empty());
    let expected_requesters: Vec<String> =
        config.peers.iter().map(|peer| peer.did.clone()).collect();

    let response = match verify_request_envelope(line.trim().as_bytes(), &expected_requesters) {
        Ok(request) => build_operator_response(ctx, &config, request).await,
        Err(err) => OperatorResponsePayload {
            version: 1,
            request_id,
            accepted: false,
            target_did: ctx.local_did.clone(),
            ts: now_ts(),
            result: serde_json::Value::Null,
            error: Some(err.to_string()),
        },
    };

    append_audit_entry(
        &ctx.data_dir,
        AuditEntry {
            ts: response.ts,
            request_id: response.request_id.clone(),
            requester_did,
            action,
            accepted: response.accepted,
            error: response.error.clone(),
        },
    );

    let (signing_key, _did) = elastos_identity::load_or_create_did(&ctx.data_dir)?;
    let envelope = sign_envelope(&signing_key, OPERATOR_RESPONSE_DOMAIN, &response)?;
    send_json_line(send, &envelope).await
}

async fn build_operator_response(
    ctx: &OperatorRuntimeContext,
    config: &OperatorControlConfig,
    request: OperatorRequestPayload,
) -> OperatorResponsePayload {
    let peer = match config
        .peers
        .iter()
        .find(|entry| entry.did == request.requester_did)
    {
        Some(peer) => peer,
        None => {
            return denied_response(
                &ctx.local_did,
                request.request_id,
                "requester DID is not configured for operator control",
            )
        }
    };

    if request.target_did != ctx.local_did {
        return denied_response(
            &ctx.local_did,
            request.request_id,
            "operator request target DID does not match this runtime",
        );
    }
    if !within_ts_skew(request.ts) {
        return denied_response(
            &ctx.local_did,
            request.request_id,
            "operator request timestamp is outside the allowed skew window",
        );
    }
    if !action_allowed(peer, &request.action) {
        return denied_response(
            &ctx.local_did,
            request.request_id,
            &format!(
                "requester DID {} is not allowed to perform {}",
                request.requester_did, request.action
            ),
        );
    }
    if is_mutating_action(&request.action) {
        if request_log_contains_request_id(
            &ctx.data_dir,
            &request.requester_did,
            &request.request_id,
        ) {
            return denied_response(
                &ctx.local_did,
                request.request_id,
                "operator request replay detected",
            );
        }
        if let Err(err) = reserve_request_id(&ctx.data_dir, &request) {
            return error_response(&ctx.local_did, request.request_id, err);
        }
    } else if audit_contains_request_id(&ctx.data_dir, &request.requester_did, &request.request_id)
    {
        return denied_response(
            &ctx.local_did,
            request.request_id,
            "operator request replay detected",
        );
    }

    let request_id = request.request_id.clone();
    let response = match request.action.as_str() {
        OPERATOR_ACTION_STATUS_READ => match gather_local_node_status(&ctx.data_dir).await {
            Ok(mut status) => {
                status
                    .node_id
                    .get_or_insert_with(|| ctx.endpoint.id().to_string());
                if status.peer_count.is_none() {
                    status.peer_count = Some(ctx.peers.lock().await.len());
                }
                if status.connect_ticket.is_none() {
                    status.connect_ticket = local_endpoint_ticket(&ctx.endpoint);
                }
                success_response(
                    &ctx.local_did,
                    request_id.clone(),
                    serde_json::to_value(status),
                )
            }
            Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
        },
        OPERATOR_ACTION_UPDATE_CHECK => match gather_local_update_check(&ctx.data_dir).await {
            Ok(update) => success_response(
                &ctx.local_did,
                request_id.clone(),
                serde_json::to_value(update),
            ),
            Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
        },
        OPERATOR_ACTION_UPDATE_APPLY => match apply_local_update(&ctx.data_dir).await {
            Ok(update) => success_response(
                &ctx.local_did,
                request_id.clone(),
                serde_json::to_value(update),
            ),
            Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
        },
        OPERATOR_ACTION_ROOM_READ => match gather_local_room_summary(&ctx.data_dir, &ctx.local_did)
        {
            Ok(summary) => success_response(
                &ctx.local_did,
                request_id.clone(),
                serde_json::to_value(summary),
            ),
            Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
        },
        OPERATOR_ACTION_ROOM_APPROVE => {
            match parse_operator_args::<OperatorRoomApproveArgs>(&request.args) {
                Ok(args) => {
                    match approve_local_room_request(&ctx.data_dir, args.request_id.as_deref())
                        .await
                    {
                        Ok(outcome) => success_response(
                            &ctx.local_did,
                            request_id.clone(),
                            serde_json::to_value(outcome),
                        ),
                        Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
                    }
                }
                Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
            }
        }
        OPERATOR_ACTION_ROOM_DENY => {
            match parse_operator_args::<OperatorRoomDenyArgs>(&request.args) {
                Ok(args) => match deny_local_room_request(
                    &ctx.data_dir,
                    args.request_id.as_deref(),
                    args.reason.as_deref(),
                )
                .await
                {
                    Ok(outcome) => success_response(
                        &ctx.local_did,
                        request_id.clone(),
                        serde_json::to_value(outcome),
                    ),
                    Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
                },
                Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
            }
        }
        OPERATOR_ACTION_ROOM_OPEN => {
            match parse_operator_args::<OperatorRoomOpenArgs>(&request.args) {
                Ok(args) => match start_local_room_gateway(&ctx.data_dir, &args.addr).await {
                    Ok(opened) => success_response(
                        &ctx.local_did,
                        request_id.clone(),
                        serde_json::to_value(opened),
                    ),
                    Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
                },
                Err(err) => error_response(&ctx.local_did, request_id.clone(), err),
            }
        }
        _ => denied_response(&ctx.local_did, request_id, "unsupported operator action"),
    };

    if is_mutating_action(&request.action) {
        finalize_reserved_request(&ctx.data_dir, &request, &response);
    }
    response
}

fn parse_operator_args<T>(args: &serde_json::Value) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    if args.is_null() {
        Ok(T::default())
    } else {
        serde_json::from_value(args.clone())
            .map_err(|err| anyhow::anyhow!("invalid operator action args: {}", err))
    }
}

fn room_capsule_installed(data_dir: &Path) -> bool {
    data_dir
        .join("capsules")
        .join(room_slug())
        .join("capsule.json")
        .exists()
}

fn gather_local_room_summary(data_dir: &Path, local_did: &str) -> Result<RoomSummary> {
    let mut summary = load_summary(data_dir)?;
    let access = local_runtime_access(data_dir, Some(local_did))?;
    let hosted = load_browser_app_hosted_endpoint(data_dir, room_slug())?;
    summary.local_runtime_did = Some(local_did.to_string());
    summary.local_runtime_role = access.member_role;
    summary.browser_access_allowed = access.browser_access_allowed;
    summary.browser_access_block_reason = access.block_reason;
    summary.canonical_hosted_guest_url = hosted.canonical_url;
    summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    Ok(summary)
}

async fn approve_local_room_request(
    data_dir: &Path,
    request_id: Option<&str>,
) -> Result<Option<ApprovalOutcome>> {
    let outcome = match request_id {
        Some(request_id) => approve_request(data_dir, request_id)?,
        None => approve_next_request(data_dir)?,
    };
    let summary =
        gather_local_room_summary(data_dir, &elastos_identity::load_or_create_did(data_dir)?.1)?;
    sync_room_notifications(data_dir, &summary)?;
    Ok(outcome)
}

async fn deny_local_room_request(
    data_dir: &Path,
    request_id: Option<&str>,
    reason: Option<&str>,
) -> Result<Option<DenyOutcome>> {
    let reason = reason.unwrap_or("Denied from remote operator CLI");
    let outcome = match request_id {
        Some(request_id) => deny_request(data_dir, request_id, reason)?,
        None => deny_next_request(data_dir, reason)?,
    };
    let summary =
        gather_local_room_summary(data_dir, &elastos_identity::load_or_create_did(data_dir)?.1)?;
    sync_room_notifications(data_dir, &summary)?;
    Ok(outcome)
}

pub async fn start_local_room_gateway(data_dir: &Path, addr: &str) -> Result<OperatorRoomOpen> {
    if !room_capsule_installed(data_dir) {
        anyhow::bail!(
            "Room browser capsule is not installed. Run `elastos setup --profile demo` first."
        );
    }

    let coords = read_runtime_coords(data_dir).await.ok_or_else(|| {
        anyhow::anyhow!("no active local runtime. Start `elastos serve` on the target and retry.")
    })?;
    let client = build_http_client(10)?;
    let shell_token = attach_shell_token(&client, &coords).await?;
    let api_base = LoopbackHttpBaseUrl::parse(&coords.api_url)?;
    let response = client
        .post(api_base.join("/api/supervisor/start-gateway")?)
        .header("Authorization", format!("Bearer {}", shell_token))
        .json(&serde_json::json!({
            "addr": addr,
            "cache_dir": serde_json::Value::Null,
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        let message = body.trim();
        if message.is_empty() {
            anyhow::bail!("failed to start room gateway at {}", addr);
        }
        anyhow::bail!("{}", message);
    }
    let response: SupervisorStartGatewayOutput = serde_json::from_str(&body).map_err(|err| {
        anyhow::anyhow!(
            "invalid start-gateway response from {}: {} ({})",
            addr,
            err,
            body
        )
    })?;
    if response.status != "ok" {
        anyhow::bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "failed to start room gateway".to_string())
        );
    }
    let bind_addr = response.path.unwrap_or_else(|| addr.to_string());
    let open_urls = crate::api::gateway::advertised_gateway_urls(&bind_addr);
    let room_urls = open_urls
        .iter()
        .map(|url| format!("{}apps/{}/", url, room_slug()))
        .collect();
    Ok(OperatorRoomOpen {
        bind_addr,
        open_urls,
        room_urls,
    })
}

fn success_response(
    local_did: &str,
    request_id: String,
    result: serde_json::Result<serde_json::Value>,
) -> OperatorResponsePayload {
    match result {
        Ok(value) => OperatorResponsePayload {
            version: 1,
            request_id,
            accepted: true,
            target_did: local_did.to_string(),
            ts: now_ts(),
            result: value,
            error: None,
        },
        Err(err) => error_response(local_did, request_id, err),
    }
}

fn denied_response(local_did: &str, request_id: String, message: &str) -> OperatorResponsePayload {
    OperatorResponsePayload {
        version: 1,
        request_id,
        accepted: false,
        target_did: local_did.to_string(),
        ts: now_ts(),
        result: serde_json::Value::Null,
        error: Some(message.to_string()),
    }
}

fn error_response(
    local_did: &str,
    request_id: String,
    err: impl std::fmt::Display,
) -> OperatorResponsePayload {
    OperatorResponsePayload {
        version: 1,
        request_id,
        accepted: false,
        target_did: local_did.to_string(),
        ts: now_ts(),
        result: serde_json::Value::Null,
        error: Some(err.to_string()),
    }
}

fn append_audit_entry(data_dir: &Path, entry: AuditEntry) {
    let path = data_dir.join(OPERATOR_AUDIT_FILE);
    let _ = append_jsonl_entry(&path, &entry);
}

fn append_request_log_entry(data_dir: &Path, entry: &RequestLogEntry) -> Result<()> {
    let path = data_dir.join(OPERATOR_REQUEST_LOG_FILE);
    append_jsonl_entry(&path, entry)
}

fn append_jsonl_entry<T: Serialize>(path: &Path, entry: &T) -> Result<()> {
    let Some(parent) = path.parent() else {
        anyhow::bail!("{} has no parent directory", path.display());
    };
    std::fs::create_dir_all(parent)?;

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

fn audit_contains_request_id(data_dir: &Path, requester_did: &str, request_id: &str) -> bool {
    if requester_did.is_empty() || request_id.is_empty() {
        return false;
    }

    let path = data_dir.join(OPERATOR_AUDIT_FILE);
    let Ok(data) = std::fs::read_to_string(path) else {
        return false;
    };

    data.lines().rev().any(|line| {
        serde_json::from_str::<AuditEntry>(line)
            .ok()
            .is_some_and(|entry| {
                entry.requester_did == requester_did && entry.request_id == request_id
            })
    })
}

fn request_log_contains_request_id(data_dir: &Path, requester_did: &str, request_id: &str) -> bool {
    if requester_did.is_empty() || request_id.is_empty() {
        return false;
    }

    let path = data_dir.join(OPERATOR_REQUEST_LOG_FILE);
    let Ok(data) = std::fs::read_to_string(path) else {
        return false;
    };

    data.lines().rev().any(|line| {
        serde_json::from_str::<RequestLogEntry>(line)
            .ok()
            .is_some_and(|entry| {
                entry.requester_did == requester_did && entry.request_id == request_id
            })
    })
}

fn reserve_request_id(data_dir: &Path, request: &OperatorRequestPayload) -> Result<()> {
    append_request_log_entry(
        data_dir,
        &RequestLogEntry {
            ts: now_ts(),
            request_id: request.request_id.clone(),
            requester_did: request.requester_did.clone(),
            action: request.action.clone(),
            state: REQUEST_STATE_RESERVED.to_string(),
            accepted: None,
            error: None,
        },
    )
}

fn finalize_reserved_request(
    data_dir: &Path,
    request: &OperatorRequestPayload,
    response: &OperatorResponsePayload,
) {
    let _ = append_request_log_entry(
        data_dir,
        &RequestLogEntry {
            ts: now_ts(),
            request_id: request.request_id.clone(),
            requester_did: request.requester_did.clone(),
            action: request.action.clone(),
            state: REQUEST_STATE_COMPLETED.to_string(),
            accepted: Some(response.accepted),
            error: response.error.clone(),
        },
    );
}

fn local_endpoint_ticket(endpoint: &Endpoint) -> Option<String> {
    let mut watcher = endpoint.watch_addr();
    let addr = watcher.get();
    let ticket_json = serde_json::json!({
        "topic": null,
        "endpoints": [addr],
    });
    let ticket_bytes = serde_json::to_vec(&ticket_json).ok()?;
    let mut ticket = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
    ticket.make_ascii_lowercase();
    Some(ticket)
}

async fn send_json_line<T: Serialize>(
    send: &mut iroh::endpoint::SendStream,
    value: &T,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    send.write_all(&bytes).await?;
    send.finish()?;
    Ok(())
}

fn extract_request_meta(envelope_bytes: &[u8]) -> Option<OperatorRequestPayload> {
    serde_json::from_slice::<serde_json::Value>(envelope_bytes)
        .ok()?
        .get("payload")
        .cloned()
        .and_then(|payload| serde_json::from_value(payload).ok())
}

struct OperatorClient {
    conn: iroh::endpoint::Connection,
    _endpoint: Endpoint,
}

impl OperatorClient {
    async fn connect_endpoint_addr(addr: iroh::EndpointAddr, timeout_secs: u64) -> Result<Self> {
        let mut rng_bytes = [0u8; 32];
        getrandom::getrandom(&mut rng_bytes).map_err(|err| anyhow::anyhow!("rng: {}", err))?;
        let secret_key = SecretKey::from_bytes(&rng_bytes);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .bind()
            .await
            .context("failed to bind operator client endpoint")?;

        let conn = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            endpoint.connect(addr, OPERATOR_ALPN),
        )
        .await
        .map_err(|_| anyhow::anyhow!("operator connection timed out"))?
        .context("operator connect failed")?;

        Ok(Self {
            conn,
            _endpoint: endpoint,
        })
    }

    async fn connect(peer: &OperatorPeer, timeout_secs: u64) -> Result<Self> {
        validate_peer_did(&peer.did)?;

        if !peer.connect_ticket.trim().is_empty() {
            let ticket_endpoints = decode_ticket_endpoints(&peer.connect_ticket);
            if ticket_endpoints.is_empty() {
                anyhow::bail!("operator peer '{}' has an invalid connect ticket", peer.did);
            }

            let mut ticket_errors = Vec::new();
            for endpoint in ticket_endpoints {
                match Self::connect_endpoint_addr(endpoint, timeout_secs).await {
                    Ok(client) => return Ok(client),
                    Err(err) => ticket_errors.push(err.to_string()),
                }
            }
            anyhow::bail!(
                "operator connection failed using the explicit ticket route: {}",
                ticket_errors.join(" | ")
            );
        }

        let public_key = public_key_from_did(&peer.did)?;
        let endpoint = iroh::EndpointAddr::from(public_key);
        Self::connect_endpoint_addr(endpoint, timeout_secs).await
    }

    async fn send_request<T: Serialize>(&self, value: &T) -> Result<Vec<u8>> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;

        let mut reader = BufReader::new(recv);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line.trim().is_empty() {
            anyhow::bail!("operator peer returned an empty response");
        }
        Ok(line.into_bytes())
    }
}

fn public_key_from_did(did: &str) -> Result<iroh::PublicKey> {
    let verifying_key = crate::crypto::decode_did_key(did)?;
    iroh::PublicKey::from_bytes(verifying_key.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid operator DID '{}': {}", did, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carrier::start_carrier_node;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use elastos_runtime::signature::generate_keypair;
    use tokio::net::TcpListener;

    fn write_test_device_key(data_dir: &Path, key: &[u8; 32]) {
        let identity_dir = data_dir.join("identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(identity_dir.join("device.key"), key).unwrap();
    }

    async fn start_ticket_provider(
        status: StatusCode,
        body: serde_json::Value,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/api/provider/peer/get_ticket",
            post(move |headers: HeaderMap| {
                let body = body.clone();
                async move {
                    if headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        != Some("Bearer shell-test")
                    {
                        return StatusCode::FORBIDDEN.into_response();
                    }
                    (status, Json(body)).into_response()
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (api_url, task)
    }

    #[tokio::test]
    async fn ticket_provider_helper_requests_only_get_ticket_and_redacts_failure() {
        let (api_url, task) = start_ticket_provider(
            StatusCode::OK,
            serde_json::json!({
                "data": {"ticket": "ticket-secret", "node_id": "node-public"}
            }),
        )
        .await;
        let client = build_http_client(2).unwrap();
        assert_eq!(
            fetch_ticket_and_node_id(&client, &api_url, "shell-test")
                .await
                .unwrap(),
            ("ticket-secret".to_string(), "node-public".to_string())
        );
        task.abort();

        let (api_url, task) = start_ticket_provider(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": "ticket-secret"}),
        )
        .await;
        let error = fetch_ticket_and_node_id(&client, &api_url, "shell-test")
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("ticket-secret"));
        task.abort();
    }

    #[test]
    fn normalize_actions_rejects_unknown_entries() {
        let err = normalize_actions(&["remote.shell".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unsupported operator action"));
    }

    #[test]
    fn supported_actions_include_update_apply() {
        assert!(supported_actions().contains(&OPERATOR_ACTION_UPDATE_APPLY));
    }

    #[test]
    fn upsert_peer_merges_existing_route_and_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, _) = generate_keypair();
        let did = elastos_identity::encode_signing_key_did(&sk);

        upsert_peer(
            dir.path(),
            OperatorPeer {
                did: did.clone(),
                label: "jetson".to_string(),
                connect_ticket: "ticket-a".to_string(),
                allow: vec![OPERATOR_ACTION_STATUS_READ.to_string()],
            },
        )
        .unwrap();

        let updated = upsert_peer(
            dir.path(),
            OperatorPeer {
                did: did.clone(),
                label: String::new(),
                connect_ticket: String::new(),
                allow: vec![OPERATOR_ACTION_UPDATE_CHECK.to_string()],
            },
        )
        .unwrap();

        assert_eq!(updated.connect_ticket, "ticket-a");
        assert_eq!(
            updated.allow,
            vec![OPERATOR_ACTION_UPDATE_CHECK.to_string()]
        );
    }

    #[test]
    fn remove_peer_deletes_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, _) = generate_keypair();
        let did = elastos_identity::encode_signing_key_did(&sk);

        upsert_peer(
            dir.path(),
            OperatorPeer {
                did: did.clone(),
                label: "jetson".to_string(),
                connect_ticket: "ticket-a".to_string(),
                allow: vec![OPERATOR_ACTION_STATUS_READ.to_string()],
            },
        )
        .unwrap();

        let removed = remove_peer(dir.path(), &did).unwrap().unwrap();
        assert_eq!(removed.did, did);
        assert!(load_operator_control(dir.path()).unwrap().peers.is_empty());
    }

    #[test]
    fn signed_operator_response_round_trip_verifies_target_did() {
        let (sk, _) = generate_keypair();
        let target_did = elastos_identity::encode_signing_key_did(&sk);
        let payload = OperatorResponsePayload {
            version: 1,
            request_id: "req-1".to_string(),
            accepted: true,
            target_did: target_did.clone(),
            ts: now_ts(),
            result: serde_json::json!({"ok": true}),
            error: None,
        };

        let envelope = sign_envelope(&sk, OPERATOR_RESPONSE_DOMAIN, &payload).unwrap();
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let verified = verify_response_envelope(&bytes, &target_did).unwrap();
        assert_eq!(verified.request_id, "req-1");
        assert!(verified.accepted);
    }

    #[test]
    fn audit_lookup_detects_duplicate_request_ids() {
        let dir = tempfile::tempdir().unwrap();
        append_audit_entry(
            dir.path(),
            AuditEntry {
                ts: now_ts(),
                request_id: "req-1".to_string(),
                requester_did: "did:key:z6Mkexample".to_string(),
                action: OPERATOR_ACTION_UPDATE_APPLY.to_string(),
                accepted: true,
                error: None,
            },
        );

        assert!(audit_contains_request_id(
            dir.path(),
            "did:key:z6Mkexample",
            "req-1"
        ));
        assert!(!audit_contains_request_id(
            dir.path(),
            "did:key:z6Mkexample",
            "req-2"
        ));
    }

    #[test]
    fn reserved_request_log_blocks_duplicate_mutating_request_ids() {
        let dir = tempfile::tempdir().unwrap();
        let request = OperatorRequestPayload {
            version: 1,
            request_id: "req-apply-1".to_string(),
            requester_did: "did:key:z6Mkexample".to_string(),
            target_did: "did:key:z6Mktarget".to_string(),
            ts: now_ts(),
            action: OPERATOR_ACTION_UPDATE_APPLY.to_string(),
            args: serde_json::Value::Null,
        };

        reserve_request_id(dir.path(), &request).unwrap();
        assert!(request_log_contains_request_id(
            dir.path(),
            &request.requester_did,
            &request.request_id
        ));

        finalize_reserved_request(
            dir.path(),
            &request,
            &OperatorResponsePayload {
                version: 1,
                request_id: request.request_id.clone(),
                accepted: true,
                target_did: request.target_did.clone(),
                ts: now_ts(),
                result: serde_json::json!({"ok": true}),
                error: None,
            },
        );

        let log_path = dir.path().join(OPERATOR_REQUEST_LOG_FILE);
        let data = std::fs::read_to_string(log_path).unwrap();
        assert!(data.contains(REQUEST_STATE_RESERVED));
        assert!(data.contains(REQUEST_STATE_COMPLETED));
    }

    #[tokio::test]
    #[ignore = "requires network socket binding (fails in sandboxed environments)"]
    async fn test_two_node_operator_status() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let key1 = [11u8; 32];
        let key2 = [12u8; 32];
        write_test_device_key(dir1.path(), &key1);
        write_test_device_key(dir2.path(), &key2);

        let (sk1, did1) = elastos_identity::derive_did(&key1);
        let (sk2, did2) = elastos_identity::derive_did(&key2);

        let _node1 = start_carrier_node(&sk1, &did1, dir1.path().to_path_buf())
            .await
            .unwrap();
        let node2 = start_carrier_node(&sk2, &did2, dir2.path().to_path_buf())
            .await
            .unwrap();
        let ticket2 = local_endpoint_ticket(&node2.endpoint).unwrap();

        upsert_peer(
            dir1.path(),
            OperatorPeer {
                did: did2.clone(),
                label: "node2".to_string(),
                connect_ticket: ticket2,
                allow: Vec::new(),
            },
        )
        .unwrap();
        upsert_peer(
            dir2.path(),
            OperatorPeer {
                did: did1.clone(),
                label: "node1".to_string(),
                connect_ticket: String::new(),
                allow: vec![OPERATOR_ACTION_STATUS_READ.to_string()],
            },
        )
        .unwrap();

        let peer = load_operator_control(dir1.path())
            .unwrap()
            .peers
            .into_iter()
            .find(|entry| entry.did == did2)
            .unwrap();
        let status = tokio::time::timeout(
            Duration::from_secs(15),
            request_remote_status(dir1.path(), &peer),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(status.did, did2);
        assert!(!status.runtime_running);
        assert_eq!(status.note.as_deref(), Some("No active local runtime"));
    }
}
