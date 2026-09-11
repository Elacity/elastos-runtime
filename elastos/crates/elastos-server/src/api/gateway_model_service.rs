//! Remote model service: the destination side of a `remote_model` grant.
//!
//! A seed Runtime routes typed model operations over Carrier `provider_invoke`
//! with target `model`. This Runtime owns the decision. It verifies the grant
//! from the source endpoint key, rewrites the request into a destination-owned
//! binding, filters offers to local engines, calls its own model provider, and
//! records every run it created for that grant so revocation can find them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use elastos_runtime::provider::ProviderRegistry;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::gateway_home_token::HomeLaunchTokenContext;
use super::gateway_provider_proxy::normalize_model_provider_request;

pub(crate) const MODEL_SERVICE_KIND: &str = "remote_model";
pub(crate) const MODEL_SERVICE_URI: &str = "elastos://peer/model";
pub(crate) const MODEL_LOCAL_OFFER: &str = "local:provider:model";
pub(crate) const MODEL_GRANT_SCHEMA: &str = "elastos.service.remote-model-grant/v1";
pub(crate) const MODEL_GRANT_SCOPE: &str = "principal_scoped_remote_model_grant";
pub(crate) const MODEL_GRANT_TTL_SECS: u64 = 6 * 3600;
pub(crate) const MODEL_RUN_RETENTION_SECS: u64 = 24 * 3600;
pub(crate) const MODEL_OPERATIONS: [&str; 5] = [
    "offers_list",
    "runs_create",
    "runs_get",
    "runs_events",
    "runs_cancel",
];
const MODEL_CONSUMER_CAPSULES: [&str; 2] = ["assistant", "home-agent"];
const MODEL_RUN_INDEX_SCHEMA: &str = "elastos.services.model-runs/v1";
const MODEL_RUN_INDEX_MAX: usize = 512;
const MODEL_REQUEST_MAX_BYTES: usize = 256 * 1024;
const AUTHORITY_DEADLINE: Duration = Duration::from_secs(1);

pub(crate) fn model_grant_id(request_id: &str) -> String {
    let digest = Sha256::digest(request_id.as_bytes());
    format!("services-remote-model-grant-{}", hex::encode(&digest[..8]))
}

/// The consumer principal's identity on this Runtime. Deterministic per source
/// Runtime and seed principal, so retries and restarts keep run ownership.
pub(crate) fn remote_principal_id(
    source_endpoint_did: &str,
    requester_principal_id: &str,
) -> String {
    let digest =
        Sha256::digest(format!("{source_endpoint_did}:{requester_principal_id}").as_bytes());
    format!("remote:{}", hex::encode(&digest[..16]))
}

/// A grant this Runtime issued for its shared local model offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelServiceGrant {
    pub provider_principal_id: String,
    pub requester_principal_id: String,
    pub requester_endpoint: iroh::PublicKey,
    pub grant_id: String,
    pub revision: u64,
    pub expires_at: u64,
}

impl ModelServiceGrant {
    pub fn validate(
        &self,
        source: &iroh::PublicKey,
        requester_principal_id: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.requester_endpoint == *source
                && requester_principal_id == self.requester_principal_id,
            "model grant requester mismatch"
        );
        anyhow::ensure!(
            self.revision > 0
                && self.revision <= now.saturating_add(30)
                && self.expires_at > now
                && self.expires_at > self.revision
                && self.expires_at - self.revision <= MODEL_GRANT_TTL_SECS,
            "model grant expired or invalid"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RemoteModelRunRecord {
    pub grant_id: String,
    pub source_endpoint_did: String,
    pub requester_principal_id: String,
    pub remote_principal_id: String,
    pub capsule_id: String,
    pub offer_id: String,
    pub request_id: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RemoteModelRunIndex {
    schema: String,
    #[serde(default)]
    runs: BTreeMap<String, RemoteModelRunRecord>,
}

fn run_index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("services-model-runs.json")
}

fn run_index_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

fn read_run_index(data_dir: &Path) -> anyhow::Result<RemoteModelRunIndex> {
    let path = run_index_path(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemoteModelRunIndex {
                schema: MODEL_RUN_INDEX_SCHEMA.to_string(),
                runs: BTreeMap::new(),
            });
        }
        Err(err) => return Err(err).context("model run index read"),
    };
    let index: RemoteModelRunIndex =
        serde_json::from_slice(&bytes).context("model run index parse")?;
    anyhow::ensure!(
        index.schema == MODEL_RUN_INDEX_SCHEMA,
        "model run index schema mismatch"
    );
    Ok(index)
}

fn write_run_index(data_dir: &Path, index: &RemoteModelRunIndex) -> anyhow::Result<()> {
    let path = run_index_path(data_dir);
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(index)?;
    std::fs::write(&temp, bytes).context("model run index write")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, &path).context("model run index commit")?;
    Ok(())
}

fn update_run_index<T>(
    data_dir: &Path,
    now: u64,
    update: impl FnOnce(&mut RemoteModelRunIndex) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _guard = run_index_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("model run index unavailable"))?;
    let mut index = read_run_index(data_dir)?;
    index.runs.retain(|_, record| {
        record
            .terminal_at
            .is_none_or(|terminal_at| now.saturating_sub(terminal_at) <= MODEL_RUN_RETENTION_SECS)
    });
    let result = update(&mut index)?;
    anyhow::ensure!(
        index.runs.len() <= MODEL_RUN_INDEX_MAX,
        "model run index is full"
    );
    write_run_index(data_dir, &index)?;
    Ok(result)
}

pub(crate) fn remote_model_run(
    data_dir: &Path,
    run_id: &str,
) -> anyhow::Result<Option<RemoteModelRunRecord>> {
    let _guard = run_index_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("model run index unavailable"))?;
    Ok(read_run_index(data_dir)?.runs.get(run_id).cloned())
}

pub(crate) fn remote_model_runs_for_grant(
    data_dir: &Path,
    grant_id: &str,
) -> anyhow::Result<Vec<(String, RemoteModelRunRecord)>> {
    let _guard = run_index_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("model run index unavailable"))?;
    Ok(read_run_index(data_dir)?
        .runs
        .into_iter()
        .filter(|(_, record)| record.grant_id == grant_id)
        .collect())
}

fn safe_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn denied(code: &str, message: &str) -> Value {
    json!({ "ok": false, "code": code, "error": message })
}

/// Bounded error classes cross Carrier; raw provider text stays on this Runtime.
fn redact_provider_error(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("rate") && lower.contains("limit") || lower.contains("concurrency") {
        "rate_limited"
    } else if lower.contains("offer")
        && (lower.contains("unknown")
            || lower.contains("not found")
            || lower.contains("unavailable"))
    {
        "offer_unavailable"
    } else if lower.contains("owner") || lower.contains("denied") || lower.contains("unauthorized")
    {
        "denied"
    } else {
        "provider_failure"
    }
}

fn provider_status_error(result: &Value) -> Option<String> {
    (result.get("status").and_then(Value::as_str) == Some("error")).then(|| {
        result
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| result.get("error").and_then(Value::as_str))
            .unwrap_or("provider error")
            .to_string()
    })
}

fn remote_context(record_principal: &str, grant_id: &str) -> HomeLaunchTokenContext {
    HomeLaunchTokenContext {
        principal_id: record_principal.to_string(),
        proof_binding_id: None,
        session_id: grant_id.to_string(),
        grant_id: grant_id.to_string(),
    }
}

/// Local engine offers are the shareable set. Hosted adapters stay private to
/// this Runtime's owner.
fn shareable_offers(result: &Value) -> Vec<Value> {
    result
        .pointer("/data/offers")
        .or_else(|| result.get("offers"))
        .and_then(Value::as_array)
        .map(|offers| {
            offers
                .iter()
                .filter(|offer| offer.get("hosted").is_none_or(Value::is_null))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn with_offers(mut result: Value, offers: Vec<Value>) -> Value {
    if result.pointer("/data/offers").is_some() {
        result["data"]["offers"] = Value::Array(offers);
    } else {
        result["offers"] = Value::Array(offers);
    }
    result
}

async fn read_authority(
    data_dir: &Path,
    network: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    source: iroh::PublicKey,
    grant_id: &str,
    requester_principal_id: &str,
) -> anyhow::Result<ModelServiceGrant> {
    let data_dir = data_dir.to_path_buf();
    let network = network.clone();
    let grant_id = grant_id.to_string();
    let requester_principal_id = requester_principal_id.to_string();
    tokio::time::timeout(
        AUTHORITY_DEADLINE,
        tokio::task::spawn_blocking(move || {
            super::gateway_home_system::authorize_home_service_model(
                &data_dir,
                &network,
                &source,
                &grant_id,
                &requester_principal_id,
                crate::auth::now_ts(),
            )
        }),
    )
    .await
    .context("model authority deadline")??
}

fn run_terminal(result: &Value) -> bool {
    let status = result
        .pointer("/data/run/status")
        .or_else(|| result.pointer("/data/status"))
        .or_else(|| result.pointer("/run/status"))
        .or_else(|| result.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "settlement_unknown"
    )
}

/// Serve one model operation for a remote consumer. Every path returns a
/// Carrier `provider_invoke` response object.
pub(crate) async fn invoke(
    registry: Arc<ProviderRegistry>,
    data_dir: &Path,
    network: Option<crate::collaboration_network::VerifiedCollaborationNetworkProfile>,
    source: iroh::PublicKey,
    data: &Value,
) -> Value {
    let operation = data["operation"].as_str().unwrap_or_default();
    if !MODEL_OPERATIONS.contains(&operation) {
        return denied(
            "invalid_provider_invocation",
            "model operation is not part of the service contract",
        );
    }
    let Some(request) = data.get("request").filter(|request| request.is_object()) else {
        return denied(
            "invalid_provider_invocation",
            "model request must be an object",
        );
    };
    if serde_json::to_vec(request)
        .map(|bytes| bytes.len() > MODEL_REQUEST_MAX_BYTES)
        .unwrap_or(true)
    {
        return denied(
            "invalid_provider_invocation",
            "model request exceeds the service size bound",
        );
    }
    if request.get("op").and_then(Value::as_str) != Some(operation) {
        return denied(
            "invalid_provider_invocation",
            "model request op must match the invocation operation",
        );
    }
    let remote = &request["remote_model"];
    let (Some(grant_id), Some(requester_principal_id), Some(capsule_id)) = (
        remote["grant_id"]
            .as_str()
            .filter(|value| safe_id(value, 128)),
        remote["principal_id"]
            .as_str()
            .filter(|value| safe_id(value, 128)),
        remote["capsule_id"]
            .as_str()
            .filter(|value| MODEL_CONSUMER_CAPSULES.contains(value)),
    ) else {
        return denied(
            "denied",
            "model request requires a remote grant, principal and consumer capsule",
        );
    };
    let Some(network) = network else {
        return denied(
            "denied",
            "model service network is not configured on this Runtime",
        );
    };
    let source_did = match crate::carrier::public_key_to_did(&source) {
        Ok(did) => did,
        Err(_) => return denied("denied", "model requester identity is invalid"),
    };
    let now = crate::auth::now_ts();

    // A fresh local request: only the fields the typed contract accepts.
    let mut local = request.clone();
    let local_object = local.as_object_mut().expect("request is an object");
    local_object.remove("remote_model");
    local_object.remove("_runtime_invocation");

    let (context, indexed_run) = match operation {
        "offers_list" | "runs_create" => {
            let grant =
                match read_authority(data_dir, &network, source, grant_id, requester_principal_id)
                    .await
                {
                    Ok(grant) => grant,
                    Err(err) => {
                        tracing::info!("remote model authority denied: {err}");
                        return denied("denied", "model grant is not active for this requester");
                    }
                };
            if let Err(err) = grant.validate(&source, requester_principal_id, now) {
                tracing::info!("remote model grant rejected: {err}");
                return denied("denied", "model grant is not active for this requester");
            }
            (
                remote_context(
                    &remote_principal_id(&source_did, requester_principal_id),
                    &grant.grant_id,
                ),
                None,
            )
        }
        _ => {
            let Some(run_id) = local_object
                .get("run_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return denied(
                    "invalid_provider_invocation",
                    "model run operation requires run_id",
                );
            };
            let record = match remote_model_run(data_dir, &run_id) {
                Ok(Some(record)) => record,
                Ok(None) => return denied("denied", "model run is not owned by this grant"),
                Err(err) => {
                    tracing::warn!("remote model run index unavailable: {err}");
                    return denied("provider_failure", "model run index is unavailable");
                }
            };
            if record.grant_id != grant_id
                || record.source_endpoint_did != source_did
                || record.requester_principal_id != requester_principal_id
                || record.capsule_id != capsule_id
            {
                return denied("denied", "model run is not owned by this grant");
            }
            (
                remote_context(&record.remote_principal_id, grant_id),
                Some((run_id, record)),
            )
        }
    };

    let normalized = match normalize_model_provider_request(operation, &local, &context, capsule_id)
    {
        Ok(normalized) => normalized,
        Err((_, message)) => {
            tracing::info!("remote model request rejected: {message}");
            return denied(
                "invalid_provider_invocation",
                "model request did not match the typed contract",
            );
        }
    };

    if operation == "runs_create" {
        let offers = match registry
            .send_raw("model", &json!({ "op": "offers_list" }))
            .await
        {
            Ok(result) => shareable_offers(&result),
            Err(err) => {
                tracing::warn!("remote model offers unavailable: {err}");
                return denied(
                    "offer_unavailable",
                    "model offers are unavailable on this Runtime",
                );
            }
        };
        let offer_id = normalized["offer_id"].as_str().unwrap_or_default();
        if !offers
            .iter()
            .any(|offer| offer["id"].as_str() == Some(offer_id))
        {
            return denied(
                "offer_unavailable",
                "model offer is not shared through this grant",
            );
        }
    }

    let result = match registry.send_raw("model", &normalized).await {
        Ok(result) => result,
        Err(err) => {
            let class = redact_provider_error(&err.to_string());
            tracing::info!("remote model provider error ({class}): {err}");
            return denied(class, "model provider rejected the operation");
        }
    };
    if let Some(message) = provider_status_error(&result) {
        let class = redact_provider_error(&message);
        tracing::info!("remote model provider status error ({class}): {message}");
        return denied(class, "model provider rejected the operation");
    }

    let result = match operation {
        "offers_list" => with_offers(result.clone(), shareable_offers(&result)),
        "runs_create" => {
            let run_id = result
                .pointer("/data/run/id")
                .or_else(|| result.pointer("/data/run_id"))
                .or_else(|| result.pointer("/run/id"))
                .or_else(|| result.get("run_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(run_id) = run_id {
                let record = RemoteModelRunRecord {
                    grant_id: grant_id.to_string(),
                    source_endpoint_did: source_did.clone(),
                    requester_principal_id: requester_principal_id.to_string(),
                    remote_principal_id: context.principal_id.clone(),
                    capsule_id: capsule_id.to_string(),
                    offer_id: normalized["offer_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    request_id: normalized
                        .pointer("/runtime_binding/request_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    created_at: now,
                    terminal_at: run_terminal(&result).then_some(now),
                };
                if let Err(err) = update_run_index(data_dir, now, |index| {
                    index.runs.entry(run_id.clone()).or_insert(record);
                    Ok(())
                }) {
                    tracing::warn!("remote model run index write failed: {err}");
                    return denied(
                        "provider_failure",
                        "model run could not be recorded on this Runtime",
                    );
                }
            }
            result
        }
        _ => {
            if let Some((run_id, record)) = indexed_run {
                if record.terminal_at.is_none() && run_terminal(&result) {
                    let _ = update_run_index(data_dir, now, |index| {
                        if let Some(entry) = index.runs.get_mut(&run_id) {
                            entry.terminal_at = Some(now);
                        }
                        Ok(())
                    });
                }
            }
            result
        }
    };
    json!({ "ok": true, "result": result })
}

/// Cancel every open run created under a grant, after revoke or denial.
/// Returns the run ids that received a cancel request.
pub(crate) async fn cancel_grant_runs(
    registry: Arc<ProviderRegistry>,
    data_dir: &Path,
    grant_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut cancelled = Vec::new();
    for (run_id, record) in remote_model_runs_for_grant(data_dir, grant_id)? {
        if record.terminal_at.is_some() {
            continue;
        }
        let context = remote_context(&record.remote_principal_id, grant_id);
        let request = json!({
            "op": "runs_cancel",
            "run_id": run_id,
            "request_id": format!("revoke:{}", record.request_id),
        });
        let Ok(normalized) =
            normalize_model_provider_request("runs_cancel", &request, &context, &record.capsule_id)
        else {
            continue;
        };
        match registry.send_raw("model", &normalized).await {
            Ok(result) if provider_status_error(&result).is_none() => cancelled.push(run_id),
            Ok(result) => tracing::info!(
                "revoke cancel for {run_id} returned {}",
                provider_status_error(&result).unwrap_or_default()
            ),
            Err(err) => tracing::info!("revoke cancel for {run_id} failed: {err}"),
        }
    }
    Ok(cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_principal_id_is_deterministic_and_per_principal() {
        let a1 = remote_principal_id("did:key:z6Mksource", "principal-a");
        let a2 = remote_principal_id("did:key:z6Mksource", "principal-a");
        let b = remote_principal_id("did:key:z6Mksource", "principal-b");
        let other_runtime = remote_principal_id("did:key:z6Mkother", "principal-a");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_ne!(a1, other_runtime);
        assert!(a1.starts_with("remote:"));
        assert_eq!(a1.len(), "remote:".len() + 32);
    }

    #[test]
    fn model_grant_id_is_stable_and_distinct_from_engine_ids() {
        let id = model_grant_id("request-1");
        assert_eq!(id, model_grant_id("request-1"));
        assert!(id.starts_with("services-remote-model-grant-"));
        assert_ne!(id, crate::carrier::engine_grant_id("request-1"));
    }

    #[test]
    fn grant_validation_checks_requester_expiry_and_ttl() {
        let key = iroh::SecretKey::from_bytes(&[71; 32]).public();
        let other = iroh::SecretKey::from_bytes(&[72; 32]).public();
        let grant = ModelServiceGrant {
            provider_principal_id: "owner".into(),
            requester_principal_id: "seed-principal".into(),
            requester_endpoint: key,
            grant_id: model_grant_id("r"),
            revision: 1_000,
            expires_at: 1_000 + MODEL_GRANT_TTL_SECS,
        };
        assert!(grant.validate(&key, "seed-principal", 1_500).is_ok());
        assert!(grant.validate(&other, "seed-principal", 1_500).is_err());
        assert!(grant.validate(&key, "another-principal", 1_500).is_err());
        assert!(grant
            .validate(&key, "seed-principal", 1_000 + MODEL_GRANT_TTL_SECS)
            .is_err());
        let too_long = ModelServiceGrant {
            expires_at: 1_000 + MODEL_GRANT_TTL_SECS + 1,
            ..grant.clone()
        };
        assert!(too_long.validate(&key, "seed-principal", 1_500).is_err());
    }

    #[test]
    fn shareable_offers_exclude_hosted_adapters() {
        let result = json!({ "status": "ok", "data": { "offers": [
            { "id": "qwen", "hosted": null },
            { "id": "gpt", "hosted": { "placement": "hosted" } },
            { "id": "local-2" }
        ] } });
        let ids: Vec<_> = shareable_offers(&result)
            .into_iter()
            .map(|offer| offer["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["qwen", "local-2"]);
        let filtered = with_offers(result.clone(), shareable_offers(&result));
        assert_eq!(filtered["data"]["offers"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn provider_errors_cross_carrier_only_as_bounded_classes() {
        assert_eq!(
            redact_provider_error("run concurrency limit reached for offer"),
            "rate_limited"
        );
        assert_eq!(
            redact_provider_error("unknown offer 'x' at /Users/anders/models"),
            "offer_unavailable"
        );
        assert_eq!(redact_provider_error("run owner mismatch"), "denied");
        assert_eq!(
            redact_provider_error("llama-server exited with code 137 at /tmp/x"),
            "provider_failure"
        );
    }

    #[test]
    fn run_index_round_trips_and_prunes_old_terminal_runs() {
        let dir = tempfile::tempdir().unwrap();
        let record = RemoteModelRunRecord {
            grant_id: "g".into(),
            source_endpoint_did: "did:key:z6Mksource".into(),
            requester_principal_id: "p".into(),
            remote_principal_id: remote_principal_id("did:key:z6Mksource", "p"),
            capsule_id: "assistant".into(),
            offer_id: "qwen".into(),
            request_id: "req-1".into(),
            created_at: 100,
            terminal_at: None,
        };
        update_run_index(dir.path(), 100, |index| {
            index.runs.insert("run:sha256:aaaa".into(), record.clone());
            index.runs.insert(
                "run:sha256:bbbb".into(),
                RemoteModelRunRecord {
                    terminal_at: Some(50),
                    request_id: "req-0".into(),
                    ..record.clone()
                },
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(
            remote_model_run(dir.path(), "run:sha256:aaaa").unwrap(),
            Some(record.clone())
        );
        assert_eq!(
            remote_model_runs_for_grant(dir.path(), "g").unwrap().len(),
            2
        );
        update_run_index(dir.path(), 50 + MODEL_RUN_RETENTION_SECS + 1, |_| Ok(())).unwrap();
        assert_eq!(
            remote_model_runs_for_grant(dir.path(), "g").unwrap().len(),
            1
        );
        assert!(remote_model_run(dir.path(), "run:sha256:bbbb")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn invoke_denies_requests_without_a_grant_or_with_a_foreign_capsule() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let source = iroh::SecretKey::from_bytes(&[73; 32]).public();
        let network = None;
        let missing = invoke(
            registry.clone(),
            dir.path(),
            network.clone(),
            source,
            &json!({ "operation": "offers_list", "request": { "op": "offers_list" } }),
        )
        .await;
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["code"], "denied");
        let foreign_capsule = invoke(
            registry.clone(),
            dir.path(),
            network.clone(),
            source,
            &json!({ "operation": "offers_list", "request": { "op": "offers_list",
                "remote_model": { "grant_id": "g", "principal_id": "p", "capsule_id": "browser" } } }),
        )
        .await;
        assert_eq!(foreign_capsule["code"], "denied");
        let wrong_op = invoke(
            registry,
            dir.path(),
            network,
            source,
            &json!({ "operation": "runs_delete", "request": { "op": "runs_delete" } }),
        )
        .await;
        assert_eq!(wrong_op["code"], "invalid_provider_invocation");
        assert!(!run_index_path(dir.path()).exists());
    }
}
