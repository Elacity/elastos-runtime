//! Remote model service: the consumer side of a `remote_model` grant.
//!
//! The Assistant keeps using the local typed model contract. When a principal
//! holds an approved grant from a contact, that contact's shared offers appear
//! beside the local ones under a `remote:` prefixed id. A run created on such
//! an offer is executed by the granting Runtime; this Runtime only routes the
//! typed operations over Carrier and remembers which grant each run belongs
//! to. A failed Carrier call is reported as `transport_interrupted`, never as
//! a run outcome.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use elastos_runtime::provider::{
    ProviderCarrierRoute, ProviderInvocation, ProviderInvocationTransport, ProviderRegistry,
    ProviderTransfer,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::gateway_home_token::HomeLaunchTokenContext;

const REMOTE_OFFER_PREFIX: &str = "remote:";
const REMOTE_RUN_INDEX_SCHEMA: &str = "elastos.services.model-remote-runs/v1";
pub(crate) const REMOTE_RUN_INDEX_MAX: usize = 1024;
const REMOTE_RUN_RETENTION_SECS: u64 = 7 * 24 * 3600;
const CARRIER_TIMEOUT_MS: u64 = 5_000;
pub(crate) const TRANSPORT_INTERRUPTED: &str = "transport_interrupted";

pub(crate) fn remote_offer_id(grant_id: &str, offer_id: &str) -> String {
    format!("{REMOTE_OFFER_PREFIX}{grant_id}:{offer_id}")
}

/// `remote:<grant_id>:<offer_id>`; grant ids never contain `:`.
pub(crate) fn parse_remote_offer_id(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix(REMOTE_OFFER_PREFIX)?;
    let (grant_id, offer_id) = rest.split_once(':')?;
    (!grant_id.is_empty() && !offer_id.is_empty()).then_some((grant_id, offer_id))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RemoteRunRoute {
    pub principal_id: String,
    pub grant_id: String,
    pub peer_did: String,
    /// The granting Runtime's Carrier ticket at creation time. Settlement of
    /// this run (`runs_get`, `runs_events`, `runs_cancel`) keeps working after
    /// the grant expires or is denied; the granting Runtime still decides.
    #[serde(default)]
    pub connect_ticket: String,
    #[serde(default)]
    pub display_name: String,
    pub offer_id: String,
    pub request_id: String,
    /// Empty on routes written before capsule identity was stored; those
    /// routes belong to Assistant.
    #[serde(default)]
    pub capsule_id: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<u64>,
}

impl RemoteRunRoute {
    /// The grant facts needed to settle this run, independent of the grant's
    /// current status on this Home. Authority to start new runs is not implied.
    fn settlement_grant(&self) -> Option<ConsumerModelGrant> {
        (!self.connect_ticket.is_empty()).then(|| ConsumerModelGrant {
            grant_id: self.grant_id.clone(),
            peer_did: self.peer_did.clone(),
            connect_ticket: self.connect_ticket.clone(),
            display_name: self.display_name.clone(),
            expires_at: 0,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RemoteRunIndex {
    schema: String,
    #[serde(default)]
    runs: BTreeMap<String, RemoteRunRoute>,
    /// Slots held between a create's reservation and its route (key to
    /// reservation time); they count toward capacity.
    #[serde(default)]
    pending: BTreeMap<String, u64>,
}

const REMOTE_RUN_PENDING_TTL_SECS: u64 = 600;

fn route_capsule_id(capsule_id: &str) -> &str {
    if capsule_id.is_empty() {
        "assistant"
    } else {
        capsule_id
    }
}

fn route_reservation_key(
    principal_id: &str,
    grant_id: &str,
    capsule_id: &str,
    request_id: &str,
) -> String {
    format!(
        "{principal_id}:{grant_id}:{}:{request_id}",
        route_capsule_id(capsule_id)
    )
}

enum RouteSlot {
    Existing,
    Reserved,
}

fn index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("services-model-remote-runs.json")
}

fn index_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

fn read_index(data_dir: &Path) -> anyhow::Result<RemoteRunIndex> {
    match std::fs::read(index_path(data_dir)) {
        Ok(bytes) => {
            let index: RemoteRunIndex =
                serde_json::from_slice(&bytes).context("remote run index parse")?;
            anyhow::ensure!(
                index.schema == REMOTE_RUN_INDEX_SCHEMA,
                "remote run index schema mismatch"
            );
            Ok(index)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(RemoteRunIndex {
            schema: REMOTE_RUN_INDEX_SCHEMA.to_string(),
            runs: BTreeMap::new(),
            pending: BTreeMap::new(),
        }),
        Err(err) => Err(err).context("remote run index read"),
    }
}

fn update_index<T>(
    data_dir: &Path,
    now: u64,
    update: impl FnOnce(&mut RemoteRunIndex) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _guard = index_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("remote run index unavailable"))?;
    let mut index = read_index(data_dir)?;
    // Settled routes leave after the retention window; open routes leave
    // after the same window from creation, since no run lasts that long.
    index.runs.retain(|_, route| {
        let anchor = route.terminal_at.unwrap_or(route.created_at);
        now.saturating_sub(anchor) <= REMOTE_RUN_RETENTION_SECS
    });
    index
        .pending
        .retain(|_, reserved_at| now.saturating_sub(*reserved_at) <= REMOTE_RUN_PENDING_TTL_SECS);
    let result = update(&mut index)?;
    anyhow::ensure!(
        index.runs.len() + index.pending.len() <= REMOTE_RUN_INDEX_MAX,
        "remote run index is full"
    );
    let path = index_path(data_dir);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&index)?).context("remote run index write")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, &path).context("remote run index commit")?;
    Ok(result)
}

pub(crate) fn remote_run_route(
    data_dir: &Path,
    principal_id: &str,
    run_id: &str,
) -> anyhow::Result<Option<RemoteRunRoute>> {
    let _guard = index_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("remote run index unavailable"))?;
    Ok(read_index(data_dir)?
        .runs
        .get(run_id)
        .filter(|route| route.principal_id == principal_id)
        .cloned())
}

/// One approved grant, as stored on the consumer request record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsumerModelGrant {
    pub grant_id: String,
    pub peer_did: String,
    pub connect_ticket: String,
    pub display_name: String,
    pub expires_at: u64,
}

impl ConsumerModelGrant {
    /// The services layer stores the granting Runtime's Carrier peer id under
    /// `peer_did` (shared wire shape with Engine grants). The Carrier route
    /// pins the ticket to a `did:key`, so the id is converted once here.
    pub(crate) fn from_record(grant: &Value) -> Option<Self> {
        let peer_id = grant["peer_did"]
            .as_str()?
            .parse::<iroh::PublicKey>()
            .ok()?;
        Some(Self {
            grant_id: grant["grant_id"].as_str()?.to_string(),
            peer_did: crate::carrier::public_key_to_did(&peer_id).ok()?,
            connect_ticket: grant["connect_ticket"].as_str()?.to_string(),
            display_name: grant["service_display_name"]
                .as_str()
                .unwrap_or("a contact's AI model")
                .to_string(),
            expires_at: grant["expires_at"].as_u64()?,
        })
    }
}

pub(crate) fn consumer_grants(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
    discovery_service: Option<
        &crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    >,
) -> anyhow::Result<Vec<ConsumerModelGrant>> {
    Ok(
        super::gateway_home_system::home_services_remote_model_grants(
            data_dir,
            context,
            discovery_service,
        )?
        .iter()
        .filter_map(ConsumerModelGrant::from_record)
        .collect(),
    )
}

#[derive(Debug)]
pub(crate) enum RemoteRouteError {
    /// The granting Runtime answered with a bounded class such as `denied`.
    Rejected { code: String },
    /// Carrier could not complete the call; the run state is unknown here.
    Transport(String),
    /// The request or local index is invalid.
    Invalid(String),
}

impl std::fmt::Display for RemoteRouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { code } => {
                write!(f, "remote model service rejected the operation: {code}")
            }
            Self::Transport(message) => write!(f, "{TRANSPORT_INTERRUPTED}: {message}"),
            Self::Invalid(message) => write!(f, "{message}"),
        }
    }
}

/// The wire request for the granting Runtime: the typed fields the local
/// normalizer accepted, with the local binding replaced by the grant facts.
/// `request_id` lives inside the local binding after normalization, so it is
/// lifted back to the top level where the destination's typed structs expect it.
fn typed_request(
    normalized: &Value,
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    grant_id: &str,
) -> Value {
    let mut request = normalized.clone();
    if let Some(object) = request.as_object_mut() {
        if let Some(request_id) = object
            .remove("runtime_binding")
            .and_then(|binding| binding.get("request_id").cloned())
            .filter(Value::is_string)
        {
            object.insert("request_id".to_string(), request_id);
        }
        object.insert(
            "remote_model".to_string(),
            json!({
                "grant_id": grant_id,
                "principal_id": context.principal_id,
                "capsule_id": capsule_id,
            }),
        );
    }
    request
}

async fn call_grant(
    registry: &ProviderRegistry,
    grant: &ConsumerModelGrant,
    op: &str,
    request: Value,
) -> Result<Value, RemoteRouteError> {
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "model-consumer".to_string(),
            target: "model".to_string(),
            op: op.to_string(),
            request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: grant.connect_ticket.clone(),
                peer_did: Some(grant.peer_did.clone()),
                timeout_ms: Some(CARRIER_TIMEOUT_MS),
            }),
        })
        .await
        .map_err(|err| classify_carrier_error(&err.to_string()))?;
    Ok(response)
}

/// The Carrier client surfaces a destination denial as the bounded class text
/// and a transport failure as anything else. Only the first is a decision.
fn classify_carrier_error(message: &str) -> RemoteRouteError {
    for code in [
        "denied",
        "rate_limited",
        "offer_unavailable",
        "provider_failure",
        "invalid_provider_invocation",
        "unauthorized_provider_target",
    ] {
        if message.contains(code) {
            return RemoteRouteError::Rejected {
                code: code.to_string(),
            };
        }
    }
    RemoteRouteError::Transport(message.to_string())
}

use super::gateway_model_service::{
    normalize_remote_model_reply, run_result_is_terminal as run_is_terminal,
};

/// Hold one route slot for a create, or recognise a request that already has
/// its route (a retry after a lost reply). Check and hold share one lock and
/// one index write, so two creates cannot both take the last slot.
fn reserve_route_slot(
    data_dir: &Path,
    now: u64,
    key: &str,
    principal_id: &str,
    grant_id: &str,
    capsule_id: &str,
    request_id: &str,
) -> Result<RouteSlot, RemoteRouteError> {
    let capsule_id = route_capsule_id(capsule_id);
    match update_index(data_dir, now, |index| {
        let existing = index.runs.values().any(|route| {
            route.principal_id == principal_id
                && route.grant_id == grant_id
                && route_capsule_id(&route.capsule_id) == capsule_id
                && route.request_id == request_id
        });
        if existing {
            return Ok(RouteSlot::Existing);
        }
        if index.pending.contains_key(key) {
            anyhow::bail!("remote run reservation is already held");
        }
        anyhow::ensure!(
            index.runs.len() + index.pending.len() < REMOTE_RUN_INDEX_MAX,
            "remote run index is full"
        );
        index.pending.insert(key.to_string(), now);
        Ok(RouteSlot::Reserved)
    }) {
        Ok(slot) => Ok(slot),
        Err(err) => {
            let message = err.to_string();
            if message.contains("remote run index is full") {
                Err(RemoteRouteError::Rejected {
                    code: "rate_limited".into(),
                })
            } else {
                Err(RemoteRouteError::Invalid(message))
            }
        }
    }
}

fn commit_route_slot(
    data_dir: &Path,
    now: u64,
    key: &str,
    run_id: &str,
    route: RemoteRunRoute,
) -> anyhow::Result<()> {
    update_index(data_dir, now, |index| {
        index.pending.remove(key);
        index.runs.entry(run_id.to_string()).or_insert(route);
        Ok(())
    })
}

fn release_route_slot(data_dir: &Path, now: u64, key: &str) {
    if let Err(err) = update_index(data_dir, now, |index| {
        index.pending.remove(key);
        Ok(())
    }) {
        tracing::warn!("remote run reservation {key} could not be released: {err}");
    }
}

fn rewrite_protocol_offer_id(value: &mut Value, grant_id: &str) {
    if let Some(Value::String(offer_id)) = value.get_mut("offer_id") {
        if parse_remote_offer_id(offer_id).is_none() {
            *offer_id = remote_offer_id(grant_id, offer_id);
        }
    }
}

fn rewrite_protocol_offer_list(offers: Option<&mut Value>, grant_id: &str) {
    let Some(Value::Array(items)) = offers else {
        return;
    };
    for item in items {
        rewrite_protocol_offer_id(item, grant_id);
    }
}

/// Prefix protocol offer identities for the consumer. User, event, and output
/// objects keep their own fields.
fn rewrite_offer_ids(value: &mut Value, grant_id: &str) {
    rewrite_protocol_offer_id(value, grant_id);
    if let Some(data) = value.get_mut("data") {
        rewrite_protocol_offer_id(data, grant_id);
        if let Some(run) = data.get_mut("run") {
            rewrite_protocol_offer_id(run, grant_id);
        }
        rewrite_protocol_offer_list(data.get_mut("offers"), grant_id);
    }
    rewrite_protocol_offer_list(value.get_mut("offers"), grant_id);
}

/// Append the offers each approved grant currently shares. An unreachable
/// grant contributes a status entry and no offers.
/// The typed `offers_list` reply of a Home that runs no local model provider.
/// Remote offers append to it, so a Linux Home without a model still lists
/// every granted service. `local_provider` names why the local list is empty.
pub(crate) fn offers_list_without_local_provider() -> Value {
    json!({
        "status": "ok",
        "data": {
            "schema": "elastos.model.offers-list/v1",
            "provider": "model-provider",
            "protocol_version": "elastos.model-provider/v1",
            "offers": [],
            "local_provider": "unavailable",
        }
    })
}

pub(crate) async fn append_remote_offers(
    registry: &ProviderRegistry,
    grants: &[ConsumerModelGrant],
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    response: &mut Value,
) {
    let mut services = Vec::new();
    for grant in grants {
        let request = typed_request(
            &json!({ "op": "offers_list" }),
            context,
            capsule_id,
            &grant.grant_id,
        );
        match call_grant(registry, grant, "offers_list", request).await {
            Ok(result) => {
                let offers = result
                    .pointer("/data/offers")
                    .or_else(|| result.get("offers"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let count = offers.len();
                for mut offer in offers {
                    if let Some(Value::String(id)) = offer.get_mut("id") {
                        *id = remote_offer_id(&grant.grant_id, id);
                    }
                    offer["remote_service"] = json!({
                        "grant_id": grant.grant_id,
                        "peer_did": grant.peer_did,
                        "display_name": grant.display_name,
                        "expires_at": grant.expires_at,
                    });
                    if let Some(Value::Array(target)) = response.pointer_mut("/data/offers") {
                        target.push(offer);
                    } else if let Some(Value::Array(target)) = response.get_mut("offers") {
                        target.push(offer);
                    }
                }
                services.push(
                    json!({ "grant_id": grant.grant_id, "status": "reachable", "offers": count }),
                );
            }
            Err(RemoteRouteError::Rejected { code }) => services
                .push(json!({ "grant_id": grant.grant_id, "status": "rejected", "code": code })),
            Err(RemoteRouteError::Transport(_)) => services
                .push(json!({ "grant_id": grant.grant_id, "status": TRANSPORT_INTERRUPTED })),
            Err(RemoteRouteError::Invalid(message)) => services.push(
                json!({ "grant_id": grant.grant_id, "status": "invalid", "message": message }),
            ),
        }
    }
    if !services.is_empty() {
        if response.get("data").is_some_and(Value::is_object) {
            response["data"]["remote_services"] = Value::Array(services);
        } else {
            response["remote_services"] = Value::Array(services);
        }
    }
}

/// Route one run operation through the grant that owns it. `Ok(None)` means
/// the operation is local.
pub(crate) async fn route_run_operation(
    registry: Arc<ProviderRegistry>,
    data_dir: &Path,
    grants: &[ConsumerModelGrant],
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    request: (&str, &Value),
    now: u64,
) -> Result<Option<Value>, RemoteRouteError> {
    let (op, normalized) = request;
    match op {
        "runs_create" => {
            let offer_id = normalized["offer_id"].as_str().unwrap_or_default();
            let Some((grant_id, inner_offer_id)) = parse_remote_offer_id(offer_id) else {
                return Ok(None);
            };
            let grant = grants
                .iter()
                .find(|grant| grant.grant_id == grant_id)
                .ok_or_else(|| RemoteRouteError::Rejected {
                    code: "denied".to_string(),
                })?;
            let mut request = typed_request(normalized, context, capsule_id, grant_id);
            request["offer_id"] = Value::String(inner_offer_id.to_string());
            // A route slot is held before the granting Runtime starts work; a
            // retry of a routed request needs no slot. A full index refuses here.
            let request_id = normalized
                .pointer("/runtime_binding/request_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let key =
                route_reservation_key(&context.principal_id, grant_id, capsule_id, &request_id);
            let held_slot = matches!(
                reserve_route_slot(
                    data_dir,
                    now,
                    &key,
                    &context.principal_id,
                    grant_id,
                    capsule_id,
                    &request_id,
                )?,
                RouteSlot::Reserved
            );
            let mut result = match call_grant(&registry, grant, op, request).await {
                Ok(result) => result,
                Err(err) => {
                    if held_slot {
                        release_route_slot(data_dir, now, &key);
                    }
                    return Err(err);
                }
            };
            let reply = match normalize_remote_model_reply(&result) {
                Ok(reply) => reply,
                Err(_) => {
                    if held_slot {
                        release_route_slot(data_dir, now, &key);
                    }
                    return Err(RemoteRouteError::Invalid(
                        "model reply is ambiguous".to_string(),
                    ));
                }
            };
            if let Some(run_id) = reply.run_id {
                let route = RemoteRunRoute {
                    principal_id: context.principal_id.clone(),
                    grant_id: grant.grant_id.clone(),
                    peer_did: grant.peer_did.clone(),
                    connect_ticket: grant.connect_ticket.clone(),
                    display_name: grant.display_name.clone(),
                    offer_id: offer_id.to_string(),
                    request_id: request_id.clone(),
                    capsule_id: capsule_id.to_string(),
                    created_at: now,
                    terminal_at: run_is_terminal(&result).then_some(now),
                };
                if let Err(err) = commit_route_slot(data_dir, now, &key, &run_id, route) {
                    if held_slot {
                        release_route_slot(data_dir, now, &key);
                    }
                    // The granting Runtime runs work this Home cannot address;
                    // ask it to settle that run before reporting the failure.
                    let cancel = typed_request(
                        &json!({ "op": "runs_cancel", "run_id": run_id, "runtime_binding": {
                            "request_id": format!("unrouted:{run_id}") } }),
                        context,
                        capsule_id,
                        grant_id,
                    );
                    if let Err(cancel_err) =
                        call_grant(&registry, grant, "runs_cancel", cancel).await
                    {
                        tracing::warn!(
                            "unrouted remote model run {run_id}: cancel failed: {cancel_err}"
                        );
                    }
                    return Err(RemoteRouteError::Invalid(err.to_string()));
                }
            } else if held_slot {
                release_route_slot(data_dir, now, &key);
            }
            rewrite_offer_ids(&mut result, grant_id);
            Ok(Some(result))
        }
        "runs_get" | "runs_events" | "runs_cancel" => {
            let run_id = normalized["run_id"].as_str().unwrap_or_default();
            let Some(route) = remote_run_route(data_dir, &context.principal_id, run_id)
                .map_err(|err| RemoteRouteError::Invalid(err.to_string()))?
            else {
                return Ok(None);
            };
            // An active grant is preferred; a run created under a grant that
            // has since expired or been denied still settles through the
            // route it was created with. The granting Runtime owns that check.
            let grant = match grants.iter().find(|grant| grant.grant_id == route.grant_id) {
                Some(grant) => grant.clone(),
                None => route
                    .settlement_grant()
                    .ok_or_else(|| RemoteRouteError::Rejected {
                        code: "denied".to_string(),
                    })?,
            };
            let request = typed_request(normalized, context, capsule_id, &route.grant_id);
            let mut result = call_grant(&registry, &grant, op, request).await?;
            if route.terminal_at.is_none() && run_is_terminal(&result) {
                let run_id = run_id.to_string();
                let _ = update_index(data_dir, now, |index| {
                    if let Some(entry) = index.runs.get_mut(&run_id) {
                        entry.terminal_at = Some(now);
                    }
                    Ok(())
                });
            }
            rewrite_offer_ids(&mut result, &route.grant_id);
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Carrier route filters ticket endpoints by `did:key`; a raw peer id
    // there matched nothing and every call reported `transport_interrupted`.
    #[test]
    fn consumer_grant_routes_by_the_granting_runtime_did() {
        let key = iroh::SecretKey::from_bytes(&[7; 32]).public();
        let grant = ConsumerModelGrant::from_record(&json!({
            "grant_id": "services-remote-model-grant-0011223344556677",
            "peer_did": key.to_string(),
            "connect_ticket": "ticket",
            "service_display_name": "Mac",
            "expires_at": 1,
        }))
        .expect("grant");
        assert_eq!(
            grant.peer_did,
            crate::carrier::public_key_to_did(&key).unwrap()
        );
        assert!(grant.peer_did.starts_with("did:key:z6Mk"));
        assert!(ConsumerModelGrant::from_record(&json!({
            "grant_id": "g", "peer_did": "not-a-peer-id", "connect_ticket": "t", "expires_at": 1,
        }))
        .is_none());
    }

    #[test]
    fn a_home_without_a_local_model_provider_still_lists_remote_offers() {
        let mut response = offers_list_without_local_provider();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["schema"], "elastos.model.offers-list/v1");
        assert_eq!(response["data"]["offers"], json!([]));
        assert_eq!(response["data"]["local_provider"], "unavailable");
        // The same target array `append_remote_offers` pushes into.
        response
            .pointer_mut("/data/offers")
            .and_then(Value::as_array_mut)
            .expect("offers array")
            .push(json!({ "id": "remote:g:qwen" }));
        assert_eq!(response["data"]["offers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn remote_offer_ids_round_trip_and_reject_malformed_values() {
        let id = remote_offer_id("services-remote-model-grant-0011223344556677", "qwen3.5-9b");
        assert_eq!(
            parse_remote_offer_id(&id),
            Some(("services-remote-model-grant-0011223344556677", "qwen3.5-9b"))
        );
        assert_eq!(parse_remote_offer_id("qwen3.5-9b"), None);
        assert_eq!(parse_remote_offer_id("remote:"), None);
        assert_eq!(parse_remote_offer_id("remote:grant-only"), None);
        assert_eq!(parse_remote_offer_id("remote::offer"), None);
    }

    #[test]
    fn typed_request_replaces_the_local_binding_with_the_remote_fields() {
        let context = HomeLaunchTokenContext {
            principal_id: "seed-principal".into(),
            proof_binding_id: None,
            session_id: "s".into(),
            grant_id: "launch-grant".into(),
        };
        let normalized = json!({
            "op": "runs_create", "offer_id": "remote:g:qwen", "operation": "chat", "input": {"x": 1},
            "runtime_binding": { "principal_id": "seed-principal", "grant_id": "launch-grant", "request_id": "req-7" }
        });
        let request = typed_request(&normalized, &context, "assistant", "g");
        assert!(request.get("runtime_binding").is_none());
        assert_eq!(request["request_id"], "req-7");
        assert_eq!(request["remote_model"]["grant_id"], "g");
        assert_eq!(request["remote_model"]["principal_id"], "seed-principal");
        assert_eq!(request["remote_model"]["capsule_id"], "assistant");
    }

    #[test]
    fn carrier_errors_split_into_decisions_and_transport_interruptions() {
        assert!(
            matches!(classify_carrier_error("model grant is not active for this requester (denied)"), RemoteRouteError::Rejected { code } if code == "denied")
        );
        assert!(
            matches!(classify_carrier_error("rate_limited"), RemoteRouteError::Rejected { code } if code == "rate_limited")
        );
        assert!(matches!(
            classify_carrier_error("connection timed out"),
            RemoteRouteError::Transport(_)
        ));
        assert!(matches!(
            classify_carrier_error(
                "Carrier provider invocation requires registered Carrier invoker"
            ),
            RemoteRouteError::Transport(_)
        ));
    }

    #[test]
    fn offer_ids_in_results_are_rewritten_once() {
        let mut result = json!({
            "data": {
                "run": { "id": "run:sha256:aa", "offer_id": "qwen" },
                "offer_id": "remote:g:qwen",
                "terminal": { "output": { "offer_id": "nested-user-offer", "text": "hello" } }
            }
        });
        rewrite_offer_ids(&mut result, "g");
        assert_eq!(result["data"]["run"]["offer_id"], "remote:g:qwen");
        assert_eq!(result["data"]["offer_id"], "remote:g:qwen");
        assert_eq!(
            result["data"]["terminal"]["output"]["offer_id"],
            "nested-user-offer"
        );
    }

    #[test]
    fn remote_run_index_is_principal_scoped_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let route = RemoteRunRoute {
            principal_id: "p1".into(),
            grant_id: "g".into(),
            peer_did: "did:key:z6Mkpeer".into(),
            connect_ticket: "ticket".into(),
            display_name: "Mac".into(),
            offer_id: "remote:g:qwen".into(),
            request_id: "req".into(),
            capsule_id: "assistant".into(),
            created_at: 10,
            terminal_at: None,
        };
        // Routes written before tickets and names were stored still parse; an
        // installed index must never block new runs.
        let legacy: RemoteRunIndex = serde_json::from_value(json!({
            "schema": REMOTE_RUN_INDEX_SCHEMA,
            "runs": { "run:sha256:old": {
                "principal_id": "p1", "grant_id": "g", "peer_did": "did:key:z6Mkpeer",
                "offer_id": "remote:g:qwen", "request_id": "req", "created_at": 1 } }
        }))
        .expect("legacy index parses");
        assert_eq!(legacy.runs["run:sha256:old"].connect_ticket, "");
        assert_eq!(legacy.runs["run:sha256:old"].display_name, "");
        // A route written before tickets were stored cannot settle by itself.
        assert!(RemoteRunRoute {
            connect_ticket: String::new(),
            ..route.clone()
        }
        .settlement_grant()
        .is_none());
        assert_eq!(
            route.settlement_grant().map(|grant| grant.grant_id),
            Some("g".to_string())
        );
        update_index(dir.path(), 10, |index| {
            index.runs.insert("run:sha256:aa".into(), route.clone());
            index.runs.insert(
                "run:sha256:bb".into(),
                RemoteRunRoute {
                    terminal_at: Some(5),
                    ..route.clone()
                },
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(
            remote_run_route(dir.path(), "p1", "run:sha256:aa").unwrap(),
            Some(route)
        );
        assert_eq!(
            remote_run_route(dir.path(), "p2", "run:sha256:aa").unwrap(),
            None
        );
        update_index(dir.path(), 5 + REMOTE_RUN_RETENTION_SECS + 1, |_| Ok(())).unwrap();
        assert_eq!(
            remote_run_route(dir.path(), "p1", "run:sha256:bb").unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn local_offers_and_unknown_runs_stay_local() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let context = HomeLaunchTokenContext {
            principal_id: "p1".into(),
            proof_binding_id: None,
            session_id: "s".into(),
            grant_id: "g".into(),
        };
        let local_create = json!({ "op": "runs_create", "offer_id": "qwen", "operation": "chat", "input": {}, "runtime_binding": {} });
        assert!(route_run_operation(
            registry.clone(),
            dir.path(),
            &[],
            &context,
            "assistant",
            ("runs_create", &local_create),
            1
        )
        .await
        .unwrap()
        .is_none());
        let local_get =
            json!({ "op": "runs_get", "run_id": "run:sha256:unknown", "runtime_binding": {} });
        assert!(route_run_operation(
            registry.clone(),
            dir.path(),
            &[],
            &context,
            "assistant",
            ("runs_get", &local_get),
            1
        )
        .await
        .unwrap()
        .is_none());
        let remote_without_grant = json!({ "op": "runs_create", "offer_id": "remote:g:qwen", "operation": "chat", "input": {}, "runtime_binding": {} });
        let err = route_run_operation(
            registry,
            dir.path(),
            &[],
            &context,
            "assistant",
            ("runs_create", &remote_without_grant),
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RemoteRouteError::Rejected { code } if code == "denied"));
    }
}
