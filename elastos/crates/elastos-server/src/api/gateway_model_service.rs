//! Remote model service: the destination side of a `remote_model` grant.
//!
//! A seed Runtime routes typed model operations over Carrier `provider_invoke`
//! with target `model`. This Runtime owns the decision. It verifies the grant
//! from the source endpoint key, rewrites the request into a destination-owned
//! binding, filters offers to shared local engines and hosted offers the owner
//! enabled for Share, calls its own model provider, and records every run it
//! created for that grant so revocation can find them. Unshared hosted
//! connections stay private.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
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
pub(crate) const MODEL_GRANT_SCHEMA: &str = "elastos.service.remote-model-grant/v2";
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

/// A Share or Disconnect decision waits for an admitted create to reach the
/// provider. New creates then observe the completed owner decision.
pub(in crate::api::gateway) fn model_share_gate() -> &'static tokio::sync::RwLock<()> {
    static GATE: OnceLock<tokio::sync::RwLock<()>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::RwLock::new(()))
}

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
    /// Exact model offers present when the person approved this grant.
    /// An older grant has no snapshot and cannot authorize a new run.
    pub approved_offer_ids: BTreeSet<String>,
    pub approved_offer_revision: String,
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
        anyhow::ensure!(
            self.approved_offer_ids.len() == 1
                && self.approved_offer_ids.iter().all(|id| safe_id(id, 256)),
            "model grant needs one exact approved offer"
        );
        anyhow::ensure!(
            self.approved_offer_revision.len() == 64
                && self
                    .approved_offer_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "model grant needs an exact offer revision"
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
    /// The typed operation of the run, kept so this Runtime can still answer
    /// a settlement read after the provider journal has pruned the run.
    #[serde(default)]
    pub operation: String,
    pub request_id: String,
    /// Canonical hash of the create input. Empty on records written before
    /// this field existed; those records still parse and still retry.
    #[serde(default)]
    pub input_hash: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<u64>,
    /// The settled status this Runtime observed, durable beyond the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_status: Option<String>,
}

impl RemoteModelRunRecord {
    /// The durable settlement reply for a run the provider no longer knows:
    /// the observed terminal status, or `settlement_unknown` when this
    /// Runtime never saw the run settle. Output is not retained here.
    fn settlement_view(&self, run_id: &str) -> Value {
        let status = self
            .terminal_status
            .clone()
            .unwrap_or_else(|| "settlement_unknown".to_string());
        json!({ "status": "ok", "data": {
            "schema": "elastos.model.run-view/v1",
            "run_id": run_id,
            "offer_id": self.offer_id,
            "operation": self.operation,
            "status": status,
            "sequence_cursor": 0,
            "terminal": { "status": status },
            "settlement_source": "runtime_index",
            "output_retained": false,
        } })
    }

    fn settlement_events_page(&self, run_id: &str, after_sequence: u64) -> Value {
        let status = self
            .terminal_status
            .clone()
            .unwrap_or_else(|| "settlement_unknown".to_string());
        let kind = match status.as_str() {
            "completed" => "completed",
            "failed" => "failed",
            "cancelled" => "cancelled",
            _ => "settlement_unknown",
        };
        // An attached client has already consumed prepared/dispatched events.
        // Emit the terminal at the next compatible sequence so a cursor of 2
        // or 7 still settles, and keep next_cursor equal to that sequence.
        // Completed index settlement keeps the status and states that output
        // is no longer retained; it does not send an empty output payload.
        let sequence = after_sequence.saturating_add(1);
        let event_data = if kind == "completed" {
            json!({ "output_retained": false })
        } else {
            json!({})
        };
        json!({ "status": "ok", "data": {
            "schema": "elastos.model.run-events/v1",
            "run_id": run_id,
            "next_cursor": sequence,
            "has_more": false,
            "events": [json!({
                "schema": "elastos.model.run-event/v1",
                "sequence": sequence,
                "kind": kind,
                "data": event_data,
                "terminal": true,
            })],
            "settlement_source": "runtime_index",
            "output_retained": false,
        } })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RemoteModelRunIndex {
    schema: String,
    #[serde(default)]
    runs: BTreeMap<String, RemoteModelRunRecord>,
    /// Slots held for runs between their reservation and their record:
    /// reservation key to reservation time. They count toward capacity.
    #[serde(default)]
    pending: BTreeMap<String, u64>,
}

/// A slot held longer than this belongs to a create that never committed or
/// released (a crash between dispatch and record); it returns to capacity.
const MODEL_RUN_PENDING_TTL_SECS: u64 = 600;

/// One create attempt is identified by who asked for what: the grant, the
/// destination-owned principal, the consumer capsule and the request id.
/// A retry of that same request reuses its record and never consumes a
/// second slot. Another capsule with the same request id is a different run.
fn run_reservation_key(
    grant_id: &str,
    remote_principal_id: &str,
    capsule_id: &str,
    request_id: &str,
) -> String {
    format!("{grant_id}:{remote_principal_id}:{capsule_id}:{request_id}")
}

fn record_capsule_id(record: &RemoteModelRunRecord) -> &str {
    if record.capsule_id.is_empty() {
        "assistant"
    } else {
        &record.capsule_id
    }
}

enum RunSlot {
    /// This request already has a run record. Answer from that record
    /// and, while the run is still open, a live journal read.
    Existing {
        run_id: String,
        record: Box<RemoteModelRunRecord>,
    },
    /// A slot is held under the reservation key until commit or release.
    Reserved,
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
                pending: BTreeMap::new(),
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
    // A settled record leaves after the retention window. An open record
    // leaves after the same window from creation: no offer runs that long
    // (the provider bounds runtime in seconds), so such a record is a run
    // whose settlement this Runtime never observed.
    index.runs.retain(|_, record| {
        let anchor = record.terminal_at.unwrap_or(record.created_at);
        now.saturating_sub(anchor) <= MODEL_RUN_RETENTION_SECS
    });
    index
        .pending
        .retain(|_, reserved_at| now.saturating_sub(*reserved_at) <= MODEL_RUN_PENDING_TTL_SECS);
    let result = update(&mut index)?;
    anyhow::ensure!(
        index.runs.len() + index.pending.len() <= MODEL_RUN_INDEX_MAX,
        "model run index is full"
    );
    write_run_index(data_dir, &index)?;
    Ok(result)
}

/// Hold one slot for a create, or recognise that the request already has a
/// record. The check and the hold happen under the same lock and land in the
/// same index write, so two creates cannot both take the last slot.
fn reserve_run_slot(
    data_dir: &Path,
    now: u64,
    key: &str,
    grant_id: &str,
    remote_principal_id: &str,
    capsule_id: &str,
    request_id: &str,
) -> anyhow::Result<RunSlot> {
    update_run_index(data_dir, now, |index| {
        if let Some((run_id, record)) = index.runs.iter().find(|(_, record)| {
            record.grant_id == grant_id
                && record.remote_principal_id == remote_principal_id
                && record_capsule_id(record) == capsule_id
                && record.request_id == request_id
        }) {
            return Ok(RunSlot::Existing {
                run_id: run_id.clone(),
                record: Box::new(record.clone()),
            });
        }
        if index.pending.contains_key(key) {
            anyhow::bail!("model run reservation is already held");
        }
        anyhow::ensure!(
            index.runs.len() + index.pending.len() < MODEL_RUN_INDEX_MAX,
            "model run index is full"
        );
        index.pending.insert(key.to_string(), now);
        Ok(RunSlot::Reserved)
    })
}

/// Turn a held slot into the run's record (or keep an existing record).
fn commit_run_slot(
    data_dir: &Path,
    now: u64,
    key: &str,
    run_id: &str,
    record: RemoteModelRunRecord,
) -> anyhow::Result<()> {
    update_run_index(data_dir, now, |index| {
        index.pending.remove(key);
        index.runs.entry(run_id.to_string()).or_insert(record);
        Ok(())
    })
}

/// Return a held slot to capacity after a create that dispatched nothing.
fn release_run_slot(data_dir: &Path, now: u64, key: &str) {
    if let Err(err) = update_run_index(data_dir, now, |index| {
        index.pending.remove(key);
        Ok(())
    }) {
        tracing::warn!("model run reservation {key} could not be released: {err}");
    }
}

/// Best-effort cancel of a run this Runtime dispatched but could not accept.
/// Returns true when the provider accepted the cancel request.
async fn cancel_unrecorded_run(
    registry: &ProviderRegistry,
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    run_id: &str,
) -> bool {
    let request = json!({ "run_id": run_id, "request_id": format!("unrecorded:{run_id}") });
    let Ok(normalized) =
        normalize_model_provider_request("runs_cancel", &request, context, capsule_id)
    else {
        tracing::warn!("unrecorded model run {run_id} could not form a cancel request");
        return false;
    };
    match registry.send_raw("model", &normalized).await {
        Ok(result) if provider_status_error(&result).is_none() => {
            tracing::info!("unrecorded model run {run_id}: cancel requested");
            true
        }
        Ok(result) => {
            tracing::warn!(
                "unrecorded model run {run_id}: cancel returned {}",
                provider_status_error(&result).unwrap_or_default()
            );
            false
        }
        Err(err) => {
            tracing::warn!("unrecorded model run {run_id}: cancel failed: {err}");
            false
        }
    }
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

pub(in crate::api::gateway) fn safe_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn denied(code: &str, message: &str) -> Value {
    json!({ "ok": false, "code": code, "error": message })
}

pub(crate) const INVOCATION_REFUSAL_SCHEMA: &str = "elastos.model.invocation-refusal/v1";

/// Binds a refusal to this invocation, without settling any previous attempt.
pub(crate) fn invocation_refusal_binding(request: &Value) -> Option<Value> {
    if request["op"] != "runs_create" || request["remote_model"]["refusal_scope"] != "invocation" {
        return None;
    }
    Some(json!({
        "schema": INVOCATION_REFUSAL_SCHEMA,
        "scope": "invocation", "dispatch": "not_started",
        "request_id": request.get("request_id")?.as_str()?,
        "offer_id": request.get("offer_id")?.as_str()?,
        "operation": request.get("operation")?.as_str()?,
        "input_hash": elastos_model_contract::model_input_hash(request.get("input")?).ok()?,
        "remote_model": request.get("remote_model")?,
    }))
}

fn refused_before_dispatch(
    data_dir: &Path,
    source_did: &str,
    request: &Value,
    code: &str,
    message: &str,
    owns_reservation: bool,
) -> Value {
    let Some(binding) = invocation_refusal_binding(request) else {
        return denied(code, message);
    };
    let grant_id = request["remote_model"]["grant_id"]
        .as_str()
        .unwrap_or_default();
    let principal = remote_principal_id(
        source_did,
        request["remote_model"]["principal_id"]
            .as_str()
            .unwrap_or_default(),
    );
    let capsule = request["remote_model"]["capsule_id"]
        .as_str()
        .unwrap_or_default();
    let request_id = request["request_id"].as_str().unwrap_or_default();
    let Ok(index) = read_run_index(data_dir) else {
        return denied(code, message);
    };
    let known = index.runs.values().any(|run| {
        run.grant_id == grant_id
            && run.remote_principal_id == principal
            && record_capsule_id(run) == capsule
            && run.request_id == request_id
    });
    let pending = index.pending.contains_key(&run_reservation_key(
        grant_id, &principal, capsule, request_id,
    ));
    if known || (pending && !owns_reservation) {
        return denied(code, message);
    }
    json!({ "ok": true, "result": {
        "status": "error", "code": "remote_model_invocation_refused",
        "reason": code, "message": "This invocation was refused before provider dispatch.",
        "refusal": binding,
    } })
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

fn provider_error_code(result: &Value) -> &str {
    result.get("code").and_then(Value::as_str).unwrap_or("")
}

fn mark_run_settled(data_dir: &Path, now: u64, run_id: &str, status: &str) {
    let _ = update_run_index(data_dir, now, |index| {
        if let Some(entry) = index.runs.get_mut(run_id) {
            if entry.terminal_at.is_none() {
                entry.terminal_at = Some(now);
                entry.terminal_status = Some(status.to_string());
            }
        }
        Ok(())
    });
}

fn record_for_index_settlement(
    data_dir: &Path,
    now: u64,
    run_id: &str,
    record: &RemoteModelRunRecord,
) -> RemoteModelRunRecord {
    if record.terminal_status.is_some() {
        return record.clone();
    }
    mark_run_settled(data_dir, now, run_id, "settlement_unknown");
    RemoteModelRunRecord {
        terminal_at: Some(now),
        terminal_status: Some("settlement_unknown".into()),
        ..record.clone()
    }
}

fn index_settlement_reply(
    data_dir: &Path,
    now: u64,
    operation: &str,
    run_id: &str,
    record: &RemoteModelRunRecord,
    after_sequence: u64,
) -> Value {
    let record = record_for_index_settlement(data_dir, now, run_id, record);
    let reply = if operation == "runs_events" {
        record.settlement_events_page(run_id, after_sequence)
    } else {
        record.settlement_view(run_id)
    };
    json!({ "ok": true, "result": reply })
}

struct RetainedRunQuery<'a> {
    operation: &'a str,
    run_id: &'a str,
    record: &'a RemoteModelRunRecord,
    after_sequence: u64,
}

fn recover_missing_run_or_deny(
    data_dir: &Path,
    now: u64,
    query: RetainedRunQuery<'_>,
    result: &Value,
    message: &str,
) -> Value {
    let RetainedRunQuery {
        operation,
        run_id,
        record,
        after_sequence,
    } = query;
    if provider_error_code(result) == "run_not_found" {
        tracing::info!(
            "remote model run {run_id} settled from the Runtime record after the provider forgot it"
        );
        return index_settlement_reply(data_dir, now, operation, run_id, record, after_sequence);
    }
    let class = redact_provider_error(message);
    tracing::info!("remote model provider status error ({class}): {message}");
    denied(class, "model provider rejected the operation")
}

fn request_conflicts_with_record(record: &RemoteModelRunRecord, normalized: &Value) -> bool {
    let offer_id = normalized["offer_id"].as_str().unwrap_or("");
    let operation = normalized["operation"].as_str().unwrap_or("");
    let input_hash = normalized
        .pointer("/runtime_binding/input_hash")
        .and_then(Value::as_str)
        .unwrap_or("");
    let capsule_id = normalized
        .pointer("/runtime_binding/capsule_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    (!record.capsule_id.is_empty() && record.capsule_id != capsule_id)
        || (!record.offer_id.is_empty() && record.offer_id != offer_id)
        || (!record.operation.is_empty() && record.operation != operation)
        || (!record.input_hash.is_empty() && record.input_hash != input_hash)
}

fn refuse_unbound_or_conflicting_create(
    record: &RemoteModelRunRecord,
    normalized: &Value,
) -> Option<Value> {
    if request_conflicts_with_record(record, normalized) {
        return Some(denied(
            "denied",
            "request_id conflicts with an existing model run",
        ));
    }
    // A pre-repair record has no input hash. RunView does not carry the
    // original fingerprint, so this Runtime cannot prove a replay. Refuse
    // create here; owned get, events and cancel still use the record.
    if record.input_hash.is_empty() {
        return Some(denied(
            "request_unbound",
            "this request cannot be replayed because its original input was not retained",
        ));
    }
    None
}

async fn answer_retained_create(
    registry: &ProviderRegistry,
    data_dir: &Path,
    now: u64,
    context: &HomeLaunchTokenContext,
    capsule_id: &str,
    normalized: &Value,
    retained: (String, RemoteModelRunRecord),
) -> Value {
    let (run_id, record) = retained;
    let request_id = normalized
        .pointer("/runtime_binding/request_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(refusal) = refuse_unbound_or_conflicting_create(&record, normalized) {
        return refusal;
    }
    let get_request = json!({
        "run_id": run_id,
        "request_id": format!("retry:{request_id}"),
    });
    let get_normalized =
        match normalize_model_provider_request("runs_get", &get_request, context, capsule_id) {
            Ok(normalized) => normalized,
            Err((_, message)) => {
                tracing::info!("remote model retained get rejected: {message}");
                return denied(
                    "invalid_provider_invocation",
                    "model request did not match the typed contract",
                );
            }
        };
    let result = match registry.send_raw("model", &get_normalized).await {
        Ok(result) => result,
        Err(err) => {
            let class = redact_provider_error(&err.to_string());
            tracing::info!("remote model provider error ({class}): {err}");
            return denied(class, "model provider rejected the operation");
        }
    };
    if let Some(message) = provider_status_error(&result) {
        return recover_missing_run_or_deny(
            data_dir,
            now,
            RetainedRunQuery {
                operation: "runs_get",
                run_id: &run_id,
                record: &record,
                after_sequence: 0,
            },
            &result,
            &message,
        );
    }
    if let Some(status) =
        run_result_terminal_status(&result).filter(|_| record.terminal_at.is_none())
    {
        mark_run_settled(data_dir, now, &run_id, &status);
    }
    json!({ "ok": true, "result": result })
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

/// Local engine offers are shareable. A hosted offer is shareable only after
/// the owner enables Share on that connection.
fn shareable_offers(result: &Value) -> Vec<Value> {
    result
        .pointer("/data/offers")
        .or_else(|| result.get("offers"))
        .and_then(Value::as_array)
        .map(|offers| {
            offers
                .iter()
                .filter(|offer| crate::api::offer_is_shareable(offer))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn listed_offers_array_mut(result: &mut Value) -> Option<&mut Vec<Value>> {
    let offers = if result.pointer("/data/offers").is_some() {
        result.pointer_mut("/data/offers")
    } else {
        result.get_mut("offers")
    };
    match offers {
        Some(Value::Array(offers)) => Some(offers),
        _ => None,
    }
}

fn annotate_listed_offers_with_hosted_share(result: &mut Value, data_dir: &Path) {
    let Ok(operator_offers) = crate::api::load_model_provider_operator_offers(data_dir) else {
        return;
    };
    let Some(listed) = listed_offers_array_mut(result) else {
        return;
    };
    for offer in listed {
        let Some(id) = offer.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(operator) = operator_offers
            .iter()
            .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(id))
        else {
            continue;
        };
        if let Some(share) = operator.get("share") {
            offer["share"] = share.clone();
        }
        if let Some(enabled) = operator.get("enabled") {
            offer["enabled"] = enabled.clone();
        }
        if crate::api::model_provider_config::operator_offer_has_key(operator) {
            offer["key_present"] = Value::Bool(true);
        }
    }
}

fn listed_offer_is_hosted(offer: &Value) -> bool {
    offer.get("hosted").is_some_and(|value| !value.is_null())
        || offer
            .pointer("/adapter/hosted")
            .is_some_and(|value| !value.is_null())
}

fn shareable_listed_offers(result: &Value, include_local: bool) -> Vec<Value> {
    shareable_offers(result)
        .into_iter()
        .filter(|offer| include_local || listed_offer_is_hosted(offer))
        .collect()
}

fn public_shared_offer(mut offer: Value) -> Value {
    if let Some(object) = offer.as_object_mut() {
        object.remove("share");
        object.remove("key_present");
        object.remove("enabled");
    }
    if let Some(hosted) = offer.get_mut("hosted").and_then(Value::as_object_mut) {
        hosted.remove("privacy_policy_ref");
        hosted.remove("terms_ref");
        if !hosted.contains_key("payer") {
            hosted.insert("payer".to_string(), Value::String("this Home".to_string()));
        }
        if !hosted.contains_key("intermediary") {
            hosted.insert(
                "intermediary".to_string(),
                Value::String("this Home".to_string()),
            );
        }
    }
    offer
}

pub(in crate::api::gateway) fn shareable_offers_for_home(
    result: &Value,
    data_dir: &Path,
    include_local: bool,
) -> Vec<Value> {
    let mut annotated = result.clone();
    annotate_listed_offers_with_hosted_share(&mut annotated, data_dir);
    shareable_listed_offers(&annotated, include_local)
        .into_iter()
        .map(public_shared_offer)
        .collect()
}

pub(in crate::api::gateway) fn offer_execution_revision<'a>(
    result: &'a Value,
    id: &str,
) -> Option<&'a str> {
    result["data"]["offer_revisions"][id]
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(in crate::api::gateway) fn shareable_offer_for_home(
    result: &Value,
    data_dir: &Path,
    include_local: bool,
    id: &str,
) -> Option<Value> {
    shareable_offers_for_home(result, data_dir, include_local)
        .into_iter()
        .find(|offer| offer["id"].as_str() == Some(id) && offer["operation"] == "text.generate")
}

fn shareable_offers_for_grant(
    result: &Value,
    data_dir: &Path,
    include_local: bool,
    approved_offer_ids: &BTreeSet<String>,
    approved_offer_revision: &str,
) -> Vec<Value> {
    shareable_offers_for_home(result, data_dir, include_local)
        .into_iter()
        .filter(|offer| {
            offer["id"].as_str().is_some_and(|id| {
                approved_offer_ids.contains(id)
                    && offer_execution_revision(result, id) == Some(approved_offer_revision)
            })
        })
        .collect()
}

fn with_offers(mut result: Value, offers: Vec<Value>) -> Value {
    if result.pointer("/data/offers").is_some() {
        result["data"]["offers"] = Value::Array(offers);
        if let Some(data) = result.get_mut("data").and_then(Value::as_object_mut) {
            data.remove("offer_revisions");
        }
    } else {
        result["offers"] = Value::Array(offers);
        if let Some(object) = result.as_object_mut() {
            object.remove("offer_revisions");
        }
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

async fn grant_is_active(
    data_dir: &Path,
    network: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    source: iroh::PublicKey,
    grant_id: &str,
    requester_principal_id: &str,
) -> bool {
    let now = crate::auth::now_ts();
    match read_authority(data_dir, network, source, grant_id, requester_principal_id).await {
        Ok(grant) => grant.validate(&source, requester_principal_id, now).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CreateRacePoint {
    BeforeDispatch,
    BeforeCommit,
}

#[cfg(test)]
struct CreateRaceBarrier {
    prepared: Arc<tokio::sync::Barrier>,
    release: tokio::sync::watch::Sender<bool>,
}

#[cfg(test)]
type CreateRaceBarriers =
    std::collections::BTreeMap<(PathBuf, CreateRacePoint), Arc<CreateRaceBarrier>>;
#[cfg(test)]
static CREATE_RACE_BARRIERS: std::sync::OnceLock<std::sync::Mutex<CreateRaceBarriers>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) struct CreateRaceBarrierGuard {
    data_dir: PathBuf,
    point: CreateRacePoint,
    barrier: Arc<CreateRaceBarrier>,
}

#[cfg(test)]
impl CreateRaceBarrierGuard {
    pub(crate) async fn wait_prepared(&self) {
        self.barrier.prepared.wait().await;
    }

    pub(crate) fn release(&self) {
        let _ = self.barrier.release.send(true);
    }
}

#[cfg(test)]
impl Drop for CreateRaceBarrierGuard {
    fn drop(&mut self) {
        if let Ok(mut slots) = CREATE_RACE_BARRIERS.get_or_init(Default::default).lock() {
            slots.remove(&(self.data_dir.clone(), self.point));
        }
        let _ = self.barrier.release.send(true);
    }
}

#[cfg(test)]
pub(crate) fn install_create_race_barrier(
    data_dir: impl AsRef<Path>,
    point: CreateRacePoint,
) -> CreateRaceBarrierGuard {
    let data_dir = data_dir.as_ref().to_path_buf();
    let (release, _) = tokio::sync::watch::channel(false);
    let barrier = Arc::new(CreateRaceBarrier {
        prepared: Arc::new(tokio::sync::Barrier::new(2)),
        release,
    });
    CREATE_RACE_BARRIERS
        .get_or_init(Default::default)
        .lock()
        .expect("create race barrier")
        .insert((data_dir.clone(), point), barrier.clone());
    CreateRaceBarrierGuard {
        data_dir,
        point,
        barrier,
    }
}

#[cfg(test)]
async fn await_create_race_barrier(data_dir: &Path, point: CreateRacePoint) {
    let barrier = CREATE_RACE_BARRIERS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|slots| slots.get(&(data_dir.to_path_buf(), point)).cloned());
    let Some(barrier) = barrier else {
        return;
    };
    let mut released = barrier.release.subscribe();
    barrier.prepared.wait().await;
    while !*released.borrow() {
        if released.changed().await.is_err() {
            break;
        }
    }
}

/// Whether a typed model reply shows the run has settled. `runs_create`,
/// `runs_get` and `runs_cancel` answer with a run view whose `status` is one of
/// the terminal statuses (a `terminal` outcome accompanies it). `runs_events`
/// answers with an events page and no status; its terminal event carries
/// `terminal: true`. Both indexes settle their records from this one reading.
pub(super) fn run_result_is_terminal(result: &Value) -> bool {
    run_result_terminal_status(result).is_some()
}

const RUN_TERMINAL_STATUSES: [&str; 4] = ["completed", "failed", "cancelled", "settlement_unknown"];

/// The settled status a typed model reply reports, if any: the run view's
/// `status` or `terminal.status`, or the kind of the events page's terminal
/// event (`output` settles as `completed`).
pub(super) fn run_result_terminal_status(result: &Value) -> Option<String> {
    let data = &result["data"];
    let terminal = |value: &Value| {
        value
            .as_str()
            .filter(|status| RUN_TERMINAL_STATUSES.contains(status))
            .map(str::to_string)
    };
    terminal(&data["status"])
        .or_else(|| terminal(&data["terminal"]["status"]))
        .or_else(|| {
            data["events"].as_array()?.iter().find_map(|event| {
                (event["terminal"] == true).then(|| match event["kind"].as_str() {
                    Some("output") | None => "completed".to_string(),
                    Some(kind) => terminal(&Value::String(kind.to_string()))
                        .unwrap_or_else(|| "completed".to_string()),
                })
            })
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NormalizedRemoteModelReply {
    pub run_id: Option<String>,
    pub offer_id: Option<String>,
    pub terminal_status: Option<String>,
}

fn unique_string_pointers(
    result: &Value,
    pointers: &[&str],
) -> Result<Option<String>, &'static str> {
    let mut found = None;
    for pointer in pointers {
        let Some(text) = result.pointer(pointer).and_then(Value::as_str) else {
            continue;
        };
        match &found {
            None => found = Some(text.to_string()),
            Some(existing) if existing == text => {}
            Some(_) => return Err("ambiguous remote model reply"),
        }
    }
    Ok(found)
}

fn protocol_offer_list(result: &Value) -> Result<Option<&Vec<Value>>, &'static str> {
    let nested = result.pointer("/data/offers");
    let top = result.get("offers");
    match (nested, top) {
        (Some(left), Some(right)) if left != right => Err("ambiguous remote model reply"),
        (Some(Value::Array(items)), _) | (_, Some(Value::Array(items))) => Ok(Some(items)),
        (Some(_), _) | (_, Some(_)) => Err("ambiguous remote model reply"),
        (None, None) => Ok(None),
    }
}

/// Read run id, offer id, and terminal status from a supported peer reply.
/// Nested user and output objects stay on the original value. Two disagreeing
/// identities are an ambiguous envelope.
pub(super) fn normalize_remote_model_reply(
    result: &Value,
) -> Result<NormalizedRemoteModelReply, &'static str> {
    let run_id = unique_string_pointers(result, &REMOTE_RUN_ID_POINTERS)?;
    let offer_id = unique_string_pointers(result, &["/data/offer_id", "/offer_id"])?;
    protocol_offer_list(result)?;
    Ok(NormalizedRemoteModelReply {
        run_id,
        offer_id,
        terminal_status: run_result_terminal_status(result),
    })
}

const REMOTE_RUN_ID_POINTERS: [&str; 4] = ["/data/run/id", "/data/run_id", "/run/id", "/run_id"];

/// After dispatch, keep ownership of accepted work. A trustworthy run ID is
/// recorded and cancelled. A reservation without a run ID stays held so a
/// retry cannot dispatch again.
async fn reject_dispatched_create(
    registry: &ProviderRegistry,
    data_dir: &Path,
    reservation_key: &str,
    held_slot: bool,
    context: &HomeLaunchTokenContext,
    record: RemoteModelRunRecord,
    result: &Value,
) -> Value {
    let now = record.created_at;
    let capsule_id = record.capsule_id.clone();
    let trustworthy_run_id = unique_string_pointers(result, &REMOTE_RUN_ID_POINTERS)
        .ok()
        .flatten();
    if let Some(run_id) = trustworthy_run_id {
        match commit_run_slot(data_dir, now, reservation_key, &run_id, record) {
            Ok(()) => {
                let status = if cancel_unrecorded_run(registry, context, &capsule_id, &run_id).await
                {
                    "cancelled"
                } else {
                    "settlement_unknown"
                };
                mark_run_settled(data_dir, now, &run_id, status);
            }
            Err(err) => {
                tracing::warn!("remote model run index write failed: {err}");
                let _ = cancel_unrecorded_run(registry, context, &capsule_id, &run_id).await;
                if held_slot {
                    release_run_slot(data_dir, now, reservation_key);
                }
            }
        }
    } else {
        tracing::warn!("dispatched model create kept its reservation without a trustworthy run id");
    }
    denied("invalid_provider_invocation", "model reply is ambiguous")
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
        remote["capsule_id"].as_str().filter(|value| {
            MODEL_CONSUMER_CAPSULES.contains(value)
                || (operation == "offers_list" && *value == "marketplace")
        }),
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

    // A fresh local request: only the fields the typed contract accepts. The
    // normalizer takes the operation as a parameter and sets `op` itself.
    let mut local = request.clone();
    let local_object = local.as_object_mut().expect("request is an object");
    local_object.remove("remote_model");
    local_object.remove("_runtime_invocation");
    local_object.remove("op");

    let (context, indexed_run, include_local, approved_offer_ids, approved_offer_revision) =
        match operation {
            "offers_list" | "runs_create" => {
                let grant = match read_authority(
                    data_dir,
                    &network,
                    source,
                    grant_id,
                    requester_principal_id,
                )
                .await
                {
                    Ok(grant) => grant,
                    Err(err) => {
                        tracing::info!("remote model authority denied: {err}");
                        return refused_before_dispatch(
                            data_dir,
                            &source_did,
                            request,
                            "denied",
                            "model grant is not active for this requester",
                            false,
                        );
                    }
                };
                if let Err(err) = grant.validate(&source, requester_principal_id, now) {
                    tracing::info!("remote model grant rejected: {err}");
                    return refused_before_dispatch(
                        data_dir,
                        &source_did,
                        request,
                        "denied",
                        "model grant is not active for this requester",
                        false,
                    );
                }
                let include_local = super::gateway_home_system::local_model_offer_is_shared(
                    data_dir,
                    &grant.provider_principal_id,
                );
                (
                    remote_context(
                        &remote_principal_id(&source_did, requester_principal_id),
                        &grant.grant_id,
                    ),
                    None,
                    include_local,
                    grant.approved_offer_ids,
                    grant.approved_offer_revision,
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
                    true,
                    BTreeSet::new(),
                    String::new(),
                )
            }
        };

    let mut normalized =
        match normalize_model_provider_request(operation, &local, &context, capsule_id) {
            Ok(normalized) => normalized,
            Err((_, message)) => {
                tracing::info!("remote model request rejected: {message}");
                return denied(
                    "invalid_provider_invocation",
                    "model request did not match the typed contract",
                );
            }
        };

    // A Share change waits until an admitted create has reached the provider
    // and its run has been recorded or cancelled on this Runtime.
    let share_guard = if operation == "runs_create" {
        Some(model_share_gate().read().await)
    } else {
        None
    };
    if operation == "runs_create" {
        let (offers, revisions) = match registry
            .send_raw("model", &json!({ "op": "offers_list" }))
            .await
        {
            Ok(result) => (
                shareable_offers_for_grant(
                    &result,
                    data_dir,
                    include_local,
                    &approved_offer_ids,
                    &approved_offer_revision,
                ),
                result["data"]["offer_revisions"].clone(),
            ),
            Err(err) => {
                tracing::warn!("remote model offers unavailable: {err}");
                return refused_before_dispatch(
                    data_dir,
                    &source_did,
                    request,
                    "offer_unavailable",
                    "model offers are unavailable on this Runtime",
                    false,
                );
            }
        };
        let offer_id = normalized["offer_id"].as_str().unwrap_or_default();
        if !offers
            .iter()
            .any(|offer| offer["id"].as_str() == Some(offer_id))
        {
            return refused_before_dispatch(
                data_dir,
                &source_did,
                request,
                "offer_unavailable",
                "model offer is not shared through this grant",
                false,
            );
        }
        let Some(execution_revision) = revisions[offer_id].as_str().filter(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) else {
            return refused_before_dispatch(
                data_dir,
                &source_did,
                request,
                "offer_unavailable",
                "model offer revision is unavailable on this Runtime",
                false,
            );
        };
        normalized["expected_execution_binding_hash"] = json!(execution_revision);
    }

    // A create holds its record slot before any model work starts. A retry of
    // an already recorded request is answered from that record before another
    // create. A full index refuses here.
    let request_id = normalized
        .pointer("/runtime_binding/request_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reservation_key =
        run_reservation_key(grant_id, &context.principal_id, capsule_id, &request_id);
    let held_slot = if operation == "runs_create" {
        match reserve_run_slot(
            data_dir,
            now,
            &reservation_key,
            grant_id,
            &context.principal_id,
            capsule_id,
            &request_id,
        ) {
            Ok(RunSlot::Reserved) => true,
            Ok(RunSlot::Existing { run_id, record }) => {
                return answer_retained_create(
                    &registry,
                    data_dir,
                    now,
                    &context,
                    capsule_id,
                    &normalized,
                    (run_id, *record),
                )
                .await;
            }
            Err(err) => {
                tracing::info!("remote model run refused before dispatch: {err}");
                return refused_before_dispatch(
                    data_dir,
                    &source_did,
                    request,
                    "rate_limited",
                    "model run capacity on this Runtime is exhausted",
                    false,
                );
            }
        }
    } else {
        false
    };

    if held_slot {
        #[cfg(test)]
        await_create_race_barrier(data_dir, CreateRacePoint::BeforeDispatch).await;
        if !grant_is_active(data_dir, &network, source, grant_id, requester_principal_id).await {
            let refusal = refused_before_dispatch(
                data_dir,
                &source_did,
                request,
                "denied",
                "model grant is not active for this requester",
                true,
            );
            release_run_slot(data_dir, now, &reservation_key);
            return refusal;
        }
    }

    let result = match registry.send_raw("model", &normalized).await {
        Ok(result) => result,
        Err(err) => {
            if held_slot {
                release_run_slot(data_dir, now, &reservation_key);
            }
            let class = redact_provider_error(&err.to_string());
            tracing::info!("remote model provider error ({class}): {err}");
            return denied(class, "model provider rejected the operation");
        }
    };
    if let Some(message) = provider_status_error(&result) {
        if held_slot {
            release_run_slot(data_dir, now, &reservation_key);
        }
        // The provider journal keeps a run for a bounded time; this Runtime's
        // record outlives it. Only a missing-run reply uses that record. Other
        // provider failures stay recoverable.
        if let Some((run_id, record)) = indexed_run
            .as_ref()
            .filter(|_| matches!(operation, "runs_get" | "runs_events"))
        {
            return recover_missing_run_or_deny(
                data_dir,
                now,
                RetainedRunQuery {
                    operation,
                    run_id,
                    record,
                    after_sequence: normalized["after_sequence"].as_u64().unwrap_or(0),
                },
                &result,
                &message,
            );
        }
        let class = redact_provider_error(&message);
        tracing::info!("remote model provider status error ({class}): {message}");
        return denied(class, "model provider rejected the operation");
    }

    if held_slot {
        #[cfg(test)]
        await_create_race_barrier(data_dir, CreateRacePoint::BeforeCommit).await;
    }

    let create_record = || RemoteModelRunRecord {
        grant_id: grant_id.to_string(),
        source_endpoint_did: source_did.clone(),
        requester_principal_id: requester_principal_id.to_string(),
        remote_principal_id: context.principal_id.clone(),
        capsule_id: capsule_id.to_string(),
        offer_id: normalized["offer_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        operation: normalized["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        request_id: request_id.clone(),
        input_hash: normalized
            .pointer("/runtime_binding/input_hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: now,
        terminal_status: None,
        terminal_at: None,
    };
    let reply = normalize_remote_model_reply(&result);
    if operation == "runs_create" {
        let expected_offer = normalized["offer_id"].as_str().unwrap_or_default();
        let offer_mismatch = reply.as_ref().ok().is_some_and(|reply| {
            reply
                .offer_id
                .as_deref()
                .is_some_and(|got| !expected_offer.is_empty() && got != expected_offer)
        });
        if reply.is_err() || offer_mismatch {
            return reject_dispatched_create(
                &registry,
                data_dir,
                &reservation_key,
                held_slot,
                &context,
                create_record(),
                &result,
            )
            .await;
        }
    }
    let reply = match reply {
        Ok(reply) => reply,
        Err(_) => {
            return denied("invalid_provider_invocation", "model reply is ambiguous");
        }
    };

    let result = match operation {
        "offers_list" => with_offers(
            result.clone(),
            shareable_offers_for_grant(
                &result,
                data_dir,
                include_local,
                &approved_offer_ids,
                &approved_offer_revision,
            ),
        ),
        "runs_create" => {
            if let Some(run_id) = reply.run_id.clone() {
                let mut record = create_record();
                record.terminal_status = reply.terminal_status.clone();
                record.terminal_at = reply.terminal_status.is_some().then_some(now);
                if let Err(err) = commit_run_slot(data_dir, now, &reservation_key, &run_id, record)
                {
                    // The run exists on this Runtime but has no owner record;
                    // settle it now rather than leave it running unaccounted.
                    tracing::warn!("remote model run index write failed: {err}");
                    let _ = cancel_unrecorded_run(&registry, &context, capsule_id, &run_id).await;
                    if held_slot {
                        release_run_slot(data_dir, now, &reservation_key);
                    }
                    return denied(
                        "provider_failure",
                        "model run could not be recorded on this Runtime",
                    );
                }
            } else if held_slot {
                release_run_slot(data_dir, now, &reservation_key);
            }
            result
        }
        _ => {
            if let Some((run_id, record)) = indexed_run {
                if let Some(status) = reply
                    .terminal_status
                    .clone()
                    .filter(|_| record.terminal_at.is_none())
                {
                    let _ = update_run_index(data_dir, now, |index| {
                        if let Some(entry) = index.runs.get_mut(&run_id) {
                            entry.terminal_at = Some(now);
                            entry.terminal_status = Some(status);
                        }
                        Ok(())
                    });
                }
            }
            result
        }
    };
    drop(share_guard);
    if held_slot
        && !grant_is_active(data_dir, &network, source, grant_id, requester_principal_id).await
    {
        let _ = cancel_grant_runs(registry, data_dir, grant_id).await;
    }
    json!({ "ok": true, "result": result })
}

/// Deny a service request as this principal and settle the runs its grant had
/// opened on this Runtime. Three steps, in this order: the principal-scoped
/// denial is saved (or nothing happens), the sweep cancels that grant's open
/// runs, and only then does the delivery result decide the message. A denial
/// by another principal or a failed save never reaches the sweep.
pub(in crate::api) async fn deny_grant_and_settle(
    registry: Option<Arc<ProviderRegistry>>,
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
    discovery_service: Option<
        &crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    >,
    request_id: &str,
) -> anyhow::Result<String> {
    let recorded = {
        let data_dir = data_dir.to_path_buf();
        let context = context.clone();
        let discovery_service = discovery_service.cloned();
        let request_id = request_id.to_string();
        tokio::task::spawn_blocking(move || {
            super::gateway_home_system::deny_home_service_access_request(
                &data_dir,
                &context,
                discovery_service.as_ref(),
                &request_id,
            )
        })
        .await
        .map_err(|err| anyhow::anyhow!(err))??
    };
    let grant_id = model_grant_id(request_id);
    if let Some(registry) = registry {
        match cancel_grant_runs(registry, data_dir, &grant_id).await {
            Ok(cancelled) if !cancelled.is_empty() => tracing::info!(
                "revoked model grant {grant_id}: cancel requested for {} run(s)",
                cancelled.len()
            ),
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("revoked model grant {grant_id}: cancel sweep failed: {err}")
            }
        }
    }
    recorded.into_message()
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
            "run_id": run_id,
            "request_id": format!("revoke:{}", record.request_id),
        });
        let Ok(normalized) =
            normalize_model_provider_request("runs_cancel", &request, &context, &record.capsule_id)
        else {
            continue;
        };
        match registry.send_raw("model", &normalized).await {
            Ok(result) if provider_status_error(&result).is_none() => {
                // A settled cancel closes the record; a later sweep skips it.
                if let Some(status) = run_result_terminal_status(&result) {
                    let now = crate::auth::now_ts();
                    let settled = run_id.clone();
                    let _ = update_run_index(data_dir, now, |index| {
                        if let Some(entry) = index.runs.get_mut(&settled) {
                            entry.terminal_at = Some(now);
                            entry.terminal_status = Some(status);
                        }
                        Ok(())
                    });
                }
                cancelled.push(run_id)
            }
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
            approved_offer_ids: BTreeSet::from(["qwen".into()]),
            approved_offer_revision: "a".repeat(64),
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
        for ids in [
            BTreeSet::new(),
            BTreeSet::from(["qwen".into(), "other".into()]),
        ] {
            let invalid = ModelServiceGrant {
                approved_offer_ids: ids,
                ..grant.clone()
            };
            assert!(invalid.validate(&key, "seed-principal", 1_500).is_err());
        }
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
    fn shareable_offers_include_explicitly_shared_hosted() {
        let result = json!({ "status": "ok", "data": { "offers": [
            { "id": "qwen", "hosted": null },
            {
                "id": "model:openrouter",
                "operation": "text.generate",
                "hosted": { "placement": "hosted", "backend_provider_label": "OpenRouter" },
                "share": { "enabled": true, "terms_ack": "openrouter-5.1-5.2+model", "processor": "OpenRouter", "payer": "this Home", "model": "fixture/model" },
                "key_present": true
            },
            { "id": "model:venice", "hosted": { "placement": "hosted", "backend_provider_label": "Venice" } }
        ] } });
        let ids: Vec<_> = shareable_offers(&result)
            .into_iter()
            .map(|offer| offer["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["qwen", "model:openrouter"]);
        let published = public_shared_offer(shareable_offers(&result)[1].clone());
        assert_eq!(published["id"], "model:openrouter");
        assert!(published.get("share").is_none());
        assert_eq!(published["hosted"]["payer"], "this Home");
        assert_eq!(published["hosted"]["intermediary"], "this Home");
        assert_eq!(published["hosted"]["backend_provider_label"], "OpenRouter");
        assert!(published["hosted"].get("privacy_policy_ref").is_none());
        assert!(published["hosted"].get("terms_ref").is_none());
        let encoded = published.to_string();
        assert!(!encoded.contains("openrouter.ai"));
        assert!(!encoded.contains("api_key"));
        let hosted_only: Vec<_> = shareable_listed_offers(&result, false)
            .into_iter()
            .map(|offer| offer["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(hosted_only, vec!["model:openrouter"]);
    }

    #[test]
    fn remote_reply_keeps_matching_identities_and_nested_output() {
        let result = json!({
            "status": "ok",
            "data": {
                "run_id": "run:sha256:aa",
                "run": { "id": "run:sha256:aa" },
                "offer_id": "qwen-local",
                "status": "completed",
                "terminal": {
                    "status": "completed",
                    "output": { "schema": "elastos.model.output.text/v1", "text": "hello" }
                }
            },
            "run_id": "run:sha256:aa",
            "offer_id": "qwen-local"
        });
        let reply = normalize_remote_model_reply(&result).expect("matching identities");
        assert_eq!(reply.run_id.as_deref(), Some("run:sha256:aa"));
        assert_eq!(reply.offer_id.as_deref(), Some("qwen-local"));
        assert_eq!(reply.terminal_status.as_deref(), Some("completed"));
        assert_eq!(
            result["data"]["terminal"]["output"]["text"], "hello",
            "normalization must leave nested output on the original reply"
        );
    }

    #[test]
    fn remote_reply_rejects_disagreeing_run_ids_and_offer_lists() {
        assert!(normalize_remote_model_reply(&json!({
            "data": { "run_id": "run:a" },
            "run_id": "run:b"
        }))
        .is_err());
        assert!(normalize_remote_model_reply(&json!({
            "data": { "offers": [{ "id": "qwen" }] },
            "offers": [{ "id": "other" }]
        }))
        .is_err());
    }

    #[test]
    fn settlement_events_page_emits_one_terminal_event() {
        let record = RemoteModelRunRecord {
            grant_id: "g".into(),
            source_endpoint_did: "did:key:z6Mksource".into(),
            requester_principal_id: "p".into(),
            remote_principal_id: remote_principal_id("did:key:z6Mksource", "p"),
            capsule_id: "assistant".into(),
            offer_id: "qwen".into(),
            operation: "text.generate".into(),
            request_id: "req-1".into(),
            input_hash: "sha256:aa".into(),
            created_at: 100,
            terminal_at: Some(150),
            terminal_status: Some("completed".into()),
        };
        let page = record.settlement_events_page("run:1", 0);
        assert_eq!(page["data"]["events"][0]["kind"], "completed");
        assert_eq!(page["data"]["events"][0]["data"]["output_retained"], false);
        assert_eq!(page["data"]["events"][0]["sequence"], 1);
        assert_eq!(page["data"]["events"][0]["terminal"], true);
        assert_eq!(page["data"]["next_cursor"], 1);
        assert_eq!(page["data"]["output_retained"], false);
        let mid = record.settlement_events_page("run:1", 2);
        assert_eq!(mid["data"]["events"][0]["sequence"], 3);
        assert_eq!(mid["data"]["next_cursor"], 3);
        let high = record.settlement_events_page("run:1", 7);
        assert_eq!(high["data"]["events"][0]["sequence"], 8);
        assert_eq!(high["data"]["next_cursor"], 8);
        let unknown = RemoteModelRunRecord {
            terminal_at: None,
            terminal_status: None,
            ..record
        };
        assert_eq!(
            unknown.settlement_events_page("run:2", 0)["data"]["events"][0]["kind"],
            "settlement_unknown"
        );
    }

    #[test]
    fn provider_errors_cross_carrier_only_as_bounded_classes() {
        assert_eq!(
            redact_provider_error("run concurrency limit reached for offer"),
            "rate_limited"
        );
        assert_eq!(
            redact_provider_error("unknown offer 'x' at /Users/owner/models"),
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
            operation: "text.generate".into(),
            request_id: "req-1".into(),
            input_hash: String::new(),
            created_at: 100,
            terminal_at: None,
            terminal_status: None,
        };
        // A record written before `operation` and `terminal_status` existed
        // still parses, and a pruned run then reports `settlement_unknown`.
        let legacy: RemoteModelRunRecord = serde_json::from_value(json!({
            "grant_id": "g", "source_endpoint_did": "did:key:z6Mksource",
            "requester_principal_id": "p", "remote_principal_id": "remote:abc",
            "capsule_id": "assistant", "offer_id": "qwen", "request_id": "req-0",
            "created_at": 1,
        }))
        .expect("legacy record parses");
        assert_eq!(
            legacy.settlement_view("run:sha256:old")["data"]["status"],
            "settlement_unknown"
        );
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
        // The open record (created at 100) leaves once the same window has
        // passed since its creation; it cannot still be running.
        update_run_index(dir.path(), 100 + MODEL_RUN_RETENTION_SECS + 1, |_| Ok(())).unwrap();
        assert!(remote_model_runs_for_grant(dir.path(), "g")
            .unwrap()
            .is_empty());
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
