use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context as _};
use elastos_common::{AffordanceApprovalMode, AffordanceRisk, CapsuleAffordanceDescriptor};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::api::gateway::HomeLaunchTokenContext;
use crate::esp_binding::{esp_request_binding, EspRequestBinding};

mod storage;
use storage::Inventory;

const SCHEMA: &str = "elastos.model.preparation-inventory/v1";
const RESOURCE: &str = "elastos://capsules/*";
const MAX_RECORDS: usize = 64;
const RESERVATION_SECONDS: u64 = 3600;
const MAX_PACKAGE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PreparationState {
    Reserved,
    Preparing,
    Verifying,
    AdmissionPending,
    Admitted,
    Failed,
    Uncertain,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct PreparationRecord {
    operation_id: String,
    request_binding: EspRequestBinding,
    catalog_head_cid: String,
    package_cid: String,
    state: PreparationState,
    total_bytes: u64,
    reserved_bytes: u64,
    index_bytes: u64,
    completed_bytes: u64,
    cancel_requested: bool,
    admission_id: String,
    created_at: u64,
    expires_at: u64,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct PreparationInventory {
    schema: String,
    records: Vec<PreparationRecord>,
}

impl Default for PreparationInventory {
    fn default() -> Self {
        Self {
            schema: SCHEMA.into(),
            records: Vec::new(),
        }
    }
}

// The invocation owner supplies this context and its resolved manifest method.
// This private boundary neither parses a caller token nor grants inference.
pub(in crate::api) struct PreparationCaller<'a> {
    pub(in crate::api) context: &'a HomeLaunchTokenContext,
    pub(in crate::api) capsule: &'a str,
    pub(in crate::api) interface: &'a str,
    pub(in crate::api) method: &'a CapsuleAffordanceDescriptor,
}

fn bounded_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
}

fn context_is_bounded(context: &HomeLaunchTokenContext) -> bool {
    bounded_id(&context.principal_id, 160)
        && bounded_id(&context.session_id, 160)
        && bounded_id(&context.grant_id, 160)
        && context
            .proof_binding_id
            .as_ref()
            .is_none_or(|id| bounded_id(id, 160))
}

fn authorize(caller: &PreparationCaller<'_>, operation: &str) -> anyhow::Result<()> {
    let risk = if operation == "status" {
        AffordanceRisk::Read
    } else {
        AffordanceRisk::Write
    };
    ensure!(
        context_is_bounded(caller.context)
            && bounded_id(caller.capsule, 128)
            && bounded_id(caller.interface, 128),
        "invalid preparation caller"
    );
    ensure!(
        caller.method.id == format!("content.{operation}")
            && caller.method.operation.as_deref() == Some(operation)
            && caller.method.resource.as_deref() == Some(RESOURCE)
            && caller.method.risk == risk
            && caller.method.approval == AffordanceApprovalMode::RuntimePolicy,
        "preparation operation is not authorized by the resolved method"
    );
    Ok(())
}

fn now() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn binding(record: &PreparationRecord) -> EspRequestBinding {
    esp_request_binding(
        &record.request_binding.request_id,
        &record.request_binding.principal,
        &record.request_binding.capsule,
        record.request_binding.interface.as_deref(),
        "content.use",
        [RESOURCE.to_owned()],
        &serde_json::json!({
            "cid": record.package_cid,
        }),
    )
}

fn operation_id(record: &PreparationRecord) -> anyhow::Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &serde_json::json!({
            "request_binding": record.request_binding,
            "catalog_head_cid": record.catalog_head_cid,
            "total_bytes": record.total_bytes,
            "created_at": record.created_at,
            "expires_at": record.expires_at,
        }),
    )?)))
}

fn canonical_cid(value: &str, codec: u64) -> bool {
    value.len() <= 128
        && cid::Cid::try_from(value).is_ok_and(|cid| {
            cid.version() == cid::Version::V1
                && cid.to_string() == value
                && cid.codec() == codec
                && cid.hash().code() == 0x12
                && cid.hash().digest().len() == 32
        })
}

impl PreparationInventory {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.schema == SCHEMA && self.records.len() <= MAX_RECORDS,
            "invalid preparation inventory bounds"
        );
        let mut identities = BTreeSet::new();
        let mut active = 0;
        for record in &self.records {
            ensure!(
                bounded_id(&record.request_binding.principal, 160)
                    && bounded_id(&record.request_binding.capsule, 128)
                    && record
                        .request_binding
                        .interface
                        .as_deref()
                        .is_some_and(|id| bounded_id(id, 128))
                    && bounded_id(&record.request_binding.request_id, 160)
                    && canonical_cid(&record.package_cid, 0x70)
                    && canonical_cid(&record.catalog_head_cid, 0x55)
                    && record.total_bytes > 0
                    && record.total_bytes <= MAX_PACKAGE_BYTES
                    && record.created_at.checked_add(RESERVATION_SECONDS)
                        == Some(record.expires_at)
                    && record.request_binding == binding(record)
                    && record.operation_id == operation_id(record)?,
                "invalid preparation record"
            );
            ensure!(
                identities.insert((
                    &record.request_binding.principal,
                    &record.request_binding.capsule,
                    &record.request_binding.request_id,
                )),
                "duplicate preparation request"
            );
            ensure!(
                record.index_bytes <= 65536 && record.completed_bytes <= record.total_bytes,
                "invalid preparation progress"
            );
            if matches!(
                record.state,
                PreparationState::Admitted | PreparationState::AdmissionPending
            ) {
                ensure!(
                    record.index_bytes > 0 && record.completed_bytes == record.total_bytes,
                    "incomplete admission record"
                );
            }
            let shared = record.admission_id != record.operation_id;
            if shared {
                ensure!(
                    self.records
                        .iter()
                        .any(|r| r.operation_id == record.admission_id
                            && r.admission_id == r.operation_id
                            && r.package_cid == record.package_cid
                            && r.state == PreparationState::Admitted),
                    "invalid shared admission"
                );
            }
            let expected = if record.state == PreparationState::Reserved {
                active += 1;
                if shared {
                    0
                } else {
                    preparation_charge(record.total_bytes)?
                }
            } else if record.state == PreparationState::Admitted {
                if shared {
                    0
                } else {
                    preparation_charge(record.total_bytes)?
                }
            } else if record.active() {
                active += 1;
                if shared {
                    0
                } else {
                    preparation_charge(record.total_bytes)?
                }
            } else {
                0
            };
            ensure!(
                record.reserved_bytes == expected,
                "invalid preparation reservation accounting"
            );
        }
        ensure!(active <= 1, "multiple active preparations");
        Ok(())
    }

    fn expire(&mut self, now: u64) -> bool {
        // Reserved records have not dispatched provider work. Active work must
        // settle and drain before expiry can release its byte charge.
        let mut changed = false;
        for record in &mut self.records {
            if record.state == PreparationState::Reserved && now >= record.expires_at {
                record.state = PreparationState::Expired;
                record.reserved_bytes = 0;
                changed = true;
            }
        }
        changed
    }
}

fn reserve(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    request_id: &str,
    cid: &str,
) -> anyhow::Result<PreparationRecord> {
    reserve_at(data_dir, caller, request_id, cid, now()?)
}

fn reserve_at(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    request_id: &str,
    cid: &str,
    now: u64,
) -> anyhow::Result<PreparationRecord> {
    authorize(caller, "use")?;
    ensure!(
        bounded_id(request_id, 160) && canonical_cid(cid, 0x70),
        "invalid preparation request identity"
    );
    // Resolve all authority before creating the inventory directory or lock.
    let config: crate::setup::ComponentsManifest = serde_json::from_slice(
        &super::read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?,
    )?;
    let trust = config.model_catalog.context("model catalog unavailable")?;
    let grant = trust
        .local_use
        .as_ref()
        .context("local model use is not granted")?;
    let entries = super::verify_model_catalog(
        &trust,
        &super::read_model_catalog_file(
            data_dir,
            super::MODEL_CATALOG_FILE,
            super::MAX_MODEL_CATALOG_BYTES,
        )?,
        now,
    )?;
    let entry = entries
        .iter()
        .find(|entry| entry.cid == cid)
        .context("model is not in the verified catalog")?;
    let request_binding = esp_request_binding(
        request_id,
        &caller.context.principal_id,
        caller.capsule,
        Some(caller.interface),
        &caller.method.id,
        [RESOURCE.to_owned()],
        &serde_json::json!({"cid": cid}),
    );
    let memory_bytes = u64::from(
        entry
            .manifest
            .model_content
            .as_ref()
            .context("passive model required")?
            .minimum_memory_mb,
    )
    .checked_mul(1024 * 1024)
    .context("model memory overflow")?;
    ensure!(
        memory_bytes <= grant.max_model_memory_bytes,
        "model exceeds local memory budget"
    );
    ensure!(
        entry.size_bytes <= MAX_PACKAGE_BYTES - 65536,
        "closure exceeds staging limit"
    );
    let full_charge = preparation_charge(entry.size_bytes)?;
    ensure!(
        full_charge <= grant.max_cache_bytes,
        "model exceeds local cache budget"
    );
    let inventory = Inventory::open(data_dir, true)?;
    let mut state = inventory.load()?;
    let expired = state.expire(now);
    if let Some(existing) = state.records.iter().find(|record| {
        record.request_binding.principal == caller.context.principal_id
            && record.request_binding.capsule == caller.capsule
            && record.request_binding.request_id == request_id
    }) {
        ensure!(
            existing.request_binding == request_binding
                && existing.catalog_head_cid == trust.head_cid
                && existing.total_bytes == entry.size_bytes,
            "preparation request identity changed"
        );
        if expired {
            inventory.save(&state)?;
        }
        return Ok(existing.clone());
    }
    ensure!(
        state.records.len() < MAX_RECORDS,
        "preparation inventory is full"
    );
    ensure!(
        !state
            .records
            .iter()
            .any(|record| record.state == PreparationState::Reserved || record.active()),
        "a preparation is already active"
    );
    let reuse = state
        .records
        .iter()
        .find(|r| {
            r.package_cid == cid
                && r.state == PreparationState::Admitted
                && r.admission_id == r.operation_id
        })
        .map(|r| r.operation_id.clone());
    let initial_charge = if reuse.is_some() { 0 } else { full_charge };
    let reserved_bytes = state
        .records
        .iter()
        .try_fold(initial_charge, |total, record| {
            total
                .checked_add(record.reserved_bytes)
                .context("preparation byte accounting overflow")
        })?;
    ensure!(
        reserved_bytes <= grant.max_cache_bytes,
        "local cache budget is reserved"
    );
    // Existing admissions already occupy disk. Charge only this new reservation
    // again against observed free space, while quota above counts every artifact.
    inventory.require_space(initial_charge)?;
    let mut record = PreparationRecord {
        operation_id: String::new(),
        request_binding,
        catalog_head_cid: trust.head_cid,
        package_cid: cid.into(),
        state: PreparationState::Reserved,
        total_bytes: entry.size_bytes,
        reserved_bytes: initial_charge,
        index_bytes: 0,
        completed_bytes: 0,
        cancel_requested: false,
        admission_id: String::new(),
        created_at: now,
        expires_at: now
            .checked_add(RESERVATION_SECONDS)
            .context("preparation expiry overflow")?,
    };
    record.operation_id = operation_id(&record)?;
    record.admission_id = reuse.unwrap_or_else(|| record.operation_id.clone());
    state.records.push(record.clone());
    inventory.save(&state)?;
    Ok(record)
}

fn status(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    operation_id: &str,
) -> anyhow::Result<PreparationRecord> {
    manage(data_dir, caller, operation_id, false, now()?)
}

fn cancel(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    operation_id: &str,
) -> anyhow::Result<PreparationRecord> {
    manage(data_dir, caller, operation_id, true, now()?)
}

fn manage(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    operation_id: &str,
    cancel: bool,
    now: u64,
) -> anyhow::Result<PreparationRecord> {
    authorize(caller, if cancel { "cancel" } else { "status" })?;
    ensure!(
        operation_id.len() == 64
            && operation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid preparation operation"
    );
    let inventory = Inventory::open(data_dir, false)?;
    let mut state = inventory.load()?;
    let index = state
        .records
        .iter()
        .position(|record| {
            record.operation_id == operation_id
                && record.request_binding.principal == caller.context.principal_id
        })
        .context("preparation operation unavailable")?;
    let mut changed = state.expire(now);
    let record = &mut state.records[index];
    if cancel && record.state == PreparationState::Reserved {
        record.state = PreparationState::Cancelled;
        record.reserved_bytes = 0;
        changed = true;
    } else if cancel && record.active() {
        record.cancel_requested = true;
        changed = true;
    }
    let result = record.clone();
    if changed {
        inventory.save(&state)?;
    }
    Ok(result)
}

impl PreparationRecord {
    fn active(&self) -> bool {
        matches!(
            self.state,
            PreparationState::Preparing
                | PreparationState::Verifying
                | PreparationState::AdmissionPending
                | PreparationState::Uncertain
        )
    }

    fn projection(&self) -> serde_json::Value {
        serde_json::json!({"operation_id":self.operation_id, "cid":self.package_cid,
            "state":self.state, "total_bytes":self.total_bytes,
            "completed_bytes":self.completed_bytes, "cancel_requested":self.cancel_requested,
            "admitted":self.state == PreparationState::Admitted, "inference_ready":false})
    }
}

// One payload on staging plus index/allocator overhead, and a conservative
// two-payload backend growth charge. Current filesystem facts are also checked.
fn preparation_charge(bytes: u64) -> anyhow::Result<u64> {
    bytes
        .checked_mul(3)
        .and_then(|n| n.checked_add(8 * 1024 * 1024 + 3 * 65536))
        .context("preparation charge overflow")
}

fn staging_charge(bytes: u64) -> anyhow::Result<u64> {
    bytes
        .checked_add(4 * 1024 * 1024 + 65536)
        .context("staging charge overflow")
}

pub(in crate::api) type Revalidate = Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>;

/// The invocation route retains this single inventory worker. Status/cancel
/// use short snapshot locks while the task retains exclusive worker ownership.
#[derive(Default)]
pub(in crate::api) struct PreparationOwner {
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    stopping: Arc<AtomicBool>,
}

impl Drop for PreparationOwner {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Do not abort a provider dispatch. The task drains it before cleanup;
        // abrupt process/runtime exit preserves active accounting for reconciliation.
    }
}

impl PreparationOwner {
    pub(in crate::api) fn invoke(
        &self,
        data_dir: &Path,
        registry: Option<Arc<elastos_runtime::provider::ProviderRegistry>>,
        caller: PreparationCaller<'_>,
        request_id: &str,
        input: &serde_json::Value,
        revalidate: Revalidate,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Use {
            cid: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Manage {
            operation_id: String,
        }
        revalidate()?;
        let record = match caller.method.operation.as_deref() {
            Some("status") => {
                let input: Manage = serde_json::from_value(input.clone())?;
                status(data_dir, &caller, &input.operation_id)?
            }
            Some("cancel") => {
                let input: Manage = serde_json::from_value(input.clone())?;
                cancel(data_dir, &caller, &input.operation_id)?
            }
            Some("use") => {
                let input: Use = serde_json::from_value(input.clone())?;
                registry
                    .as_ref()
                    .context("preparation backend unavailable")?;
                reserve(data_dir, &caller, request_id, &input.cid)?
            }
            _ => anyhow::bail!("preparation method unavailable"),
        };
        let starting = record.state == PreparationState::Reserved
            && caller.method.operation.as_deref() == Some("use");
        if !record.active() && !starting {
            return Ok(record.projection());
        }
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("preparation owner unavailable"))?;
        if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Ok(record.projection());
        }
        let registry = registry.context("preparation backend unavailable")?;
        let inventory = Inventory::open(data_dir, false)?;
        let worker_lock = inventory.worker_lock()?;
        let mut snapshot = inventory.load()?;
        if starting {
            current_entry(data_dir, &record)?;
        }
        let shared = snapshot
            .records
            .iter()
            .find(|r| {
                r.operation_id == record.admission_id && r.operation_id != record.operation_id
            })
            .cloned();
        let current = snapshot
            .records
            .iter_mut()
            .find(|r| r.operation_id == record.operation_id)
            .context("preparation disappeared")?;
        let starting = starting && current.state == PreparationState::Reserved;
        if !starting && !current.active() {
            return Ok(current.projection());
        }
        if starting {
            current.state = PreparationState::Preparing;
            if let Some(shared) = shared {
                current.index_bytes = shared.index_bytes;
                current.completed_bytes = shared.completed_bytes;
            }
        }
        let current = current.clone();
        if starting {
            require_cache_budget(data_dir, &snapshot)?;
            inventory.require_space(if current.admission_id == current.operation_id {
                staging_charge(current.total_bytes)?
            } else {
                0
            })?;
            inventory.save(&snapshot)?;
        }
        drop(inventory);
        let data = data_dir.to_path_buf();
        let stopping = self.stopping.clone();
        let operation = current.operation_id.clone();
        *worker = Some(tokio::spawn(async move {
            let _worker_lock = worker_lock;
            let result = if starting {
                prepare(&data, &registry, &operation, &stopping, &revalidate).await
            } else {
                reconcile(&data, &registry, &operation, &stopping, &revalidate).await
            };
            if let Err(error) = result {
                tracing::debug!(error = ?error, "private model preparation stopped");
                // The serialized local barrier is required even after transport
                // failure. Failed drain preserves all staged bytes and charges.
                let drained = registry.prepare_local_ipfs_backend().await.is_ok();
                if let Err(error) = settle_failure(&data, &operation, drained) {
                    tracing::debug!(error = ?error, "private model preparation settlement uncertain");
                }
            }
        }));
        Ok(current.projection())
    }
}

fn require_cache_budget(data_dir: &Path, state: &PreparationInventory) -> anyhow::Result<()> {
    let config: crate::setup::ComponentsManifest = serde_json::from_slice(
        &super::read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?,
    )?;
    let grant = config
        .model_catalog
        .and_then(|c| c.local_use)
        .context("local use unavailable")?;
    let total = state
        .records
        .iter()
        .try_fold(0u64, |sum, r| sum.checked_add(r.reserved_bytes))
        .context("cache accounting overflow")?;
    ensure!(total <= grant.max_cache_bytes, "cache budget unavailable");
    Ok(())
}

fn current_entry(
    data_dir: &Path,
    record: &PreparationRecord,
) -> anyhow::Result<super::VerifiedModelCatalogEntry> {
    let config: crate::setup::ComponentsManifest = serde_json::from_slice(
        &super::read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?,
    )?;
    let trust = config.model_catalog.context("catalog unavailable")?;
    ensure!(
        trust.head_cid == record.catalog_head_cid,
        "catalog changed during preparation"
    );
    let grant = trust.local_use.as_ref().context("local use revoked")?;
    let entry = super::verify_model_catalog(
        &trust,
        &super::read_model_catalog_file(
            data_dir,
            super::MODEL_CATALOG_FILE,
            super::MAX_MODEL_CATALOG_BYTES,
        )?,
        now()?,
    )?
    .into_iter()
    .find(|e| e.cid == record.package_cid)
    .context("selected model unavailable")?;
    let model = entry
        .manifest
        .model_content
        .as_ref()
        .context("model metadata unavailable")?;
    ensure!(
        entry.size_bytes == record.total_bytes
            && preparation_charge(entry.size_bytes)? <= grant.max_cache_bytes
            && u64::from(model.minimum_memory_mb) * 1024 * 1024 <= grant.max_model_memory_bytes,
        "model policy changed"
    );
    Ok(entry)
}

fn load_operation(data_dir: &Path, id: &str) -> anyhow::Result<PreparationRecord> {
    let inventory = Inventory::open(data_dir, false)?;
    inventory
        .load()?
        .records
        .into_iter()
        .find(|r| r.operation_id == id)
        .context("preparation unavailable")
}

fn update_operation(
    data_dir: &Path,
    id: &str,
    update: impl FnOnce(&mut PreparationRecord),
) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    let mut state = inventory.load()?;
    update(
        state
            .records
            .iter_mut()
            .find(|r| r.operation_id == id)
            .context("preparation unavailable")?,
    );
    inventory.save(&state)
}

fn require_active(
    data_dir: &Path,
    id: &str,
    stop: &AtomicBool,
    revalidate: &Revalidate,
) -> anyhow::Result<PreparationRecord> {
    revalidate()?;
    let record = load_operation(data_dir, id)?;
    ensure!(
        record.active()
            && !record.cancel_requested
            && !stop.load(Ordering::Acquire)
            && now()? < record.expires_at,
        "preparation stopped"
    );
    current_entry(data_dir, &record)?;
    Ok(record)
}

async fn require_capacity(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    record: &PreparationRecord,
) -> anyhow::Result<()> {
    let staging = staging_charge(record.total_bytes)?;
    let backend = preparation_charge(record.total_bytes)?
        .checked_sub(staging)
        .context("capacity charge overflow")?;
    let observed = registry.check_local_ipfs_capacity(backend).await?;
    let inventory = Inventory::open(data_dir, false)?;
    require_cache_budget(data_dir, &inventory.load()?)?;
    let outstanding_stage = staging
        .checked_sub(record.completed_bytes + record.index_bytes)
        .context("staging accounting overflow")?;
    let required = if inventory.volume()? == observed.volume_id {
        outstanding_stage
            .checked_add(backend)
            .context("shared volume charge overflow")?
    } else {
        outstanding_stage
    };
    inventory.require_space(required)
}

async fn prepare(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    id: &str,
    stop: &AtomicBool,
    revalidate: &Revalidate,
) -> anyhow::Result<()> {
    let record = require_active(data_dir, id, stop, revalidate)?;
    registry.prepare_local_ipfs_backend().await?;
    require_active(data_dir, id, stop, revalidate)?;
    let entry = current_entry(data_dir, &record)?;
    let closure = crate::content::parse_content_object_manifest(
        &entry.cid,
        &serde_json::to_vec(&entry.object_manifest)?,
    )?;
    if record.index_bytes > 0 && record.completed_bytes == record.total_bytes {
        return finish_admission(data_dir, registry, id, &closure, stop, revalidate, false).await;
    }
    if record.index_bytes > 0 || record.completed_bytes > 0 {
        anyhow::bail!("interrupted partial preparation requires settled cleanup");
    }
    require_capacity(data_dir, registry, &record).await?;
    require_active(data_dir, id, stop, revalidate)?;
    let index =
        crate::content::fetch_model_part(registry, &entry.cid, "_elastos_object.json", None)
            .await?;
    require_active(data_dir, id, stop, revalidate)?;
    let object: serde_json::Value = serde_json::from_slice(&index)?;
    ensure!(
        object == entry.object_manifest,
        "fetched index differs from signed catalog"
    );
    let stage = Inventory::open(data_dir, false)?.stage(true)?;
    let mut file = stage.create_file("_elastos_object.json")?;
    file.write_all(&index)?;
    file.sync_all()?;
    update_operation(data_dir, id, |record| {
        record.index_bytes = index.len() as u64
    })?;
    for expected in &closure.files {
        let mut file = stage.create_file(&expected.path)?;
        let mut digest = Sha256::new();
        let mut offset = 0u64;
        while offset < expected.size {
            let record = require_active(data_dir, id, stop, revalidate)?;
            require_capacity(data_dir, registry, &record).await?;
            require_active(data_dir, id, stop, revalidate)?;
            let length = (expected.size - offset).min(65536);
            let bytes = crate::content::fetch_model_part(
                registry,
                &entry.cid,
                &expected.path,
                Some((offset, length)),
            )
            .await?;
            require_active(data_dir, id, stop, revalidate)?;
            if expected.path == entry.manifest.entrypoint && offset == 0 {
                ensure!(
                    bytes.len() >= 8
                        && &bytes[..4] == b"GGUF"
                        && matches!(u32::from_le_bytes(bytes[4..8].try_into()?), 2 | 3),
                    "invalid GGUF header"
                );
            }
            stage.check_file(&expected.path, &file, offset)?;
            file.write_all(&bytes)?;
            digest.update(&bytes);
            offset += length;
            update_operation(data_dir, id, |record| record.completed_bytes += length)?;
        }
        file.sync_all()?;
        ensure!(
            hex::encode(digest.finalize()) == expected.sha256,
            "model file hash mismatch"
        );
    }
    update_operation(data_dir, id, |record| {
        record.state = PreparationState::Verifying
    })?;
    finish_admission(data_dir, registry, id, &closure, stop, revalidate, false).await
}

async fn reconcile(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    id: &str,
    stop: &AtomicBool,
    revalidate: &Revalidate,
) -> anyhow::Result<()> {
    // A restarted owner first drains the same local provider before observing
    // the rename or removing staging. Status/cancel never restart a transfer.
    registry.prepare_local_ipfs_backend().await?;
    let record = load_operation(data_dir, id)?;
    let renamed = record.admission_id == id
        && match Inventory::open(data_dir, false)?.admitted(id) {
            Ok(_) => true,
            Err(err) if storage::missing(&err) => false,
            Err(err) => return Err(err),
        };
    if renamed {
        let entry = current_entry(data_dir, &record)?;
        let closure = crate::content::parse_content_object_manifest(
            &entry.cid,
            &serde_json::to_vec(&entry.object_manifest)?,
        )?;
        finish_admission(data_dir, registry, id, &closure, stop, revalidate, true).await
    } else {
        settle_failure(data_dir, id, true)
    }
}

fn require_admission(
    data_dir: &Path,
    id: &str,
    stop: &AtomicBool,
    revalidate: &Revalidate,
    reconcile_existing: bool,
) -> anyhow::Result<PreparationRecord> {
    if !reconcile_existing {
        return require_active(data_dir, id, stop, revalidate);
    }
    revalidate()?;
    let record = load_operation(data_dir, id)?;
    ensure!(
        record.active() && record.admission_id == id && !stop.load(Ordering::Acquire),
        "admission reconciliation stopped"
    );
    current_entry(data_dir, &record)?;
    // Expiry/cancel forbid a new rename, not a receipt for an exact artifact
    // already renamed by this operation. Current caller and catalog still bind it.
    Ok(record)
}

async fn finish_admission(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    id: &str,
    closure: &crate::content::ContentObjectManifest,
    stop: &AtomicBool,
    revalidate: &Revalidate,
    reconcile_existing: bool,
) -> anyhow::Result<()> {
    let record = require_admission(data_dir, id, stop, revalidate, reconcile_existing)?;
    let (stage, renamed) = {
        let inventory = Inventory::open(data_dir, false)?;
        match inventory.admitted(&record.admission_id) {
            Ok(stage) => (stage, true),
            Err(err) if storage::missing(&err) && record.admission_id == id => {
                (inventory.stage(false)?, false)
            }
            Err(err) => return Err(err),
        }
    };
    ensure!(
        !reconcile_existing || renamed,
        "admission rename is not present"
    );
    let entry = current_entry(data_dir, &record)?;
    verify_package_identity(registry, &stage, &record, &entry, closure).await?;
    require_admission(data_dir, id, stop, revalidate, reconcile_existing)?;
    stage.check()?;
    let inventory = Inventory::open(data_dir, false)?;
    let mut snapshot = inventory.load()?;
    let current = snapshot
        .records
        .iter_mut()
        .find(|r| r.operation_id == id)
        .context("preparation unavailable")?;
    ensure!(
        reconcile_existing || !current.cancel_requested,
        "preparation cancelled before admission"
    );
    current.state = PreparationState::AdmissionPending;
    inventory.save(&snapshot)?;
    if !renamed {
        inventory.admit(id)?;
    }
    snapshot
        .records
        .iter_mut()
        .find(|r| r.operation_id == id)
        .context("preparation unavailable")?
        .state = PreparationState::Admitted;
    inventory.save(&snapshot)
}

async fn verify_package_identity(
    registry: &elastos_runtime::provider::ProviderRegistry,
    stage: &storage::Stage,
    record: &PreparationRecord,
    entry: &super::VerifiedModelCatalogEntry,
    closure: &crate::content::ContentObjectManifest,
) -> anyhow::Result<()> {
    let index = stage.read_index()?;
    ensure!(
        index.len() as u64 == record.index_bytes
            && serde_json::from_slice::<serde_json::Value>(&index)? == entry.object_manifest,
        "stored index differs from signed catalog"
    );
    let mut files: Vec<_> = closure
        .files
        .iter()
        .map(|f| (f.path.clone(), f.size))
        .collect();
    files.push(("_elastos_object.json".into(), record.index_bytes));
    files.sort();
    let actual = registry
        .hash_local_ipfs_directory(&stage.path, &files, record.total_bytes + record.index_bytes)
        .await?;
    ensure!(
        actual == record.package_cid,
        "independent package CID mismatch"
    );
    stage.check()
}

fn local_model_startup_profile(platform: &str) -> anyhow::Result<serde_json::Value> {
    ensure!(
        platform == "darwin-arm64",
        "admitted model host profile is unavailable"
    );
    // Runtime-owned profile from the verified local Qwen/engine proof. Catalog
    // metadata cannot tune execution. Other hosts require their own proof.
    Ok(serde_json::json!({
        "context_size":4096, "parallel":1, "threads":8, "batch_threads":8,
        "gpu_layers":99, "health_timeout_ms":120000,
        "shutdown_timeout_ms":5000, "enable_thinking":false
    }))
}

/// Compose the existing model-provider Init at Runtime startup. This function
/// has no capsule route; public inference still uses the existing model grant.
pub async fn append_admitted_model_startup_offers(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    config: &mut elastos_runtime::provider::BridgeProviderConfig,
) -> anyhow::Result<()> {
    // A Home without preparation inventory keeps its operator offers unchanged.
    match std::fs::symlink_metadata(data_dir.join("model-preparation")) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
        Ok(_) => {}
    }
    let (snapshot, _worker) = {
        let inventory = Inventory::open(data_dir, false)?;
        (inventory.load()?, inventory.worker_lock()?)
    };
    if !snapshot
        .records
        .iter()
        .any(|r| r.state == PreparationState::Admitted)
    {
        return Ok(());
    }
    let manifest_bytes =
        super::read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?;
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(&manifest_bytes)?;
    let Some(trust) = manifest.model_catalog.as_ref() else {
        return Ok(());
    };
    let entries = super::model_catalog_entries(data_dir)?.context("model catalog unavailable")?;
    let mut offers = config.extra["offers"]
        .as_array()
        .context("model startup offers unavailable")?
        .clone();
    for entry in entries {
        // Aliases authorize reuse of one artifact; they do not create duplicate
        // offers or transfer ownership between preparation request records.
        let Some(record) = snapshot.records.iter().find(|r| {
            r.state == PreparationState::Admitted
                && r.package_cid == entry.cid
                && r.catalog_head_cid == trust.head_cid
        }) else {
            continue;
        };
        let entry = current_entry(data_dir, record)?;
        require_cache_budget(data_dir, &snapshot)?;
        let settings = local_model_startup_profile(&crate::setup::detect_platform())?;
        let engine = crate::setup::verified_local_model_engine(data_dir, &manifest)?;
        let stage = Inventory::open(data_dir, false)?.admitted(&record.admission_id)?;
        let closure = crate::content::parse_content_object_manifest(
            &entry.cid,
            &serde_json::to_vec(&entry.object_manifest)?,
        )?;
        for file in &closure.files {
            stage.verify_model_file(file, file.path == entry.manifest.entrypoint)?;
        }
        verify_package_identity(registry, &stage, record, &entry, &closure).await?;
        current_entry(data_dir, record)?;
        ensure!(
            super::read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?
                == manifest_bytes
                && crate::setup::verified_local_model_engine(data_dir, &manifest)? == engine,
            "model startup engine or policy changed"
        );
        let current = Inventory::open(data_dir, false)?.load()?;
        ensure!(
            current == snapshot,
            "model admission changed during startup"
        );
        let weights = closure
            .files
            .iter()
            .find(|f| f.path == entry.manifest.entrypoint)
            .context("model entrypoint is unavailable")?;
        let identity = serde_json::json!({
            "schema":"elastos.model.admitted-offer/v1",
            "cid":entry.cid, "entrypoint":weights.path, "weights_sha256":weights.sha256,
            "engine_receipt_sha256":engine.receipt_sha256, "engine_sha256":engine.sha256,
        });
        let id = format!(
            "model:{}",
            hex::encode(Sha256::digest(serde_json::to_vec(&identity)?))
        );
        ensure!(
            offers.len() < 64
                && !offers
                    .iter()
                    .any(|offer| offer["id"].as_str() == Some(id.as_str())),
            "admitted model offer conflicts with operator configuration"
        );
        // The provider's execution binding includes this CID-bound offer ID,
        // artifact digests and policy. Paths and request aliases are not identity.
        offers.push(serde_json::json!({
            "id":id, "title":entry.manifest.name, "operation":"text.generate",
            "input_modalities":["text/plain"], "output_modalities":["text/plain"],
            "enabled":true,
            "policy":{
                "concurrency_limit":1, "input_bytes_limit":32768,
                "inline_output_bytes_limit":65536, "event_bytes_limit":4096,
                "runtime_ms_limit":120000, "retention_secs":3600,
                "cancel_settlement_timeout_ms":15000
            },
            "adapter":{
                "kind":"local_llama_cpp_text",
                "engine":{"path":engine.path, "sha256":engine.sha256},
                "model":{"path":stage.path.join(&weights.path), "sha256":format!("sha256:{}",weights.sha256)},
                "settings":settings
            }
        }));
    }
    let mut extra = config.extra.clone();
    extra["offers"] = serde_json::Value::Array(offers);
    ensure!(
        serde_json::to_vec(&extra)?.len() <= 256 * 1024,
        "model startup config exceeds bound"
    );
    config.extra = extra;
    Ok(())
}

fn settle_failure(data_dir: &Path, id: &str, drained: bool) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    let mut snapshot = inventory.load()?;
    let record = snapshot
        .records
        .iter_mut()
        .find(|r| r.operation_id == id)
        .context("preparation unavailable")?;
    if record.state == PreparationState::Admitted {
        return Ok(());
    }
    let admission_may_exist = record.admission_id == id
        && match inventory.admitted(id) {
            Ok(_) => true,
            Err(err) if storage::missing(&err) => false,
            Err(_) => true,
        };
    if !drained || admission_may_exist {
        record.state = PreparationState::Uncertain;
    } else {
        if record.admission_id == id {
            inventory.remove_stage()?;
        }
        record.state = if record.cancel_requested {
            PreparationState::Cancelled
        } else if now()? >= record.expires_at {
            PreparationState::Expired
        } else {
            PreparationState::Failed
        };
        record.reserved_bytes = 0;
    }
    inventory.save(&snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(operation: &str) -> CapsuleAffordanceDescriptor {
        serde_json::from_value(serde_json::json!({
            "id": format!("content.{operation}"), "operation": operation,
            "resource": RESOURCE, "risk": if operation == "status" { "read" } else { "write" },
            "approval": "runtime_policy", "audit": "summary",
        }))
        .unwrap()
    }

    fn context() -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: "person:reservation-fixture".into(),
            session_id: "session-1".into(),
            proof_binding_id: Some("proof-1".into()),
            grant_id: "grant-1".into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, String) {
        let root = tempfile::tempdir().unwrap();
        let payload = super::super::tests::model_catalog_fixture();
        super::super::tests::write_model_catalog_fixture(root.path(), &payload);
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"] = serde_json::json!({
                "max_cache_bytes": 64 * 1024 * 1024,
                "max_model_memory_bytes": 16_u64 * 1024 * 1024 * 1024,
            });
        });
        (root, payload["entries"][0]["cid"].as_str().unwrap().into())
    }

    fn change_config(root: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let path = root.join("components.json");
        let mut config = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        change(&mut config);
        std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    }

    fn caller<'a>(
        context: &'a HomeLaunchTokenContext,
        method: &'a CapsuleAffordanceDescriptor,
    ) -> PreparationCaller<'a> {
        PreparationCaller {
            context,
            capsule: "marketplace",
            interface: "elastos.marketplace.catalog",
            method,
        }
    }

    // This backend proves caller sequencing and exact retained bytes. Native
    // hashing and its real-Kubo CID proof are covered at the provider boundary.
    struct PreparationBackend {
        files: std::collections::BTreeMap<String, Vec<u8>>,
        cid: Mutex<String>,
        calls: Mutex<Vec<String>>,
        hold_drain: AtomicBool,
        fail_drain: AtomicBool,
        hold_read: AtomicBool,
        read_fault: Mutex<Option<&'static str>>,
        native: Option<Arc<dyn elastos_runtime::provider::Provider>>,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for PreparationBackend {
        fn name(&self) -> &'static str {
            "preparation-fixture"
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec![]
        }
        async fn handle(
            &self,
            _: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            panic!("preparation must use the private local boundary")
        }
        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
            let op = request["op"].as_str().unwrap();
            self.calls.lock().unwrap().push(op.into());
            if let Some(native) = &self.native {
                let response = native.send_raw(request).await?;
                if response["status"] == "error" {
                    eprintln!(
                        "isolated preparation native {op}: {}",
                        response.to_string().chars().take(1024).collect::<String>()
                    );
                }
                return Ok(response);
            }
            match op {
                "cat" => {
                    use base64::Engine as _;
                    assert_eq!(request["bounded_read"], true);
                    let path = request["path"].as_str().unwrap();
                    if path == "weights.gguf" && self.hold_read.swap(false, Ordering::AcqRel) {
                        self.entered.notify_one();
                        self.release.notified().await;
                    }
                    let mut response = if path == "_elastos_object.json" {
                        assert_eq!(request["max_bytes"], 65536);
                        serde_json::json!({"_runtime_complete_metadata":{
                            "schema":"elastos.provider.complete-metadata/v1", "cid":request["cid"],
                            "path":path, "max_bytes":65536, "actual_bytes":self.files[path].len(), "completed":true
                        }})
                    } else {
                        let range = &request["_runtime_invocation"]["range"];
                        serde_json::json!({"_runtime_applied_range":{
                            "schema":"elastos.provider.applied-range/v1", "cid":request["cid"],
                            "path":path, "start":range["start"], "end":range["end"]
                        }})
                    };
                    let mut bytes = if path == "_elastos_object.json" {
                        self.files[path].clone()
                    } else {
                        let range = &request["_runtime_invocation"]["range"];
                        let start = range["start"].as_u64().unwrap() as usize;
                        let end = range["end"].as_u64().unwrap() as usize;
                        assert!(end >= start && end - start < 65536);
                        self.files[path][start..=end].to_vec()
                    };
                    let fault = *self.read_fault.lock().unwrap();
                    match (path, fault) {
                        ("weights.gguf", Some("file_tamper"))
                        | ("_elastos_object.json", Some("index_tamper")) => {
                            *bytes.last_mut().unwrap() ^= 1;
                        }
                        ("weights.gguf", Some("file_truncate"))
                        | ("_elastos_object.json", Some("index_truncate")) => {
                            bytes.pop();
                        }
                        _ => {}
                    }
                    response["data"] =
                        serde_json::json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                    Ok(serde_json::json!({"status":"ok","data":response}))
                }
                "runtime_check_capacity" => Ok(serde_json::json!({"status":"ok","data":{
                    "volume_id":7,"capacity_bytes":1_u64 << 40,"available_bytes":1_u64 << 39,"required_bytes":request["required_bytes"]
                }})),
                "runtime_prepare_backend" => {
                    if self.hold_drain.swap(false, Ordering::AcqRel) {
                        self.entered.notify_one();
                        self.release.notified().await;
                    }
                    if self.fail_drain.load(Ordering::Acquire) {
                        return Err(elastos_runtime::provider::ProviderError::Provider(
                            "fixture drain failed".into(),
                        ));
                    }
                    Ok(serde_json::json!({"status":"ok"}))
                }
                "runtime_hash_staged_directory" => {
                    let directory = &request["directory"];
                    let root = Path::new(directory["root"].as_str().unwrap());
                    assert_eq!(
                        directory["files"].as_array().unwrap().len(),
                        self.files.len()
                    );
                    for file in directory["files"].as_array().unwrap() {
                        let path = file["path"].as_str().unwrap();
                        let bytes = std::fs::read(root.join(path)).unwrap();
                        assert_eq!(file["size"].as_u64().unwrap(), bytes.len() as u64);
                        assert_eq!(bytes, self.files[path]);
                    }
                    Ok(
                        serde_json::json!({"status":"ok","data":{"cid":self.cid.lock().unwrap().clone()}}),
                    )
                }
                _ => panic!("reconciliation/reuse dispatched unexpected operation {op}"),
            }
        }
    }

    fn package_fixture(
        weights: Vec<u8>,
    ) -> (
        serde_json::Value,
        std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        package_fixture_with_provenance(weights, b"fixture provenance")
    }

    fn package_fixture_with_provenance(
        weights: Vec<u8>,
        provenance: &[u8],
    ) -> (
        serde_json::Value,
        std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let mut payload = super::super::tests::model_catalog_fixture();
        let mut files: std::collections::BTreeMap<String, Vec<u8>> =
            std::collections::BTreeMap::from([
                ("LICENSE".into(), b"fixture license".to_vec()),
                ("LICENSE.base".into(), b"fixture base license".to_vec()),
                ("PROVENANCE.md".into(), provenance.to_vec()),
                (
                    "capsule.json".into(),
                    serde_json::to_vec(&payload["entries"][0]["capsule_manifest"]).unwrap(),
                ),
                ("weights.gguf".into(), weights),
            ]);
        let object = &mut payload["entries"][0]["object_manifest"];
        let mut digest = Sha256::new();
        for file in object["files"].as_array_mut().unwrap() {
            let path = file["path"].as_str().unwrap().to_owned();
            file["size"] = serde_json::json!(files[&path].len());
            file["sha256"] = serde_json::json!(hex::encode(Sha256::digest(&files[&path])));
            digest.update(path.as_bytes());
            digest.update(b"\0");
            digest.update(file["sha256"].as_str().unwrap().as_bytes());
            digest.update(b"\0");
            digest.update(file["size"].to_string().as_bytes());
            digest.update(b"\0");
        }
        object["content_digest"] = serde_json::json!(format!("sha256:{:x}", digest.finalize()));
        let index = serde_json::to_vec(object).unwrap();
        files.insert("_elastos_object.json".into(), index);
        (payload, files)
    }

    fn write_preparation_catalog(root: &Path, payload: &serde_json::Value) {
        super::super::tests::write_model_catalog_fixture(root, payload);
        change_config(root, |config| {
            config["model_catalog"]["local_use"] = serde_json::json!({
                "max_cache_bytes":64 * 1024 * 1024, "max_model_memory_bytes":16_u64 * 1024 * 1024 * 1024
            })
        });
    }

    async fn staged_fixture(
        created_at: u64,
        renamed: bool,
    ) -> (
        tempfile::TempDir,
        PreparationRecord,
        Arc<PreparationBackend>,
        Arc<elastos_runtime::provider::ProviderRegistry>,
    ) {
        let (payload, files) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
        staged_package_fixture(created_at, renamed, payload, files).await
    }

    async fn staged_package_fixture(
        created_at: u64,
        renamed: bool,
        payload: serde_json::Value,
        files: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> (
        tempfile::TempDir,
        PreparationRecord,
        Arc<PreparationBackend>,
        Arc<elastos_runtime::provider::ProviderRegistry>,
    ) {
        let root = tempfile::tempdir().unwrap();
        write_preparation_catalog(root.path(), &payload);
        let cid = payload["entries"][0]["cid"].as_str().unwrap().to_owned();
        let mut record = reserve_at(
            root.path(),
            &caller(&context(), &method("use")),
            "original",
            &cid,
            created_at,
        )
        .unwrap();
        record.state = PreparationState::AdmissionPending;
        record.index_bytes = files["_elastos_object.json"].len() as u64;
        record.completed_bytes = record.total_bytes;
        record.reserved_bytes = preparation_charge(record.total_bytes).unwrap();
        {
            let inventory = Inventory::open(root.path(), false).unwrap();
            let mut snapshot = inventory.load().unwrap();
            snapshot.records[0] = record.clone();
            inventory.save(&snapshot).unwrap();
            let stage = inventory.stage(true).unwrap();
            for (path, bytes) in &files {
                let mut file = stage.create_file(path).unwrap();
                file.write_all(bytes).unwrap();
                file.sync_all().unwrap();
            }
            if renamed {
                inventory.admit(&record.operation_id).unwrap();
            }
        }
        let backend = Arc::new(PreparationBackend::new(files, cid));
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        registry
            .register_sub_provider("ipfs", backend.clone())
            .await
            .unwrap();
        (root, record, backend, registry)
    }

    #[test]
    fn model_startup_profile_is_runtime_owned_and_rejects_unproved_hosts() {
        assert_eq!(
            local_model_startup_profile("darwin-arm64").unwrap(),
            serde_json::json!({
                "context_size":4096, "parallel":1, "threads":8, "batch_threads":8,
                "gpu_layers":99, "health_timeout_ms":120000,
                "shutdown_timeout_ms":5000, "enable_thinking":false
            })
        );
        for platform in ["linux-arm64", "linux-amd64", "darwin-amd64", "*"] {
            assert!(local_model_startup_profile(platform).is_err());
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    mod startup_binding {
        use super::*;
        use std::os::unix::fs::PermissionsExt as _;

        struct EngineFixture(std::path::PathBuf);

        impl Drop for EngineFixture {
            fn drop(&mut self) {
                // Only this disposable fixture bundle needs write permission
                // restored so TempDir can remove its protected receipt/files.
                std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }

        fn install_engine(root: &Path) -> EngineFixture {
            let relative = "libexec/fixture-engine";
            let bundle = root.join(relative);
            std::fs::create_dir_all(&bundle).unwrap();
            let bytes = b"fixture engine bytes; never executed";
            let digest = format!("sha256:{:x}", Sha256::digest(bytes));
            let archive = format!("sha256:{}", "a".repeat(64));
            let platform = crate::setup::detect_platform();
            std::fs::write(bundle.join("llama-server"), bytes).unwrap();
            std::fs::write(
                bundle.join(".elastos-engine.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema":"elastos.local-model-engine/v2", "version":"fixture-v1",
                    "platform":platform, "archive_sha256":archive,
                    "entries":[{"path":"llama-server", "sha256":digest, "type":"file"}]
                }))
                .unwrap(),
            )
            .unwrap();
            change_config(root, |config| {
                config["external"]["llama-server"] = serde_json::json!({
                    "version":"fixture-v1", "platforms":{platform:{
                        "install_path":relative, "binary_path":"llama-server", "checksum":archive
                    }}
                });
            });
            for (name, mode) in [("llama-server", 0o500), (".elastos-engine.json", 0o400)] {
                std::fs::set_permissions(bundle.join(name), std::fs::Permissions::from_mode(mode))
                    .unwrap();
            }
            std::fs::set_permissions(&bundle, std::fs::Permissions::from_mode(0o500)).unwrap();
            EngineFixture(bundle)
        }

        fn config(root: &Path) -> elastos_runtime::provider::BridgeProviderConfig {
            elastos_runtime::provider::BridgeProviderConfig {
                base_path: root.canonicalize().unwrap().to_string_lossy().into_owned(),
                extra: serde_json::json!({
                    "provider_id":"model-provider",
                    "journal_dir":root.join("providers/model-provider/journal"),
                    "offers":[{"id":"operator-owned", "enabled":false}]
                }),
                ..Default::default()
            }
        }

        fn admit(root: &Path, record: &PreparationRecord) {
            update_operation(root, &record.operation_id, |r| {
                r.state = PreparationState::Admitted
            })
            .unwrap();
        }

        #[tokio::test]
        async fn model_startup_binding_requires_admission_and_preserves_operator_config() {
            let empty = tempfile::tempdir().unwrap();
            let registry = elastos_runtime::provider::ProviderRegistry::new();
            let mut actual = config(empty.path());
            let before = serde_json::to_value(&actual).unwrap();
            append_admitted_model_startup_offers(empty.path(), &registry, &mut actual)
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(&actual).unwrap(), before);
            assert!(!empty.path().join("model-preparation").exists());

            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            let mut actual = config(root.path());
            let before = actual.extra.clone();
            append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                .await
                .unwrap();
            assert_eq!(actual.extra, before, "pending rename is not admitted");
            admit(root.path(), &record);
            assert!(
                append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                    .await
                    .is_err()
            );
            assert_eq!(
                actual.extra, before,
                "missing engine preserves configured offers"
            );
            assert!(backend.calls.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn model_startup_binding_consumes_admission_with_stable_restart_and_alias_identity() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let mut first = config(root.path());
            let operator = first.extra["offers"][0].clone();
            append_admitted_model_startup_offers(root.path(), &registry, &mut first)
                .await
                .unwrap();
            assert_eq!(first.extra["offers"].as_array().unwrap().len(), 2);
            assert_eq!(first.extra["offers"][0], operator);
            let offer = &first.extra["offers"][1];
            assert_eq!(offer["operation"], "text.generate");
            assert_eq!(offer["adapter"]["kind"], "local_llama_cpp_text");
            assert_eq!(
                offer["adapter"]["settings"],
                local_model_startup_profile("darwin-arm64").unwrap()
            );
            assert_eq!(
                offer["adapter"]["model"]["sha256"],
                format!("sha256:{:x}", Sha256::digest(b"GGUF\x03\0\0\0fixture"))
            );
            assert_eq!(
                offer["adapter"]["model"]["path"],
                root.path()
                    .canonicalize()
                    .unwrap()
                    .join("model-preparation")
                    .join(format!("admitted-{}", record.operation_id))
                    .join("weights.gguf")
                    .to_string_lossy()
                    .as_ref()
            );
            assert_eq!(
                backend.calls.lock().unwrap().as_slice(),
                ["runtime_hash_staged_directory"]
            );

            {
                let inventory = Inventory::open(root.path(), false).unwrap();
                let mut state = inventory.load().unwrap();
                let mut alias = state.records[0].clone();
                alias.request_binding.principal = "person:second-owner".into();
                alias.request_binding.request_id = "alias-request".into();
                alias.request_binding = binding(&alias);
                alias.operation_id = operation_id(&alias).unwrap();
                alias.reserved_bytes = 0;
                state.records.push(alias);
                inventory.save(&state).unwrap();
            }
            let state_path = root.path().join("model-preparation/state.json");
            let persisted = std::fs::read(&state_path).unwrap();
            let mut restarted = config(root.path());
            append_admitted_model_startup_offers(root.path(), &registry, &mut restarted)
                .await
                .unwrap();
            assert_eq!(
                restarted.extra, first.extra,
                "aliases/restart must not create another offer"
            );
            assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
            assert!(!root
                .path()
                .join("providers/model-provider/journal")
                .exists());

            let mut collision = config(root.path());
            collision.extra["offers"]
                .as_array_mut()
                .unwrap()
                .push(offer.clone());
            let before = collision.extra.clone();
            assert!(
                append_admitted_model_startup_offers(root.path(), &registry, &mut collision)
                    .await
                    .is_err()
            );
            assert_eq!(
                collision.extra, before,
                "operator IDs cannot be overwritten"
            );

            let inventory = Inventory::open(root.path(), false).unwrap();
            let mut invalid = inventory.load().unwrap();
            invalid.records[1].request_binding.principal = "person:forged-owner".into();
            assert!(inventory.save(&invalid).is_err());
            assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
        }

        #[tokio::test]
        async fn model_startup_binding_keeps_whole_package_identity_when_weights_match() {
            let mut ids = Vec::new();
            let mut weights = Vec::new();
            for (index, provenance) in [
                b"publisher conversion A".as_slice(),
                b"publisher conversion B",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut payload, files) =
                    package_fixture_with_provenance(b"GGUF\x03\0\0\0fixture".to_vec(), provenance);
                // The existing backend fixture stands in for native CID hashing;
                // distinct notice bytes belong to distinct package identities.
                let hash =
                    cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest([index as u8]))
                        .unwrap();
                payload["entries"][0]["cid"] =
                    serde_json::json!(cid::Cid::new_v1(0x70, hash).to_string());
                let (root, record, _, registry) =
                    staged_package_fixture(now().unwrap(), true, payload, files).await;
                admit(root.path(), &record);
                let _engine = install_engine(root.path());
                let mut actual = config(root.path());
                append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                    .await
                    .unwrap();
                ids.push(actual.extra["offers"][1]["id"].clone());
                weights.push(actual.extra["offers"][1]["adapter"]["model"]["sha256"].clone());
            }
            assert_eq!(weights[0], weights[1]);
            assert_ne!(
                ids[0], ids[1],
                "the provider execution binding includes the package-bound offer ID"
            );
        }

        #[tokio::test]
        async fn model_startup_binding_requires_current_catalog_trust() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"] = serde_json::Value::Null
            });
            let mut actual = config(root.path());
            let before = actual.extra.clone();
            assert!(
                append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                    .await
                    .is_err()
            );
            assert_eq!(actual.extra, before);
            assert!(backend.calls.lock().unwrap().is_empty());

            change_config(root.path(), |config| {
                config["model_catalog"] = serde_json::Value::Null
            });
            append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                .await
                .unwrap();
            assert_eq!(
                actual.extra, before,
                "removed catalog cannot admit a persisted offer"
            );
            assert!(backend.calls.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn model_startup_binding_rejects_tampering_and_unavailable_verifier_without_config_effects(
        ) {
            for fault in [
                "weights",
                "notice",
                "index",
                "engine",
                "receipt",
                "platform",
                "alias",
                "native_cid",
                "backend",
            ] {
                let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
                admit(root.path(), &record);
                let engine = install_engine(root.path());
                let admitted = root
                    .path()
                    .join("model-preparation")
                    .join(format!("admitted-{}", record.operation_id));
                match fault {
                    "weights" => {
                        std::fs::write(admitted.join("weights.gguf"), b"GGUF\x03\0\0\0corrupt")
                            .unwrap()
                    }
                    "notice" => {
                        std::fs::write(admitted.join("LICENSE"), b"changed license").unwrap()
                    }
                    "index" => {
                        std::fs::write(admitted.join("_elastos_object.json"), b"{}").unwrap()
                    }
                    "engine" | "receipt" => {
                        let path = engine.0.join(if fault == "engine" {
                            "llama-server"
                        } else {
                            ".elastos-engine.json"
                        });
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                            .unwrap();
                        std::fs::write(&path, b"changed").unwrap();
                        std::fs::set_permissions(
                            path,
                            std::fs::Permissions::from_mode(if fault == "engine" {
                                0o500
                            } else {
                                0o400
                            }),
                        )
                        .unwrap();
                    }
                    "platform" => change_config(root.path(), |config| {
                        config["external"]["llama-server"]["platforms"] = serde_json::json!({});
                    }),
                    "alias" => {
                        std::fs::remove_file(admitted.join("weights.gguf")).unwrap();
                        std::os::unix::fs::symlink("LICENSE", admitted.join("weights.gguf"))
                            .unwrap();
                    }
                    "native_cid" => {
                        *backend.cid.lock().unwrap() =
                            "bafybeiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()
                    }
                    "backend" => {}
                    _ => unreachable!(),
                }
                let unavailable = elastos_runtime::provider::ProviderRegistry::new();
                let selected = if fault == "backend" {
                    &unavailable
                } else {
                    registry.as_ref()
                };
                let mut actual = config(root.path());
                let before = serde_json::to_value(&actual).unwrap();
                let persisted =
                    std::fs::read(root.path().join("model-preparation/state.json")).unwrap();
                assert!(
                    append_admitted_model_startup_offers(root.path(), selected, &mut actual)
                        .await
                        .is_err(),
                    "{fault}"
                );
                assert_eq!(serde_json::to_value(&actual).unwrap(), before, "{fault}");
                assert_eq!(
                    std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
                    persisted
                );
            }
        }

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_MODEL_PROVIDER_PATH; parent runs actual provider Init proof"]
        async fn model_startup_binding_real_model_provider_accepts_generated_init() {
            use elastos_runtime::provider::ProviderBridge;

            let binary = std::path::PathBuf::from(
                std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH")
                    .expect("explicit model-provider prerequisite required"),
            );
            assert!(binary.is_absolute() && std::fs::symlink_metadata(&binary).unwrap().is_file());
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let mut actual = config(root.path());
            actual.extra["offers"] = serde_json::json!([]);
            append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                .await
                .unwrap();
            let id = actual.extra["offers"][0]["id"].clone();
            let bridge = ProviderBridge::spawn(&binary, actual).await.unwrap();
            let response = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let status = bridge.send_raw(&serde_json::json!({"op":"status"})).await?;
                let offers = bridge
                    .send_raw(&serde_json::json!({"op":"offers_list"}))
                    .await?;
                Ok::<_, elastos_runtime::provider::bridge::BridgeError>((status, offers))
            })
            .await;
            // Reap before assertions so a schema failure cannot leak the child.
            bridge.shutdown().await.unwrap();
            let (status, offers) = response.unwrap().unwrap();
            assert_eq!(status["status"], "ok");
            assert_eq!(status["data"]["offers_ready"], 1);
            assert_eq!(offers["status"], "ok");
            assert_eq!(offers["data"]["offers"][0]["id"], id);
            assert_eq!(offers["data"]["offers"].as_array().unwrap().len(), 1);
            assert!(bridge
                .send_raw(&serde_json::json!({"op":"status"}))
                .await
                .is_err());
        }
    }

    impl PreparationBackend {
        fn new(files: std::collections::BTreeMap<String, Vec<u8>>, cid: String) -> Self {
            Self {
                files,
                cid: Mutex::new(cid),
                calls: Mutex::new(vec![]),
                hold_drain: AtomicBool::new(false),
                fail_drain: AtomicBool::new(false),
                hold_read: AtomicBool::new(false),
                read_fault: Mutex::new(None),
                native: None,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            }
        }
    }

    async fn register_content(
        registry: &Arc<elastos_runtime::provider::ProviderRegistry>,
        root: &Path,
    ) {
        registry
            .register_sub_provider(
                "content",
                Arc::new(crate::content::ContentProvider::new(
                    root.to_path_buf(),
                    Arc::downgrade(registry),
                )),
            )
            .await
            .unwrap();
    }

    fn assert_failed_start_preflight(missing_registry: bool) {
        let (root, cid) = fixture();
        // A 1 MiB budget fits the signed payload but not the full charge.
        // The other case retains sufficient budget and isolates readiness.
        if !missing_registry {
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"]["max_cache_bytes"] =
                    serde_json::json!(1024 * 1024)
            });
        }
        let registry = (!missing_registry)
            .then(|| Arc::new(elastos_runtime::provider::ProviderRegistry::new()));
        let owner = PreparationOwner::default();
        assert!(owner
            .invoke(
                root.path(),
                registry,
                caller(&context(), &method("use")),
                "failed-preflight",
                &serde_json::json!({"cid":cid}),
                Arc::new(|| Ok(()))
            )
            .is_err());
        assert!(owner.worker.lock().unwrap().is_none());
        if root.path().join("model-preparation").exists() {
            let inventory = Inventory::open(root.path(), false).unwrap();
            assert!(
                inventory.load().unwrap().records.is_empty(),
                "failed preflight stranded a reservation (missing_registry={missing_registry})"
            );
        }
    }

    #[tokio::test]
    async fn model_preparation_full_charge_preflight_does_not_strand_reservation() {
        assert_failed_start_preflight(false);
    }

    #[tokio::test]
    async fn model_preparation_missing_registry_preflight_does_not_strand_reservation() {
        assert_failed_start_preflight(true);
    }

    #[tokio::test]
    async fn model_preparation_aggregate_budget_counts_admission_and_full_reservation() {
        let (root, admitted, _, _) = staged_fixture(now().unwrap(), true).await;
        update_operation(root.path(), &admitted.operation_id, |record| {
            record.state = PreparationState::Admitted
        })
        .unwrap();
        let charge = preparation_charge(admitted.total_bytes).unwrap();
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] = serde_json::json!(charge)
        });
        // Exact-CID aliases fit at the existing artifact's exact budget: one
        // charge remains attached to the original, with separate request binding.
        let alias = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "alias-at-limit",
            &admitted.package_cid,
        )
        .unwrap();
        assert_eq!(alias.admission_id, admitted.operation_id);
        assert_eq!(alias.reserved_bytes, 0);
        cancel(
            root.path(),
            &caller(&context(), &method("cancel")),
            &alias.operation_id,
        )
        .unwrap();

        // The catalog moves to a different package while the prior artifact
        // remains admitted and charged. A per-entry-only check would overbook.
        let (mut payload, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
        let next_cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
        assert_ne!(next_cid, admitted.package_cid);
        payload["entries"][0]["cid"] = serde_json::json!(next_cid);
        write_preparation_catalog(root.path(), &payload);
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] =
                serde_json::json!(2 * charge - 1)
        });
        let state_path = root.path().join("model-preparation/state.json");
        let original = std::fs::read(&state_path).unwrap();
        let owner = PreparationOwner::default();
        assert!(owner
            .invoke(
                root.path(),
                Some(Arc::new(elastos_runtime::provider::ProviderRegistry::new())),
                caller(&context(), &method("use")),
                "next-package",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(()))
            )
            .is_err());
        assert!(owner.worker.lock().unwrap().is_none());
        assert_eq!(std::fs::read(&state_path).unwrap(), original);

        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] = serde_json::json!(2 * charge)
        });
        let reserved = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "next-package",
            next_cid,
        )
        .unwrap();
        assert_eq!(reserved.state, PreparationState::Reserved);
        assert_eq!(reserved.reserved_bytes, charge);
        let snapshot = Inventory::open(root.path(), false).unwrap().load().unwrap();
        assert_eq!(
            snapshot
                .records
                .iter()
                .map(|record| record.reserved_bytes)
                .sum::<u64>(),
            2 * charge
        );
    }

    #[tokio::test]
    async fn model_preparation_actual_fetch_rejects_tampered_and_truncated_bytes() {
        for fault in [
            "index_tamper",
            "index_truncate",
            "file_tamper",
            "file_truncate",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (payload, files) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            write_preparation_catalog(root.path(), &payload);
            let cid = payload["entries"][0]["cid"].as_str().unwrap().to_owned();
            let backend = Arc::new(PreparationBackend::new(files, cid.clone()));
            *backend.read_fault.lock().unwrap() = Some(fault);
            let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
            registry
                .register_sub_provider("ipfs", backend.clone())
                .await
                .unwrap();
            register_content(&registry, root.path()).await;
            let owner = PreparationOwner::default();
            let result = owner
                .invoke(
                    root.path(),
                    Some(registry),
                    caller(&context(), &method("use")),
                    "fresh",
                    &serde_json::json!({"cid":cid}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            join_worker(&owner).await;
            let record =
                load_operation(root.path(), result["operation_id"].as_str().unwrap()).unwrap();
            assert_eq!(record.state, PreparationState::Failed, "{fault}");
            assert_eq!(record.reserved_bytes, 0);
            assert!(!root.path().join("model-preparation/stage").exists());
            assert!(storage::missing(
                &Inventory::open(root.path(), false)
                    .unwrap()
                    .admitted(&record.operation_id)
                    .err()
                    .unwrap()
            ));
            let calls = backend.calls.lock().unwrap();
            assert!(calls.iter().any(|op| op == "cat"));
            assert!(!calls.iter().any(|op| op == "runtime_hash_staged_directory"));
            assert_eq!(calls.last().unwrap(), "runtime_prepare_backend");
        }
    }

    #[tokio::test]
    async fn model_preparation_actual_fetch_cancel_waits_for_held_read() {
        let root = tempfile::tempdir().unwrap();
        let (payload, files) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
        write_preparation_catalog(root.path(), &payload);
        let cid = payload["entries"][0]["cid"].as_str().unwrap().to_owned();
        let backend = Arc::new(PreparationBackend::new(files, cid.clone()));
        backend.hold_read.store(true, Ordering::Release);
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        registry
            .register_sub_provider("ipfs", backend.clone())
            .await
            .unwrap();
        register_content(&registry, root.path()).await;
        let owner = PreparationOwner::default();
        let result = owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("use")),
                "fresh",
                &serde_json::json!({"cid":cid}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            backend.entered.notified(),
        )
        .await
        .unwrap();
        let id = result["operation_id"].as_str().unwrap();
        let before = load_operation(root.path(), id).unwrap();
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("cancel")),
                "cancel",
                &serde_json::json!({"operation_id":id}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        assert_eq!(
            load_operation(root.path(), id).unwrap().reserved_bytes,
            before.reserved_bytes
        );
        assert!(root.path().join("model-preparation/stage").exists());
        assert!(!owner.worker.lock().unwrap().as_ref().unwrap().is_finished());
        backend.release.notify_one();
        join_worker(&owner).await;
        let record = load_operation(root.path(), id).unwrap();
        assert_eq!(record.state, PreparationState::Cancelled);
        assert_eq!(record.completed_bytes, before.completed_bytes);
        assert_eq!(record.reserved_bytes, 0);
        assert!(!root.path().join("model-preparation/stage").exists());
        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.last().unwrap(), "runtime_prepare_backend");
        assert!(!calls.iter().any(|op| op == "runtime_hash_staged_directory"));
    }

    async fn join_worker(owner: &PreparationOwner) {
        let task = owner
            .worker
            .lock()
            .unwrap()
            .take()
            .expect("worker was started");
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    mod process_proof {
        use super::*;
        use std::fs::{self, File};
        use std::io::{Read as _, Seek as _};
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        use std::path::PathBuf;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                self.0.wait().expect("reap exact fixture child");
            }
        }

        fn command(binary: &Path, root: &Path, repo: &Path, cwd: &Path, args: &[&str]) -> Command {
            let mut command = Command::new(binary);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .env("HOME", root)
                .env("IPFS_PATH", repo)
                .env("TMPDIR", root)
                .env("PATH", "/usr/bin:/bin")
                .env("LANG", "C")
                .stdin(Stdio::null());
            command
        }

        // File-backed output avoids pipe deadlocks; all commands share the test
        // deadline and the same small output cap. This owner only runs Kubo CLI.
        async fn run(mut command: Command, root: &Path, deadline: Instant) -> String {
            let mut stdout = tempfile::tempfile_in(root).unwrap();
            let stderr = tempfile::tempfile_in(root).unwrap();
            let mut child = OwnedChild(
                command
                    .stdout(stdout.try_clone().unwrap())
                    .stderr(stderr.try_clone().unwrap())
                    .spawn()
                    .unwrap(),
            );
            loop {
                assert!(Instant::now() < deadline, "fixture command deadline");
                assert!(
                    stdout.metadata().unwrap().len() <= 65536
                        && stderr.metadata().unwrap().len() <= 65536,
                    "fixture command output bound"
                );
                if let Some(status) = child.0.try_wait().unwrap() {
                    if !status.success() {
                        let mut diagnostic = stderr.try_clone().unwrap();
                        diagnostic.rewind().unwrap();
                        let mut bytes = Vec::new();
                        diagnostic.take(4096).read_to_end(&mut bytes).unwrap();
                        panic!(
                            "isolated Kubo command failed: {status}: {}",
                            String::from_utf8_lossy(&bytes)
                        );
                    }
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            stdout.rewind().unwrap();
            let mut bytes = Vec::new();
            stdout.take(65537).read_to_end(&mut bytes).unwrap();
            assert!(bytes.len() <= 65536);
            String::from_utf8(bytes).unwrap().trim().into()
        }

        fn disk_bytes(path: &Path) -> (u64, u64) {
            fn visit(path: &Path, count: &mut usize) -> (u64, u64) {
                let meta = match fs::symlink_metadata(path) {
                    Ok(meta) => meta,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (0, 0),
                    Err(error) => panic!("fixture allocation observation failed: {error}"),
                };
                *count += 1;
                assert!(*count <= 4096 && (meta.is_dir() || meta.is_file()));
                let mut bytes = (
                    if meta.is_file() { meta.len() } else { 0 },
                    meta.blocks() * 512,
                );
                if meta.is_dir() {
                    // Rename can remove a sampled stage between stat/read_dir.
                    let entries = match fs::read_dir(path) {
                        Ok(entries) => entries,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return bytes,
                        Err(error) => panic!("fixture directory observation failed: {error}"),
                    };
                    for entry in entries {
                        let child = visit(&entry.unwrap().path(), count);
                        bytes.0 += child.0;
                        bytes.1 += child.1;
                    }
                }
                bytes
            }
            visit(path, &mut 0)
        }

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_KUBO_PATH and ELASTOS_TEST_IPFS_PROVIDER_PATH; parent runs isolated process proof"]
        async fn model_preparation_real_content_native_process_use_admit_and_reuse() {
            use elastos_runtime::provider::{
                BridgeProviderConfig, CapsuleProvider, ProviderBridge,
            };
            let prerequisite = |name| {
                let path = PathBuf::from(
                    std::env::var_os(name)
                        .expect("explicit prerequisite required; skipped test is not proof"),
                );
                assert!(path.is_absolute() && fs::symlink_metadata(&path).unwrap().is_file());
                path.canonicalize().unwrap()
            };
            let kubo = prerequisite("ELASTOS_TEST_KUBO_PATH");
            let native = prerequisite("ELASTOS_TEST_IPFS_PROVIDER_PATH");
            if let Some(override_path) = std::env::var_os("ELASTOS_IPFS_KUBO_PATH") {
                assert_eq!(
                    fs::canonicalize(override_path).unwrap(),
                    kubo,
                    "ambient provider override differs from pinned fixture prerequisite"
                );
            }
            let root = tempfile::tempdir().unwrap();
            let root_path = root.path().canonicalize().unwrap();
            let dir = File::open(&root_path).unwrap();
            let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            assert_eq!(
                unsafe { libc::fstatvfs(dir.as_raw_fd(), stats.as_mut_ptr()) },
                0
            );
            let stats = unsafe { stats.assume_init() };
            storage::require_space_floor(
                u128::from(stats.f_blocks) * u128::from(stats.f_frsize),
                u128::from(stats.f_bavail) * u128::from(stats.f_frsize),
                64 * 1024 * 1024,
            )
            .unwrap();
            let data = root_path.join("data");
            let seed = root_path.join("seed");
            let repo = data.join("ipfs-repo");
            for path in [&data, &seed, &data.join("bin")] {
                fs::create_dir(path).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            // The native provider resolves the explicit fixture tool locally;
            // only this isolated directory contains the test link.
            std::os::unix::fs::symlink(&kubo, data.join("bin/kubo")).unwrap();
            let deadline = Instant::now() + Duration::from_secs(90);
            assert_eq!(
                run(
                    command(
                        &kubo,
                        &root_path,
                        &repo,
                        &root_path,
                        &["version", "--number"]
                    ),
                    &root_path,
                    deadline
                )
                .await,
                "0.40.1"
            );
            run(
                command(
                    &kubo,
                    &root_path,
                    &repo,
                    &root_path,
                    &["init", "--empty-repo"],
                ),
                &root_path,
                deadline,
            )
            .await;
            let config_path = repo.join("config");
            let mut config: serde_json::Value =
                serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
            config["Bootstrap"] = serde_json::json!([]);
            config["Addresses"]["API"] = serde_json::json!("/ip4/127.0.0.1/tcp/0");
            config["Addresses"]["Gateway"] = serde_json::json!("");
            config["Addresses"]["Swarm"] = serde_json::json!([]);
            config["Routing"]["Type"] = serde_json::json!("none");
            config["Discovery"]["MDNS"]["Enabled"] = serde_json::json!(false);
            config["AutoConf"]["Enabled"] = serde_json::json!(false);
            config["Import"] = serde_json::json!({
                "CidVersion":1,"UnixFSRawLeaves":true,"UnixFSChunker":"size-262144", "HashFunction":"sha2-256",
                "UnixFSFileMaxLinks":174,"UnixFSDirectoryMaxLinks":0,"UnixFSHAMTDirectoryMaxFanout":256,
                "UnixFSHAMTDirectorySizeThreshold":"256KiB","UnixFSHAMTDirectorySizeEstimation":"links",
                "UnixFSDAGLayout":"balanced","FastProvideRoot":false,"FastProvideWait":false
            });
            fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
            let mut weights = vec![0; 8 * 1024 * 1024];
            let mut random = 0x7b16_984d_3c20_a5e1u64;
            for bytes in weights.chunks_mut(8) {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let len = bytes.len();
                bytes.copy_from_slice(&random.to_le_bytes()[..len]);
            }
            weights[..8].copy_from_slice(b"GGUF\x03\0\0\0");
            let (mut payload, files) = package_fixture(weights);
            assert!(files.values().map(|bytes| bytes.len()).sum::<usize>() <= 16 * 1024 * 1024);
            for (path, bytes) in &files {
                fs::write(seed.join(path), bytes).unwrap();
            }
            let mut args = vec![
                "add",
                "--recursive=true",
                "--quieter=true",
                "--wrap-with-directory=true",
                "--only-hash=false",
                "--pin=true",
                "--cid-version=1",
                "--hash=sha2-256",
                "--raw-leaves=true",
                "--chunker=size-262144",
                "--trickle=false",
                "--max-file-links=174",
                "--max-directory-links=0",
                "--max-hamt-fanout=256",
                "--inline=false",
                "--nocopy=false",
                "--fscache=false",
                "--preserve-mode=false",
                "--preserve-mtime=false",
                "--empty-dirs=false",
                "--progress=false",
                "--fast-provide-root=false",
                "--fast-provide-wait=false",
            ];
            args.extend(files.keys().map(String::as_str));
            let cid = run(
                command(&kubo, &root_path, &repo, &seed, &args),
                &root_path,
                deadline,
            )
            .await;
            assert!(canonical_cid(&cid, 0x70));
            payload["entries"][0]["cid"] = serde_json::json!(cid);
            write_preparation_catalog(&data, &payload);
            assert!(
                !data.join("model-preparation").exists(),
                "Use starts with an empty preparation inventory"
            );
            let backend_before = disk_bytes(&repo);
            let seed_disk = disk_bytes(&seed);
            let stdout = tempfile::tempfile_in(&root_path).unwrap();
            let stderr = tempfile::tempfile_in(&root_path).unwrap();
            let mut daemon = OwnedChild(
                command(
                    &kubo,
                    &root_path,
                    &repo,
                    &root_path,
                    &["daemon", "--offline", "--routing=none", "--enable-gc=false"],
                )
                .stdout(stdout.try_clone().unwrap())
                .stderr(stderr.try_clone().unwrap())
                .spawn()
                .unwrap(),
            );
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_millis(250))
                .build()
                .unwrap();
            let port = loop {
                assert!(
                    Instant::now() < deadline,
                    "fixture daemon readiness deadline"
                );
                assert!(
                    stdout.metadata().unwrap().len() <= 65536
                        && stderr.metadata().unwrap().len() <= 65536
                );
                assert!(
                    daemon.0.try_wait().unwrap().is_none(),
                    "fixture daemon exited"
                );
                if let Ok(api) = fs::read_to_string(repo.join("api")) {
                    if let Some(port) = api
                        .trim()
                        .strip_prefix("/ip4/127.0.0.1/tcp/")
                        .and_then(|s| s.parse::<u16>().ok())
                    {
                        if let Ok(mut response) = client
                            .post(format!("http://127.0.0.1:{port}/api/v0/version"))
                            .send()
                            .await
                        {
                            let mut bytes = Vec::new();
                            while let Some(chunk) = response.chunk().await.unwrap() {
                                assert!(bytes.len() + chunk.len() <= 4096);
                                bytes.extend_from_slice(&chunk);
                            }
                            let version: serde_json::Value =
                                serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(version["Version"], "0.40.1");
                            break port;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            fs::write(data.join("ipfs-coords.json"), serde_json::to_vec(&serde_json::json!({
                "kubo_pid":daemon.0.id(),"api_port":port,"gateway_port":0,"started_at":now().unwrap(),"last_used":now().unwrap()
            })).unwrap()).unwrap();
            let bridge = Arc::new(
                ProviderBridge::spawn(
                    &native,
                    BridgeProviderConfig {
                        base_path: data.to_string_lossy().into_owned(),
                        ..Default::default()
                    },
                )
                .await
                .expect("real native provider Init"),
            );
            let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
            let mut backend = PreparationBackend::new(files, cid.clone());
            backend.native = Some(Arc::new(CapsuleProvider::with_scheme(
                bridge.clone(),
                "ipfs",
            )));
            let backend = Arc::new(backend);
            registry
                .register_sub_provider("ipfs", backend.clone())
                .await
                .unwrap();
            register_content(&registry, &data).await;
            let owner = Arc::new(PreparationOwner::default());
            let test_owner = owner.clone();
            let test_data = data.clone();
            let test_repo = repo.clone();
            let test_registry = registry.clone();
            let test_backend = backend.clone();
            let mut work = tokio::spawn(async move {
                let started = Instant::now();
                let input = serde_json::json!({"cid":cid});
                let response = test_owner
                    .invoke(
                        &test_data,
                        Some(test_registry.clone()),
                        caller(&context(), &method("use")),
                        "process-use",
                        &input,
                        Arc::new(|| Ok(())),
                    )
                    .unwrap();
                let id = response["operation_id"].as_str().unwrap().to_owned();
                let mut staged_peak = (0, 0);
                let mut backend_peak = backend_before;
                while !test_owner
                    .worker
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_finished()
                {
                    let stage = disk_bytes(&test_data.join("model-preparation/stage"));
                    let backend = disk_bytes(&test_repo);
                    staged_peak = (staged_peak.0.max(stage.0), staged_peak.1.max(stage.1));
                    backend_peak = (backend_peak.0.max(backend.0), backend_peak.1.max(backend.1));
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                join_worker(&test_owner).await;
                let elapsed_ms = started.elapsed().as_millis();
                let record = load_operation(&test_data, &id).unwrap();
                assert_eq!(record.state, PreparationState::Admitted);
                assert_eq!(record.completed_bytes, record.total_bytes);
                assert_eq!(
                    record.index_bytes,
                    test_backend.files["_elastos_object.json"].len() as u64
                );
                assert!(!test_data.join("model-preparation/stage").exists());
                let admitted = Inventory::open(&test_data, false)
                    .unwrap()
                    .admitted(&id)
                    .unwrap();
                let inode = fs::metadata(&admitted.path).unwrap().ino();
                for (path, bytes) in &test_backend.files {
                    assert_eq!(fs::read(admitted.path.join(path)).unwrap(), *bytes);
                }
                let admitted_disk = disk_bytes(&admitted.path);
                let requests = test_backend.calls.lock().unwrap().clone();
                let reads = requests.iter().filter(|op| *op == "cat").count();
                let expected_reads = 1 + test_backend
                    .files
                    .iter()
                    .filter(|(path, _)| *path != "_elastos_object.json")
                    .map(|(_, bytes)| bytes.len().div_ceil(65536))
                    .sum::<usize>();
                assert_eq!(reads, expected_reads);
                assert_eq!(
                    requests
                        .iter()
                        .filter(|op| *op == "runtime_hash_staged_directory")
                        .count(),
                    1
                );
                let reopened = PreparationOwner::default();
                let alias = reopened
                    .invoke(
                        &test_data,
                        Some(test_registry),
                        caller(&context(), &method("use")),
                        "process-reuse",
                        &input,
                        Arc::new(|| Ok(())),
                    )
                    .unwrap();
                join_worker(&reopened).await;
                let reused =
                    load_operation(&test_data, alias["operation_id"].as_str().unwrap()).unwrap();
                assert_eq!(reused.state, PreparationState::Admitted);
                assert_eq!(reused.admission_id, id);
                assert_eq!(reused.reserved_bytes, 0);
                assert_eq!(fs::metadata(&admitted.path).unwrap().ino(), inode);
                assert_eq!(
                    test_backend
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|op| *op == "cat")
                        .count(),
                    reads
                );
                assert!(!reused.projection()["inference_ready"].as_bool().unwrap());
                serde_json::json!({"elapsed_preparation_ms":elapsed_ms,"content_reads":reads,
                    "provider_requests":test_backend.calls.lock().unwrap().len(),"total_bytes":record.total_bytes,
                    "index_bytes":record.index_bytes,"reserved_bytes":record.reserved_bytes,
                    "seed_disk":seed_disk,"backend_before":backend_before,"backend_sampled_peak":backend_peak,
                    "staging_sampled_peak":staged_peak,"admitted_disk":admitted_disk,"backend_after":disk_bytes(&test_repo),
                    "reuse_content_reads":0,"inference_ready":false})
            });
            let outcome =
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut work).await;
            owner.stopping.store(true, Ordering::Release);
            if outcome.is_err() {
                work.abort();
                let _ = work.await;
            }
            // Preserve the bridge's normal shutdown/reap contract on success,
            // assertion failure and timeout before deleting this isolated root.
            let shutdown = bridge.shutdown().await;
            let pending = owner.worker.lock().unwrap().take();
            if let Some(mut pending) = pending {
                if tokio::time::timeout(Duration::from_secs(5), &mut pending)
                    .await
                    .is_err()
                {
                    pending.abort();
                    let _ = pending.await;
                }
            }
            drop(owner);
            drop(registry);
            drop(backend);
            drop(bridge);
            daemon.0.kill().unwrap();
            daemon.0.wait().unwrap();
            drop(daemon);
            drop(dir);
            drop(stdout);
            drop(stderr);
            root.close().unwrap();
            assert!(!root_path.exists());
            shutdown.expect("native bridge shutdown and reap");
            let mut receipt = outcome
                .expect("production preparation deadline")
                .expect("production preparation task");
            receipt["fixture_cleanup"] = serde_json::json!(true);
            receipt["native_shutdown_reap"] = serde_json::json!(true);
            receipt["kubo_reaped"] = serde_json::json!(true);
            receipt["proof_limit"] = serde_json::json!("Isolated signed synthetic package from seeded offline Kubo; actual Content/native Use/admission/reuse. Allocation observations are samples, not continuous peak; extra seed copy is fixture-only. Cold-network, exact Qwen and inference remain open.");
            println!("{receipt}");
        }
    }

    #[tokio::test]
    async fn model_preparation_restart_cancel_drains_before_cleanup_and_release() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), false).await;
        backend.hold_drain.store(true, Ordering::Release);
        // Cleanup remains possible after local-use revocation.
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"] = serde_json::Value::Null
        });
        let owner = PreparationOwner::default();
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("cancel")),
                "cancel",
                &serde_json::json!({"operation_id":record.operation_id}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            backend.entered.notified(),
        )
        .await
        .unwrap();
        let pending = load_operation(root.path(), &record.operation_id).unwrap();
        assert!(pending.cancel_requested);
        assert_eq!(pending.reserved_bytes, record.reserved_bytes);
        assert!(root
            .path()
            .join("model-preparation/stage/weights.gguf")
            .exists());
        backend.release.notify_one();
        join_worker(&owner).await;
        let terminal = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(terminal.state, PreparationState::Cancelled);
        assert_eq!(terminal.reserved_bytes, 0);
        assert!(!root.path().join("model-preparation/stage").exists());
        assert_eq!(*backend.calls.lock().unwrap(), ["runtime_prepare_backend"]);
    }

    #[tokio::test]
    async fn model_preparation_failed_drain_keeps_charge_until_exact_cancel_retry() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), false).await;
        backend.fail_drain.store(true, Ordering::Release);
        let owner = PreparationOwner::default();
        let input = serde_json::json!({"operation_id":record.operation_id});
        owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("cancel")),
                "cancel",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let uncertain = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(uncertain.state, PreparationState::Uncertain);
        assert_eq!(uncertain.reserved_bytes, record.reserved_bytes);
        assert!(root
            .path()
            .join("model-preparation/stage/weights.gguf")
            .exists());
        backend.fail_drain.store(false, Ordering::Release);
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("cancel")),
                "retry",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let cancelled = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(cancelled.state, PreparationState::Cancelled);
        assert_eq!(cancelled.reserved_bytes, 0);
        assert!(!root.path().join("model-preparation/stage").exists());
    }

    #[tokio::test]
    async fn model_preparation_renamed_receipt_requires_current_authority_after_hash() {
        let (root, record, backend, registry) = staged_fixture(1, true).await;
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let guard: Revalidate = Arc::new(move || {
            ensure!(
                checks.fetch_add(1, Ordering::AcqRel) < 2,
                "fixture authority revoked"
            );
            Ok(())
        });
        let owner = PreparationOwner::default();
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("status")),
                "status",
                &serde_json::json!({"operation_id":record.operation_id}),
                guard,
            )
            .unwrap();
        join_worker(&owner).await;
        let uncertain = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(uncertain.state, PreparationState::Uncertain);
        assert_eq!(uncertain.reserved_bytes, record.reserved_bytes);
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|op| *op == "runtime_hash_staged_directory")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn model_preparation_restart_status_does_not_admit_expired_staging() {
        let (root, record, backend, registry) = staged_fixture(1, false).await;
        let owner = PreparationOwner::default();
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("status")),
                "status",
                &serde_json::json!({"operation_id":record.operation_id}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let terminal = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(terminal.state, PreparationState::Expired);
        assert_eq!(terminal.reserved_bytes, 0);
        assert!(!root.path().join("model-preparation/stage").exists());
        assert_eq!(*backend.calls.lock().unwrap(), ["runtime_prepare_backend"]);
    }

    #[tokio::test]
    async fn model_preparation_restart_reconciles_expired_rename_after_exact_hash() {
        let (root, record, backend, registry) = staged_fixture(1, true).await;
        let owner = PreparationOwner::default();
        let input = serde_json::json!({"operation_id":record.operation_id});
        let wrong_cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
        *backend.cid.lock().unwrap() = wrong_cid.into();
        owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("status")),
                "status",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let uncertain = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(uncertain.state, PreparationState::Uncertain);
        assert_eq!(uncertain.reserved_bytes, record.reserved_bytes);
        *backend.cid.lock().unwrap() = record.package_cid.clone();
        // A later cancel cannot erase the already-renamed artifact; it settles
        // the exact prior effect after current authority and bytes are verified.
        owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("cancel")),
                "cancel",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let admitted = load_operation(root.path(), &record.operation_id).unwrap();
        assert_eq!(admitted.state, PreparationState::Admitted);
        assert_eq!(admitted.reserved_bytes, record.reserved_bytes);
        assert!(admitted.cancel_requested);
        assert!(Inventory::open(root.path(), false)
            .unwrap()
            .admitted(&record.operation_id)
            .is_ok());
    }

    #[tokio::test]
    async fn model_preparation_exact_cid_reuse_preserves_actor_request_isolation() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        let owner = PreparationOwner::default();
        owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("status")),
                "status",
                &serde_json::json!({"operation_id":record.operation_id}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        let mut other = context();
        other.principal_id = "person:other-fixture".into();
        for op in ["status", "cancel"] {
            assert!(owner
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(&other, &method(op)),
                    op,
                    &serde_json::json!({"operation_id":record.operation_id}),
                    Arc::new(|| Ok(()))
                )
                .is_err());
        }
        let input = serde_json::json!({"cid":record.package_cid});
        let response = owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&other, &method("use")),
                "fresh",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        let id = response["operation_id"].as_str().unwrap().to_owned();
        assert_ne!(id, record.operation_id);
        join_worker(&owner).await;
        let reused = load_operation(root.path(), &id).unwrap();
        assert_eq!(reused.state, PreparationState::Admitted);
        assert_eq!(reused.admission_id, record.operation_id);
        assert_eq!(reused.reserved_bytes, 0);
        assert_eq!(reused.request_binding.principal, other.principal_id);
        assert_eq!(reused.request_binding.request_id, "fresh");
        assert_eq!(
            load_operation(root.path(), &record.operation_id)
                .unwrap()
                .reserved_bytes,
            record.reserved_bytes
        );
        assert!(!root.path().join("model-preparation/stage").exists());
        let prior = backend.calls.lock().unwrap().len();
        let replay = owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&other, &method("use")),
                "fresh",
                &input,
                Arc::new(|| Ok(())),
            )
            .unwrap();
        assert_eq!(replay["operation_id"], id);
        assert_eq!(replay["admitted"], true);
        assert_eq!(replay["inference_ready"], false);
        assert_eq!(backend.calls.lock().unwrap().len(), prior);
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|op| *op == "runtime_hash_staged_directory")
                .count(),
            2
        );
    }

    #[test]
    fn model_preparation_grant_and_resource_bounds_fail_before_reservation() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!({"max_cache_bytes": 0, "max_model_memory_bytes": 8192}),
            serde_json::json!({"max_cache_bytes": u64::MAX, "max_model_memory_bytes": 8192}),
            serde_json::json!({"max_cache_bytes": 1.5, "max_model_memory_bytes": 8192}),
            serde_json::json!({"max_cache_bytes": "1024", "max_model_memory_bytes": 8192}),
            serde_json::json!({"max_cache_bytes": 1024, "max_model_memory_bytes": 0}),
            serde_json::json!({"max_cache_bytes": 1024, "max_model_memory_bytes": u64::MAX}),
            serde_json::json!({"max_cache_bytes": 1024, "max_model_memory_bytes": 8192, "actor": "system"}),
            serde_json::json!({"max_cache_bytes": 1, "max_model_memory_bytes": 16_u64 * 1024 * 1024 * 1024}),
            serde_json::json!({"max_cache_bytes": 1024 * 1024, "max_model_memory_bytes": 1}),
        ] {
            let (root, cid) = fixture();
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"] = value
            });
            let context = context();
            assert!(reserve(root.path(), &caller(&context, &method("use")), "req", &cid).is_err());
            assert!(!root.path().join("model-preparation").exists());
        }
    }

    #[test]
    fn model_preparation_rejects_unbounded_identity_and_undeclared_method() {
        let (root, cid) = fixture();
        let context = context();
        for request in [
            String::new(),
            "x".repeat(161),
            "../escape".into(),
            "with space".into(),
        ] {
            assert!(reserve(
                root.path(),
                &caller(&context, &method("use")),
                &request,
                &cid
            )
            .is_err());
        }
        for identity in [String::new(), "x".repeat(161), "principal\nother".into()] {
            let context = HomeLaunchTokenContext {
                principal_id: identity,
                ..context.clone()
            };
            assert!(reserve(root.path(), &caller(&context, &method("use")), "req", &cid).is_err());
        }
        for operation in ["status", "cancel", "install"] {
            assert!(reserve(
                root.path(),
                &caller(&context, &method(operation)),
                "req",
                &cid
            )
            .is_err());
        }
        let mut privileged = method("use");
        privileged.risk = AffordanceRisk::Privileged;
        assert!(reserve(root.path(), &caller(&context, &privileged), "req", &cid).is_err());
        assert!(reserve(
            root.path(),
            &caller(&context, &method("use")),
            "req",
            "../weights.gguf"
        )
        .is_err());
        assert!(!root.path().join("model-preparation").exists());
    }

    #[test]
    fn model_preparation_replay_and_cross_surface_management_preserve_gateway_binding() {
        let (root, cid) = fixture();
        let initial = context();
        let use_method = method("use");
        let caller = caller(&initial, &use_method);
        let record = reserve(root.path(), &caller, "request-1", &cid).unwrap();
        let newer = HomeLaunchTokenContext {
            session_id: "session-2".into(),
            grant_id: "grant-2".into(),
            ..initial.clone()
        };
        assert_eq!(
            reserve(
                root.path(),
                &PreparationCaller {
                    context: &newer,
                    ..caller
                },
                "request-1",
                &cid
            )
            .unwrap(),
            record
        );
        assert!(reserve(
            root.path(),
            &PreparationCaller {
                interface: "elastos.changed",
                ..caller
            },
            "request-1",
            &cid
        )
        .is_err());
        let alternate = "bafybeif6wztovlmfv73or7wyr62rp5spo5aqjgacdkol34bbkjdhvrbgdm";
        assert!(reserve(root.path(), &caller, "request-1", alternate).is_err());

        let status_method = method("status");
        let manager = PreparationCaller {
            context: &newer,
            capsule: "system",
            interface: "elastos.system.settings",
            method: &status_method,
        };
        assert_eq!(
            status(root.path(), &manager, &record.operation_id).unwrap(),
            record
        );
        let bytes = std::fs::read(root.path().join("model-preparation/state.json")).unwrap();
        let foreign = HomeLaunchTokenContext {
            principal_id: "person:other".into(),
            ..newer.clone()
        };
        assert!(status(
            root.path(),
            &PreparationCaller {
                context: &foreign,
                ..manager
            },
            &record.operation_id
        )
        .is_err());
        assert!(
            cancel(root.path(), &manager, &record.operation_id).is_err(),
            "read method cannot cancel"
        );
        let cancel_method = method("cancel");
        let manager = PreparationCaller {
            method: &cancel_method,
            ..manager
        };
        assert!(cancel(
            root.path(),
            &PreparationCaller {
                context: &foreign,
                ..manager
            },
            &record.operation_id
        )
        .is_err());
        assert_eq!(
            std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
            bytes
        );
        let terminal = cancel(root.path(), &manager, &record.operation_id).unwrap();
        assert_eq!(terminal.request_binding, record.request_binding);
        assert_eq!(terminal.state, PreparationState::Cancelled);
        assert_eq!(terminal.reserved_bytes, 0);
        let agent = PreparationCaller {
            capsule: "agent",
            interface: "elastos.agent.content",
            method: &status_method,
            ..manager
        };
        assert_eq!(
            status(root.path(), &agent, &record.operation_id).unwrap(),
            terminal
        );
    }

    #[test]
    fn model_preparation_restart_preserves_reservation_until_cancel_or_expiry() {
        let (root, cid) = fixture();
        let context = context();
        let use_method = method("use");
        let caller = caller(&context, &use_method);
        let record = reserve_at(root.path(), &caller, "req-1", &cid, 2).unwrap();
        assert_eq!(
            reserve_at(root.path(), &caller, "req-1", &cid, 3).unwrap(),
            record
        );
        assert!(reserve_at(root.path(), &caller, "req-2", &cid, 3).is_err());
        // A crash before atomic publication leaves only this uncommitted file.
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.path().join("model-preparation/state.next"))
            .unwrap()
            .write_all(b"incomplete")
            .unwrap();
        let reader_method = method("status");
        let reader = PreparationCaller {
            method: &reader_method,
            ..caller
        };
        assert_eq!(
            manage(root.path(), &reader, &record.operation_id, false, 3).unwrap(),
            record
        );
        assert!(!root.path().join("model-preparation/state.next").exists());
        let expired = manage(
            root.path(),
            &reader,
            &record.operation_id,
            false,
            record.expires_at,
        )
        .unwrap();
        assert_eq!(expired.state, PreparationState::Expired);
        assert_eq!(expired.reserved_bytes, 0);
        assert_eq!(expired.request_binding, record.request_binding);
        let next = reserve_at(root.path(), &caller, "req-2", &cid, record.expires_at).unwrap();
        assert_ne!(next.operation_id, record.operation_id);
    }

    #[test]
    fn model_preparation_record_bound_and_serialization_corruption_fail_closed() {
        let (root, cid) = fixture();
        let context = context();
        let use_method = method("use");
        let caller = caller(&context, &use_method);
        let record = reserve(root.path(), &caller, "req-1", &cid).unwrap();
        let state_path = root.path().join("model-preparation/state.json");
        let original = std::fs::read(&state_path).unwrap();
        for corrupt in [b"{".to_vec(), b"null".to_vec(), vec![b' '; 256 * 1024 + 1]] {
            std::fs::write(&state_path, &corrupt).unwrap();
            assert!(reserve(root.path(), &caller, "req-2", &cid).is_err());
            assert_eq!(std::fs::read(&state_path).unwrap(), corrupt);
        }
        for (field, value) in [
            ("reserved_bytes", serde_json::json!(0)),
            ("operation_id", serde_json::json!("x")),
            ("total_bytes", serde_json::json!(u64::MAX)),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            json["records"][0][field] = value;
            let corrupt = serde_json::to_vec(&json).unwrap();
            std::fs::write(&state_path, &corrupt).unwrap();
            assert!(reserve(root.path(), &caller, "req-2", &cid).is_err());
            assert_eq!(std::fs::read(&state_path).unwrap(), corrupt);
        }
        std::fs::write(&state_path, original).unwrap();
        let inventory = Inventory::open(root.path(), false).unwrap();
        let mut state = inventory.load().unwrap();
        state.records.clear();
        for number in 0..MAX_RECORDS {
            let mut terminal = record.clone();
            terminal.request_binding.request_id = format!("req-{number}");
            terminal.request_binding = binding(&terminal);
            terminal.operation_id = operation_id(&terminal).unwrap();
            terminal.admission_id = terminal.operation_id.clone();
            terminal.state = PreparationState::Cancelled;
            terminal.reserved_bytes = 0;
            state.records.push(terminal);
        }
        inventory.save(&state).unwrap();
        drop(inventory);
        let prior = std::fs::read(&state_path).unwrap();
        assert!(reserve(root.path(), &caller, "req-over-limit", &cid).is_err());
        assert_eq!(std::fs::read(&state_path).unwrap(), prior);
    }

    #[test]
    fn model_preparation_concurrent_reservations_do_not_overbook() {
        let (root, cid) = fixture();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let launch = |request_id| {
                let root = root.path();
                let cid = &cid;
                let barrier = &barrier;
                scope.spawn(move || {
                    let context = context();
                    let method = method("use");
                    barrier.wait();
                    reserve(root, &caller(&context, &method), request_id, cid)
                })
            };
            let first = launch("first");
            let second = launch("second");
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let state = Inventory::open(root.path(), false).unwrap().load().unwrap();
        assert_eq!(state.records.len(), 1);
        assert_eq!(
            state.records[0],
            *results
                .iter()
                .find_map(|result| result.as_ref().ok())
                .unwrap()
        );
    }

    #[test]
    fn model_preparation_process_worker() {
        let Some(path) = std::env::var_os("ELASTOS_TEST_PREPARATION_LOCK_ROOT") else {
            return;
        };
        let cid = std::env::var("ELASTOS_TEST_PREPARATION_LOCK_CID").unwrap();
        let context = context();
        let err = reserve(
            Path::new(&path),
            &caller(&context, &method("use")),
            "child",
            &cid,
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::WouldBlock),
            "another process must respect the held inventory lock: {err}"
        );
    }

    #[test]
    fn model_preparation_lock_is_process_safe() {
        let (root, cid) = fixture();
        let lock = Inventory::open(root.path(), true).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["model_preparation_process_worker", "--nocapture"])
            .env("ELASTOS_TEST_PREPARATION_LOCK_ROOT", root.path())
            .env("ELASTOS_TEST_PREPARATION_LOCK_CID", &cid)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        assert!(lock.load().unwrap().records.is_empty());
        drop(lock);
        let context = context();
        assert!(reserve(
            root.path(),
            &caller(&context, &method("use")),
            "after-child",
            &cid
        )
        .is_ok());
    }

    #[test]
    fn model_preparation_disk_floor_uses_checked_outstanding_bytes() {
        assert!(storage::require_space_floor(1000, 200, 100).is_ok());
        assert!(storage::require_space_floor(1000, 200, 101).is_err());
        assert!(storage::require_space_floor(1000, 200, 201).is_err());
        assert!(storage::require_space_floor(0, 0, 0).is_err());
        assert!(storage::require_space_floor(1000, 1001, 0).is_err());
        assert!(storage::require_space_floor(u128::MAX, u128::MAX, 0).is_err());
    }

    #[test]
    fn model_preparation_immutable_admission_facts_detect_valid_json_tampering() {
        let (root, cid) = fixture();
        let context = context();
        let use_method = method("use");
        let caller = caller(&context, &use_method);
        let record = reserve(root.path(), &caller, "req", &cid).unwrap();
        let path = root.path().join("model-preparation/state.json");
        let original = std::fs::read(&path).unwrap();
        let other_head = cid::Cid::new_v1(
            0x55,
            cid::multihash::Multihash::<64>::wrap(0x12, &[3; 32]).unwrap(),
        )
        .to_string();
        for change_head in [false, true] {
            let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            if change_head {
                json["records"][0]["catalog_head_cid"] = serde_json::json!(other_head);
            } else {
                json["records"][0]["total_bytes"] = serde_json::json!(record.total_bytes + 1);
                json["records"][0]["reserved_bytes"] = serde_json::json!(record.total_bytes + 1);
            }
            let corrupt = serde_json::to_vec(&json).unwrap();
            std::fs::write(&path, &corrupt).unwrap();
            assert!(reserve(root.path(), &caller, "req", &cid).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        }
        // Even an internally self-consistent rewritten size cannot replay the
        // original request against the freshly verified catalog entry.
        let mut rewritten = record;
        rewritten.total_bytes += 1;
        rewritten.reserved_bytes = preparation_charge(rewritten.total_bytes).unwrap();
        rewritten.operation_id = operation_id(&rewritten).unwrap();
        rewritten.admission_id = rewritten.operation_id.clone();
        let rewritten = PreparationInventory {
            schema: SCHEMA.into(),
            records: vec![rewritten],
        };
        rewritten.validate().unwrap();
        std::fs::write(&path, serde_json::to_vec(&rewritten).unwrap()).unwrap();
        assert!(reserve(root.path(), &caller, "req", &cid).is_err());
    }

    #[test]
    fn model_preparation_revocation_blocks_new_work_but_preserves_owner_cancellation() {
        let (root, cid) = fixture();
        let context = context();
        let record = reserve(root.path(), &caller(&context, &method("use")), "req", &cid).unwrap();
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"] = serde_json::Value::Null
        });
        std::fs::write(
            root.path().join(super::super::MODEL_CATALOG_FILE),
            b"unavailable catalog",
        )
        .unwrap();
        assert!(reserve(root.path(), &caller(&context, &method("use")), "new", &cid).is_err());
        assert_eq!(
            status(
                root.path(),
                &caller(&context, &method("status")),
                &record.operation_id
            )
            .unwrap(),
            record
        );
        assert_eq!(
            cancel(
                root.path(),
                &caller(&context, &method("cancel")),
                &record.operation_id
            )
            .unwrap()
            .state,
            PreparationState::Cancelled
        );
    }

    #[test]
    fn model_preparation_rejects_unsafe_directory_file_and_temporary_shapes() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};
        for mode in [
            "directory-symlink",
            "regular-directory",
            "readable",
            "writable",
        ] {
            let (root, cid) = fixture();
            let path = root.path().join("model-preparation");
            match mode {
                "directory-symlink" => symlink(root.path(), &path).unwrap(),
                "regular-directory" => std::fs::write(&path, b"preserve").unwrap(),
                _ => {
                    std::fs::create_dir(&path).unwrap();
                    std::fs::set_permissions(
                        &path,
                        std::fs::Permissions::from_mode(if mode == "readable" {
                            0o755
                        } else {
                            0o770
                        }),
                    )
                    .unwrap();
                }
            }
            let context = context();
            assert!(reserve(root.path(), &caller(&context, &method("use")), "req", &cid).is_err());
        }
        for name in ["state.json", "state.next", "lock"] {
            for shape in ["symlink", "hardlink", "fifo", "readable", "directory"] {
                let (root, cid) = fixture();
                let context = context();
                let record =
                    reserve(root.path(), &caller(&context, &method("use")), "req", &cid).unwrap();
                let directory = root.path().join("model-preparation");
                let path = directory.join(name);
                let original = std::fs::read(directory.join("state.json")).unwrap();
                let protected = root.path().join("protected-fixture");
                std::fs::write(&protected, b"preserve unrelated bytes").unwrap();
                std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
                if path.exists() {
                    std::fs::remove_file(&path).unwrap();
                }
                match shape {
                    "symlink" => symlink(&protected, &path).unwrap(),
                    "hardlink" => std::fs::hard_link(&protected, &path).unwrap(),
                    "fifo" => {
                        use std::os::unix::ffi::OsStrExt as _;
                        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
                    }
                    "readable" => {
                        std::fs::write(
                            &path,
                            if name == "state.json" {
                                original.as_slice()
                            } else {
                                b""
                            },
                        )
                        .unwrap();
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                            .unwrap();
                    }
                    _ => std::fs::create_dir(&path).unwrap(),
                }
                assert!(
                    status(
                        root.path(),
                        &caller(&context, &method("status")),
                        &record.operation_id
                    )
                    .is_err(),
                    "accepted {shape} at {name}"
                );
                assert_eq!(
                    std::fs::read(&protected).unwrap(),
                    b"preserve unrelated bytes"
                );
                if name != "state.json" {
                    assert_eq!(
                        std::fs::read(directory.join("state.json")).unwrap(),
                        original
                    );
                }
            }
        }
    }

    #[test]
    fn model_preparation_storage_failure_and_path_replacement_preserve_prior_state() {
        use std::os::unix::fs::symlink;
        for replacement in ["temporary", "lock", "directory", "state"] {
            let (root, cid) = fixture();
            let context = context();
            reserve(root.path(), &caller(&context, &method("use")), "req", &cid).unwrap();
            let path = root.path().join("model-preparation");
            let original = std::fs::read(path.join("state.json")).unwrap();
            let inventory = Inventory::open(root.path(), false).unwrap();
            let mut state = inventory.load().unwrap();
            state.records[0].state = PreparationState::Cancelled;
            state.records[0].reserved_bytes = 0;
            match replacement {
                "temporary" => std::fs::create_dir(path.join("state.next")).unwrap(),
                "lock" => {
                    std::fs::rename(path.join("lock"), path.join("previous-lock")).unwrap();
                    std::fs::write(path.join("lock"), b"replacement").unwrap();
                }
                "directory" => {
                    std::fs::rename(&path, root.path().join("previous-inventory")).unwrap();
                    symlink(root.path().join("previous-inventory"), &path).unwrap();
                }
                _ => {
                    std::fs::rename(path.join("state.json"), path.join("previous-state")).unwrap();
                    symlink(path.join("previous-state"), path.join("state.json")).unwrap();
                }
            }
            assert!(inventory.save(&state).is_err());
            assert_eq!(std::fs::read(path.join("state.json")).unwrap(), original);
        }
    }

    #[test]
    fn model_preparation_reserves_reopens_and_cancels_exactly_once() {
        let root = tempfile::tempdir().unwrap();
        let payload = super::super::tests::model_catalog_fixture();
        super::super::tests::write_model_catalog_fixture(root.path(), &payload);
        let config_path = root.path().join("components.json");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        config["model_catalog"]["local_use"] = serde_json::json!({
            "max_cache_bytes": 64 * 1024 * 1024,
            "max_model_memory_bytes": 16_u64 * 1024 * 1024 * 1024,
        });
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let entries = super::super::model_catalog_entries(root.path())
            .unwrap()
            .unwrap();
        let entry = &entries[0];
        let context = HomeLaunchTokenContext {
            principal_id: "person:preparation-fixture".into(),
            session_id: "preparation-session".into(),
            proof_binding_id: Some("preparation-proof".into()),
            grant_id: "preparation-grant".into(),
        };
        let capsule = "marketplace";
        let request_id = "prepare-fixture-1";
        let use_method = method("use");
        let status_method = method("status");
        let cancel_method = method("cancel");
        let caller = PreparationCaller {
            context: &context,
            capsule,
            interface: "elastos.marketplace.catalog",
            method: &use_method,
        };
        let reader = PreparationCaller {
            method: &status_method,
            ..caller
        };
        let canceller = PreparationCaller {
            method: &cancel_method,
            ..caller
        };

        let reserved = reserve(root.path(), &caller, request_id, &entry.cid)
            .expect("approved verified model must create a durable reservation");
        assert!(!reserved.operation_id.is_empty());
        assert_eq!(reserved.state, PreparationState::Reserved);
        assert_eq!(
            reserved.reserved_bytes,
            preparation_charge(entry.size_bytes).unwrap()
        );
        assert_eq!(reserved.package_cid, entry.cid);
        assert_eq!(
            reserved.catalog_head_cid,
            config["model_catalog"]["head_cid"].as_str().unwrap()
        );
        assert_eq!(reserved.request_binding.request_id, request_id);
        assert_eq!(reserved.request_binding.principal, context.principal_id);
        assert_eq!(reserved.request_binding.capsule, capsule);
        assert_eq!(
            reserved.request_binding,
            esp_request_binding(
                request_id,
                &context.principal_id,
                capsule,
                Some(caller.interface),
                &use_method.id,
                use_method.resource.iter().cloned(),
                &serde_json::json!({"cid": entry.cid}),
            )
        );
        assert_eq!(
            status(root.path(), &reader, &reserved.operation_id).unwrap(),
            reserved,
            "a new read must reopen the same durable reservation"
        );
        assert_eq!(
            reserve(root.path(), &caller, request_id, &entry.cid).unwrap(),
            reserved,
            "exact request replay must not reserve twice"
        );

        let cancelled = cancel(root.path(), &canceller, &reserved.operation_id).unwrap();
        assert_eq!(cancelled.state, PreparationState::Cancelled);
        assert_eq!(cancelled.reserved_bytes, 0);
        assert_eq!(cancelled.operation_id, reserved.operation_id);
        assert_eq!(cancelled.request_binding, reserved.request_binding);
        assert_eq!(
            cancel(root.path(), &canceller, &reserved.operation_id).unwrap(),
            cancelled,
            "cancellation must release the reservation exactly once"
        );
        assert_eq!(
            status(root.path(), &reader, &reserved.operation_id).unwrap(),
            cancelled,
            "the terminal cancellation must survive reopening"
        );
        assert!(!root.path().join("capsules").exists());
    }
}
