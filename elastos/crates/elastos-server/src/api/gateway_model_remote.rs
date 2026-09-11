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
const REMOTE_RUN_INDEX_MAX: usize = 1024;
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
    pub offer_id: String,
    pub request_id: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RemoteRunIndex {
    schema: String,
    #[serde(default)]
    runs: BTreeMap<String, RemoteRunRoute>,
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
    index.runs.retain(|_, route| {
        route
            .terminal_at
            .is_none_or(|terminal_at| now.saturating_sub(terminal_at) <= REMOTE_RUN_RETENTION_SECS)
    });
    let result = update(&mut index)?;
    anyhow::ensure!(
        index.runs.len() <= REMOTE_RUN_INDEX_MAX,
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
    fn from_record(grant: &Value) -> Option<Self> {
        Some(Self {
            grant_id: grant["grant_id"].as_str()?.to_string(),
            peer_did: grant["peer_did"].as_str()?.to_string(),
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

fn typed_request(
    normalized: &Value,
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    grant_id: &str,
) -> Value {
    let mut request = normalized.clone();
    if let Some(object) = request.as_object_mut() {
        object.remove("runtime_binding");
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

fn run_id_of(result: &Value) -> Option<String> {
    result
        .pointer("/data/run/id")
        .or_else(|| result.pointer("/data/run_id"))
        .or_else(|| result.pointer("/run/id"))
        .or_else(|| result.get("run_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn run_is_terminal(result: &Value) -> bool {
    let status = result
        .pointer("/data/run/status")
        .or_else(|| result.pointer("/data/status"))
        .or_else(|| result.pointer("/run/status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "settlement_unknown"
    )
}

fn rewrite_offer_ids(value: &mut Value, grant_id: &str) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(offer_id)) = map.get_mut("offer_id") {
                if parse_remote_offer_id(offer_id).is_none() {
                    *offer_id = remote_offer_id(grant_id, offer_id);
                }
            }
            for child in map.values_mut() {
                rewrite_offer_ids(child, grant_id);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_offer_ids(item, grant_id)),
        _ => {}
    }
}

/// Append the offers each approved grant currently shares. An unreachable
/// grant contributes a status entry and no offers.
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
    op: &str,
    normalized: &Value,
    now: u64,
) -> Result<Option<Value>, RemoteRouteError> {
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
            let mut result = call_grant(&registry, grant, op, request).await?;
            if let Some(run_id) = run_id_of(&result) {
                let route = RemoteRunRoute {
                    principal_id: context.principal_id.clone(),
                    grant_id: grant.grant_id.clone(),
                    peer_did: grant.peer_did.clone(),
                    offer_id: offer_id.to_string(),
                    request_id: normalized
                        .pointer("/runtime_binding/request_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    created_at: now,
                    terminal_at: run_is_terminal(&result).then_some(now),
                };
                update_index(data_dir, now, |index| {
                    index.runs.entry(run_id).or_insert(route);
                    Ok(())
                })
                .map_err(|err| RemoteRouteError::Invalid(err.to_string()))?;
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
            let grant = grants
                .iter()
                .find(|grant| grant.grant_id == route.grant_id)
                .ok_or_else(|| RemoteRouteError::Rejected {
                    code: "denied".to_string(),
                })?;
            let request = typed_request(normalized, context, capsule_id, &route.grant_id);
            let mut result = call_grant(&registry, grant, op, request).await?;
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
            "runtime_binding": { "principal_id": "seed-principal", "grant_id": "launch-grant" }
        });
        let request = typed_request(&normalized, &context, "assistant", "g");
        assert!(request.get("runtime_binding").is_none());
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
        let mut result = json!({ "data": { "run": { "id": "run:sha256:aa", "offer_id": "qwen" }, "offer_id": "remote:g:qwen" } });
        rewrite_offer_ids(&mut result, "g");
        assert_eq!(result["data"]["run"]["offer_id"], "remote:g:qwen");
        assert_eq!(result["data"]["offer_id"], "remote:g:qwen");
    }

    #[test]
    fn remote_run_index_is_principal_scoped_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let route = RemoteRunRoute {
            principal_id: "p1".into(),
            grant_id: "g".into(),
            peer_did: "did:key:z6Mkpeer".into(),
            offer_id: "remote:g:qwen".into(),
            request_id: "req".into(),
            created_at: 10,
            terminal_at: None,
        };
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
            "runs_create",
            &local_create,
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
            "runs_get",
            &local_get,
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
            "runs_create",
            &remote_without_grant,
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RemoteRouteError::Rejected { code } if code == "denied"));
    }
}
