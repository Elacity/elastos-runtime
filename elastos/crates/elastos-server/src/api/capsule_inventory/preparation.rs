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
const CAPACITY_WINDOW_BYTES: u64 = 1024 * 1024;

// Static checkpoints retain a useful cause after cleanup without retaining
// provider text, host paths, credentials or media bytes in the inventory.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PreparationFailurePhase {
    Preparation,
    Authority,
    Policy,
    Capacity,
    MetadataRead,
    MetadataWrite,
    MetadataSync,
    MetadataIntegrity,
    WeightsRead,
    WeightsHeader,
    WeightsWrite,
    WeightsSync,
    WeightsIntegrity,
}

impl std::fmt::Display for PreparationFailurePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PreparationFailurePhase {}

impl PreparationFailurePhase {
    fn public_class(self) -> &'static str {
        match self {
            Self::Authority => "authorization_unavailable",
            Self::Policy => "policy_unavailable",
            Self::Capacity => "capacity_unavailable",
            Self::MetadataRead | Self::WeightsRead => "content_unavailable",
            Self::MetadataWrite | Self::MetadataSync | Self::WeightsWrite | Self::WeightsSync => {
                "local_storage_unavailable"
            }
            Self::MetadataIntegrity | Self::WeightsHeader | Self::WeightsIntegrity => {
                "verification_failed"
            }
            Self::Preparation => "preparation_unavailable",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PreparationState {
    CapacityPending,
    Reserved,
    Preparing,
    Verifying,
    AdmissionPending,
    Admitted,
    Reclaimed,
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
    activation_pending: bool,
    admission_id: String,
    #[serde(default)]
    activation: Option<ModelActivation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_phase: Option<PreparationFailurePhase>,
    created_at: u64,
    expires_at: u64,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RetentionClaim {
    principal: String,
    cid: String,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct PreparationInventory {
    schema: String,
    records: Vec<PreparationRecord>,
    retention_claims: Vec<RetentionClaim>,
    #[serde(default)]
    retirement: Option<ModelRetirement>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ModelActivation {
    offer: serde_json::Value,
    entrypoint: String,
    engine_receipt_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RetirementPhase {
    WithdrawalPending,
    Withdrawn,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ModelRetirement {
    operation_id: String,
    admission_id: String,
    phase: RetirementPhase,
}

impl Default for PreparationInventory {
    fn default() -> Self {
        Self {
            schema: SCHEMA.into(),
            records: Vec::new(),
            retention_claims: Vec::new(),
            retirement: None,
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
            self.schema == SCHEMA
                && self.records.len() <= MAX_RECORDS
                && self.retention_claims.len() <= MAX_RECORDS,
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
            if let Some(activation) = &record.activation {
                ensure!(
                    !shared
                        && matches!(
                            record.state,
                            PreparationState::Admitted | PreparationState::Reclaimed
                        ),
                    "activation descriptor has no admitted owner"
                );
                activation.validate(record)?;
            }
            if record.state == PreparationState::Admitted && !record.activation_pending {
                ensure!(
                    self.records
                        .iter()
                        .any(|owner| owner.operation_id == record.admission_id
                            && owner.activation.is_some()),
                    "activated admission has no original binding"
                );
            }
            if record.state == PreparationState::Reclaimed && !shared {
                ensure!(
                    record.activation.is_some(),
                    "reclaimed owner has no activation evidence"
                );
            }
            if shared {
                ensure!(
                    self.records
                        .iter()
                        .any(|r| r.operation_id == record.admission_id
                            && r.admission_id == r.operation_id
                            && r.package_cid == record.package_cid
                            && (r.state == PreparationState::Admitted
                                || (matches!(
                                    record.state,
                                    PreparationState::Reclaimed
                                        | PreparationState::Cancelled
                                        | PreparationState::Expired
                                        | PreparationState::Failed
                                ) && r.state == PreparationState::Reclaimed))),
                    "invalid shared admission"
                );
            }
            let expected = if record.state == PreparationState::CapacityPending {
                active += 1;
                ensure!(
                    record.index_bytes == 0
                        && record.completed_bytes == 0
                        && (!record.cancel_requested
                            || self
                                .retirement
                                .as_ref()
                                .is_some_and(|r| r.operation_id == record.operation_id)),
                    "capacity-pending preparation has effects"
                );
                0
            } else if record.state == PreparationState::Reserved {
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
        if let Some(retirement) = &self.retirement {
            ensure!(
                self.records
                    .iter()
                    .any(|record| record.operation_id == retirement.operation_id
                        && record.state == PreparationState::CapacityPending)
                    && self.records.iter().any(|record| record.operation_id
                        == retirement.admission_id
                        && record.admission_id == record.operation_id
                        && record.state == PreparationState::Admitted
                        && record.activation.is_some()),
                "invalid model retirement ownership"
            );
        }
        let mut claims = BTreeSet::new();
        for claim in &self.retention_claims {
            ensure!(
                bounded_id(&claim.principal, 160)
                    && canonical_cid(&claim.cid, 0x70)
                    && claims.insert((&claim.principal, &claim.cid))
                    && self.records.iter().any(|record| record.backs_retention()
                        && record.request_binding.principal == claim.principal
                        && record.package_cid == claim.cid),
                "invalid retention claim"
            );
        }
        Ok(())
    }

    fn kept(&self, principal: &str, cid: &str) -> bool {
        self.retention_claims
            .iter()
            .any(|claim| claim.principal == principal && claim.cid == cid)
    }

    fn prune_retention(&mut self) -> bool {
        let before = self.retention_claims.len();
        let records = &self.records;
        self.retention_claims.retain(|claim| {
            records.iter().any(|record| {
                record.backs_retention()
                    && record.request_binding.principal == claim.principal
                    && record.package_cid == claim.cid
            })
        });
        self.retention_claims.len() != before
    }

    fn expire(&mut self, now: u64) -> bool {
        // Pre-dispatch records have no provider work. Active work must
        // settle and drain before expiry can release its byte charge.
        let mut changed = false;
        for record in &mut self.records {
            if record.pre_dispatch()
                && now >= record.expires_at
                && self
                    .retirement
                    .as_ref()
                    .is_none_or(|r| r.operation_id != record.operation_id)
            {
                record.state = PreparationState::Expired;
                record.reserved_bytes = 0;
                changed = true;
            }
        }
        self.prune_retention() || changed
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
    // Reclaimed requests are immutable history, even after catalog rotation.
    // They cannot become a fresh preparation or reactivate their old offer.
    match std::fs::symlink_metadata(data_dir.join("model-preparation")) {
        Ok(_) => {
            let inventory = Inventory::open(data_dir, false)?;
            let state = inventory.load()?;
            if let Some(record) = state.records.iter().find(|r| {
                (r.state == PreparationState::Reclaimed
                    || (matches!(
                        r.state,
                        PreparationState::Cancelled
                            | PreparationState::Expired
                            | PreparationState::Failed
                    ) && state.records.iter().any(|owner| {
                        owner.operation_id == r.admission_id
                            && owner.state == PreparationState::Reclaimed
                    })))
                    && r.request_binding.principal == caller.context.principal_id
                    && r.request_binding.capsule == caller.capsule
                    && r.request_binding.request_id == request_id
            }) {
                ensure!(
                    record.package_cid == cid
                        && record.request_binding
                            == esp_request_binding(
                                request_id,
                                &caller.context.principal_id,
                                caller.capsule,
                                Some(caller.interface),
                                &caller.method.id,
                                [RESOURCE.to_owned()],
                                &serde_json::json!({"cid":cid})
                            ),
                    "reclaimed preparation request changed"
                );
                return Ok(record.clone());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
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
            .any(|record| record.pre_dispatch() || record.active()),
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
    let capacity_pending =
        reserved_bytes > grant.max_cache_bytes || !inventory.space_fits(initial_charge)?;
    // Existing admissions already occupy disk. Charge only this new reservation
    // again against observed free space, while quota above counts every artifact.
    inventory.require_space(if capacity_pending { 0 } else { initial_charge })?;
    let mut record = PreparationRecord {
        activation_pending: true,
        operation_id: String::new(),
        request_binding,
        catalog_head_cid: trust.head_cid,
        package_cid: cid.into(),
        state: if capacity_pending {
            PreparationState::CapacityPending
        } else {
            PreparationState::Reserved
        },
        total_bytes: entry.size_bytes,
        reserved_bytes: if capacity_pending { 0 } else { initial_charge },
        index_bytes: 0,
        completed_bytes: 0,
        cancel_requested: false,
        admission_id: String::new(),
        activation: None,
        failure_phase: None,
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

fn set_retention(
    data_dir: &Path,
    caller: &PreparationCaller<'_>,
    request_id: &str,
    cid: &str,
    keep: bool,
    revalidate: &Revalidate,
) -> anyhow::Result<serde_json::Value> {
    authorize(caller, "retention")?;
    ensure!(
        bounded_id(request_id, 160) && canonical_cid(cid, 0x70),
        "invalid retention input"
    );
    let inventory = Inventory::open(data_dir, false)?;
    let mut state = inventory.load()?;
    let principal = &caller.context.principal_id;
    let record = state
        .records
        .iter()
        .filter(|record| {
            record.backs_retention()
                && record.request_binding.principal == *principal
                && record.package_cid == cid
        })
        .max_by_key(|record| record.state == PreparationState::Admitted)
        .context("retention preparation unavailable")?;
    let admitted = record.state == PreparationState::Admitted;
    if admitted {
        inventory.admitted(&record.admission_id)?.check()?;
    } else if keep && !state.kept(principal, cid) {
        ensure!(
            !record.cancel_requested && now()? < record.expires_at,
            "retention preparation is settling"
        );
    }
    ensure!(
        state
            .retirement
            .as_ref()
            .is_none_or(|r| r.admission_id != record.admission_id),
        "model retirement pending"
    );
    // Desired local retention is independent of model-use permission. Pending
    // intent survives reconciliation and admission, but ends when its last
    // operation settles without admission. It does not pin or fetch content.
    // Aliases share one principal/CID claim; release does not evict bytes/offers.
    if state.kept(principal, cid) != keep {
        if keep {
            ensure!(
                state.retention_claims.len() < MAX_RECORDS,
                "retention claims full"
            );
            state.retention_claims.push(RetentionClaim {
                principal: principal.clone(),
                cid: cid.into(),
            });
        } else {
            state
                .retention_claims
                .retain(|claim| claim.principal != *principal || claim.cid != cid);
        }
        revalidate()?;
        inventory.save(&state)?;
    }
    Ok(serde_json::json!({"cid":cid,"kept":keep,"admitted":admitted}))
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
    let retiring = state
        .retirement
        .as_ref()
        .is_some_and(|r| r.operation_id == operation_id);
    let record = &mut state.records[index];
    if cancel && record.pre_dispatch() && !retiring {
        record.state = PreparationState::Cancelled;
        record.reserved_bytes = 0;
        changed = true;
    } else if cancel && (record.active() || retiring) {
        record.cancel_requested = true;
        changed = true;
    }
    let result = record.clone();
    changed |= state.prune_retention();
    if changed {
        inventory.save(&state)?;
    }
    Ok(result)
}

impl PreparationRecord {
    fn backs_retention(&self) -> bool {
        self.state == PreparationState::Admitted || self.pre_dispatch() || self.active()
    }

    fn pre_dispatch(&self) -> bool {
        matches!(
            self.state,
            PreparationState::CapacityPending | PreparationState::Reserved
        )
    }

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
            "admitted":self.state == PreparationState::Admitted,
            "activation_pending":self.state == PreparationState::Admitted && self.activation_pending,
            "failure_class":self.failure_phase.map(PreparationFailurePhase::public_class)})
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
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Retention {
            cid: String,
            keep: bool,
        }
        revalidate()?;
        let record = match caller.method.operation.as_deref() {
            Some("retention") => {
                let input: Retention = serde_json::from_value(input.clone())?;
                return set_retention(
                    data_dir,
                    &caller,
                    request_id,
                    &input.cid,
                    input.keep,
                    &revalidate,
                );
            }
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
        let kept = Inventory::open(data_dir, false)?
            .load()?
            .kept(&caller.context.principal_id, &record.package_cid);
        let project = |record: &PreparationRecord| {
            let mut value = record.projection();
            value["kept"] = serde_json::json!(kept);
            value
        };
        let starting = record.pre_dispatch() && caller.method.operation.as_deref() == Some("use");
        let capacity_resume = record.state == PreparationState::CapacityPending;
        let activation_retry = record.state == PreparationState::Admitted
            && caller.method.operation.as_deref() == Some("use");
        if !record.active() && !starting && !activation_retry && !capacity_resume {
            return Ok(project(&record));
        }
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("preparation owner unavailable"))?;
        if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Ok(project(&record));
        }
        let registry = registry.context("preparation backend unavailable")?;
        let inventory = Inventory::open(data_dir, false)?;
        let worker_lock = inventory.worker_lock()?;
        let mut snapshot = inventory.load()?;
        if starting || capacity_resume {
            current_entry(data_dir, &record)?;
        }
        let shared = snapshot
            .records
            .iter()
            .find(|r| {
                r.operation_id == record.admission_id && r.operation_id != record.operation_id
            })
            .cloned();
        let mut capacity_pending = false;
        if starting || capacity_resume {
            let pending = snapshot
                .records
                .iter()
                .find(|r| r.operation_id == record.operation_id)
                .context("preparation disappeared")?;
            if pending.state == PreparationState::CapacityPending {
                let charge = if pending.admission_id == pending.operation_id {
                    preparation_charge(pending.total_bytes)?
                } else {
                    0
                };
                capacity_pending = snapshot.retirement.is_some()
                    || !cache_budget_fits(data_dir, &snapshot, charge)?
                    || !inventory.space_fits(charge)?;
                if capacity_pending {
                    if snapshot.retirement.is_none()
                        && retirement_candidate(data_dir, &snapshot)?.is_none()
                    {
                        return Ok(project(pending));
                    }
                } else {
                    // Pending intent acquires its full charge only under the existing
                    // worker lock, after both current quota and real headroom fit.
                    inventory.require_space(charge)?;
                    let pending = snapshot
                        .records
                        .iter_mut()
                        .find(|r| r.operation_id == record.operation_id)
                        .context("preparation disappeared")?;
                    pending.state = PreparationState::Reserved;
                    pending.reserved_bytes = charge;
                }
            }
        }
        let current = snapshot
            .records
            .iter_mut()
            .find(|r| r.operation_id == record.operation_id)
            .context("preparation disappeared")?;
        let starting = (starting || capacity_resume) && current.state == PreparationState::Reserved;
        if !starting && !current.active() && !activation_retry && !capacity_pending {
            return Ok(project(current));
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
            let starting = if capacity_pending {
                match prepare_capacity(
                    &data,
                    &registry,
                    &operation,
                    &stopping,
                    &revalidate,
                    &worker_lock,
                )
                .await
                {
                    Ok(true) => true,
                    Ok(false) => return,
                    Err(error) => {
                        // A failed withdrawal remains bound to this victim and
                        // keeps its bytes/charge. Generic preparation cleanup
                        // cannot settle or discard that effect record.
                        tracing::debug!(?error, "private model retirement pending");
                        return;
                    }
                }
            } else {
                starting
            };
            let result = if activation_retry {
                Ok(())
            } else if starting {
                prepare(&data, &registry, &operation, &stopping, &revalidate).await
            } else {
                reconcile(&data, &registry, &operation, &stopping, &revalidate).await
            };
            if let Err(error) = result {
                let phase = error
                    .downcast_ref::<PreparationFailurePhase>()
                    .copied()
                    .unwrap_or(PreparationFailurePhase::Preparation);
                let recorded = update_operation(&data, &operation, |record| {
                    record.failure_phase.get_or_insert(phase);
                })
                .is_ok();
                // The serialized local barrier is required even after transport
                // failure. Failed drain preserves all staged bytes and charges.
                let drained = registry.prepare_local_ipfs_backend().await.is_ok();
                let settled = settle_failure(&data, &operation, drained).is_ok();
                use elastos_runtime::provider::ProviderError;
                let provider_error_kind =
                    error
                        .downcast_ref::<ProviderError>()
                        .map(|error| match error {
                            ProviderError::NotFound(_) => "NotFound",
                            ProviderError::PermissionDenied(_) => "PermissionDenied",
                            ProviderError::InvalidUri(_) => "InvalidUri",
                            ProviderError::Provider(_) => "Provider",
                            ProviderError::NoProvider(_) => "NoProvider",
                            ProviderError::Unavailable(_) => "Unavailable",
                            ProviderError::Io(_) => "Io",
                        });
                tracing::warn!(operation_id = %operation, ?phase, ?provider_error_kind,
                    io_error_kind = ?error.downcast_ref::<std::io::Error>().map(std::io::Error::kind),
                    recorded, drained, settled, "private model preparation stopped");
            }
            // Admission and activation share this exact inventory worker guard.
            // Short snapshot locks remain available during provider I/O.
            if let Err(error) = activate_admitted_model(
                &data,
                &registry,
                &operation,
                &stopping,
                &revalidate,
                &worker_lock,
            )
            .await
            {
                tracing::debug!(?error, "private model activation pending");
            }
            drop(worker_lock);
        }));
        Ok(project(&current))
    }
}

async fn activate_admitted_model(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    operation: &str,
    stopping: &AtomicBool,
    revalidate: &Revalidate,
    worker: &std::fs::File,
) -> anyhow::Result<()> {
    let record = load_operation(data_dir, operation)?;
    if record.state != PreparationState::Admitted {
        return Ok(());
    }
    // This flag is an activation attempt receipt, never inference authority.
    set_activation_pending(data_dir, operation, true)?;
    revalidate()?;
    ensure!(
        !stopping.load(Ordering::Acquire),
        "model activation stopped"
    );
    current_entry(data_dir, &record)?;
    let mut config = crate::api::model_provider_bridge_config(data_dir)?;
    append_admitted_model_offers_locked(data_dir, registry, &mut config, worker).await?;
    preserve_active_model_offers(data_dir, registry, &mut config).await?;
    revalidate()?;
    ensure!(
        !stopping.load(Ordering::Acquire),
        "model activation stopped"
    );
    registry.refresh_local_model_configuration(&config).await?;
    set_activation_pending(data_dir, operation, false)
}

async fn preserve_active_model_offers(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    config: &mut elastos_runtime::provider::BridgeProviderConfig,
) -> anyhow::Result<()> {
    let state = Inventory::open(data_dir, false)?.load()?;
    ensure!(state.retirement.is_none(), "retirement owns model refresh");
    let missing: Vec<_> = state
        .records
        .iter()
        .filter(|r| {
            r.operation_id == r.admission_id
                && r.state == PreparationState::Admitted
                && r.activation.as_ref().is_some_and(|a| {
                    !config.extra["offers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|o| o["id"] == a.offer["id"])
                })
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let live = registry.local_model_offers().await?;
    for owner in missing {
        let activation = owner.activation.as_ref().unwrap();
        let found: Vec<_> = live
            .iter()
            .filter(|o| o["id"] == activation.offer["id"])
            .collect();
        if found.is_empty() {
            continue;
        }
        ensure!(
            found.len() == 1 && *found[0] == activation.summary(),
            "active model binding changed"
        );
        activation.check_root(data_dir, owner)?;
        let offers = config.extra["offers"].as_array_mut().unwrap();
        ensure!(offers.len() < 64, "model offer count exceeds bound");
        offers.push(activation.offer.clone());
        config.extra["runtime_admitted_offers"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"offer_id":activation.offer["id"]}));
    }
    Ok(())
}

fn set_activation_pending(data_dir: &Path, operation: &str, pending: bool) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    let mut snapshot = inventory.load()?;
    let record = snapshot
        .records
        .iter_mut()
        .find(|r| r.operation_id == operation)
        .context("model admission unavailable")?;
    ensure!(
        record.state == PreparationState::Admitted,
        "model admission unavailable"
    );
    ensure!(
        snapshot
            .retirement
            .as_ref()
            .is_none_or(|r| r.admission_id != record.admission_id),
        "model retirement owns activation"
    );
    record.activation_pending = pending;
    inventory.save(&snapshot)
}

fn require_cache_budget(data_dir: &Path, state: &PreparationInventory) -> anyhow::Result<()> {
    ensure!(
        cache_budget_fits(data_dir, state, 0)?,
        "cache budget unavailable"
    );
    Ok(())
}

fn cache_budget_fits(
    data_dir: &Path,
    state: &PreparationInventory,
    additional: u64,
) -> anyhow::Result<bool> {
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
        .try_fold(additional, |sum, r| sum.checked_add(r.reserved_bytes))
        .context("cache accounting overflow")?;
    Ok(total <= grant.max_cache_bytes)
}

fn retirement_protected(
    data_dir: &Path,
    state: &PreparationInventory,
    owner: &PreparationRecord,
) -> anyhow::Result<bool> {
    if state
        .retention_claims
        .iter()
        .any(|claim| claim.cid == owner.package_cid)
        || state.records.iter().any(|record| {
            record.admission_id == owner.operation_id
                && record.operation_id != owner.operation_id
                && (record.pre_dispatch() || record.active())
        })
    {
        return Ok(true);
    }
    let directory = data_dir
        .canonicalize()?
        .join("model-preparation")
        .join(format!("admitted-{}", owner.operation_id));
    let operator = crate::api::model_provider_bridge_config(data_dir)?;
    for offer in operator.extra["offers"]
        .as_array()
        .context("operator offers unavailable")?
    {
        if owner
            .activation
            .as_ref()
            .is_some_and(|a| a.offer["id"] == offer["id"])
        {
            return Ok(true);
        }
        for artifact in ["engine", "model"] {
            if let Some(path) = offer["adapter"][artifact]["path"].as_str() {
                if Path::new(path).starts_with(&directory) {
                    return Ok(true);
                }
                // Unavailable operator path identity is protection, not evidence
                // that the operator has released an artifact.
                match Path::new(path).canonicalize() {
                    Ok(path) if !path.starts_with(&directory) => {}
                    _ => return Ok(true),
                }
            }
        }
    }
    Ok(false)
}

fn retirement_candidate(
    data_dir: &Path,
    state: &PreparationInventory,
) -> anyhow::Result<Option<String>> {
    let mut owners: Vec<_> = state
        .records
        .iter()
        .filter(|record| {
            record.operation_id == record.admission_id
                && record.state == PreparationState::Admitted
                && record.activation.is_some()
        })
        .collect();
    owners.sort_by_key(|record| (record.created_at, &record.operation_id));
    for owner in owners {
        if !retirement_protected(data_dir, state, owner)? {
            return Ok(Some(owner.operation_id.clone()));
        }
    }
    Ok(None)
}

fn withdrawal_config(
    data_dir: &Path,
    state: &PreparationInventory,
    retirement: &ModelRetirement,
    live: &[serde_json::Value],
) -> anyhow::Result<elastos_runtime::provider::BridgeProviderConfig> {
    let mut config = crate::api::model_provider_bridge_config(data_dir)?;
    let offers = config.extra["offers"]
        .as_array_mut()
        .context("operator offers unavailable")?;
    let mut ids = BTreeSet::new();
    let mut admitted = Vec::new();
    for public in live {
        let id = public["id"]
            .as_str()
            .context("live model identity unavailable")?;
        ensure!(ids.insert(id), "duplicate live model identity");
        if offers.iter().any(|offer| offer["id"] == id) {
            continue;
        }
        let owner = state
            .records
            .iter()
            .find(|r| {
                r.operation_id == r.admission_id
                    && r.state == PreparationState::Admitted
                    && r.activation.as_ref().is_some_and(|a| a.offer["id"] == id)
            })
            .context("live model has no exact activation owner")?;
        let activation = owner.activation.as_ref().unwrap();
        activation.check_root(data_dir, owner)?;
        ensure!(
            activation.summary() == *public,
            "live activation evidence changed"
        );
        if owner.operation_id != retirement.admission_id {
            offers.push(activation.offer.clone());
            admitted.push(serde_json::json!({"offer_id":id}));
        }
    }
    config.extra["runtime_admitted_offers"] = serde_json::json!(admitted);
    Ok(config)
}

fn finish_model_retirement(data_dir: &Path) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    let mut state = inventory.load()?;
    let retirement = state.retirement.clone().context("retirement unavailable")?;
    ensure!(
        retirement.phase == RetirementPhase::Withdrawn,
        "withdrawal is unconfirmed"
    );
    let owner = state
        .records
        .iter()
        .find(|r| r.operation_id == retirement.admission_id)
        .context("retirement owner unavailable")?;
    ensure!(
        !retirement_protected(data_dir, &state, owner)?,
        "retirement owner is protected"
    );
    owner
        .activation
        .as_ref()
        .context("activation unavailable")?
        .check_root(data_dir, owner)?;
    inventory.remove_admitted(&retirement.admission_id)?;
    for record in &mut state.records {
        if record.admission_id == retirement.admission_id
            && record.state == PreparationState::Admitted
        {
            record.state = PreparationState::Reclaimed;
            record.reserved_bytes = 0;
            record.activation_pending = false;
        }
    }
    state.retirement = None;
    // Cancellation/expiry may settle only after the unresolved effect is gone.
    if let Some(record) = state
        .records
        .iter_mut()
        .find(|r| r.operation_id == retirement.operation_id)
    {
        if record.cancel_requested {
            record.state = PreparationState::Cancelled;
        }
    }
    state.expire(now()?);
    inventory.save(&state)
}

fn mark_model_withdrawn(data_dir: &Path, expected: &ModelRetirement) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    let mut state = inventory.load()?;
    ensure!(
        state.retirement.as_ref() == Some(expected),
        "retirement ownership changed"
    );
    state.retirement.as_mut().unwrap().phase = RetirementPhase::Withdrawn;
    inventory.save(&state)
}

async fn prepare_capacity(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    operation: &str,
    stopping: &AtomicBool,
    revalidate: &Revalidate,
    _worker: &std::fs::File,
) -> anyhow::Result<bool> {
    for _ in 0..=MAX_RECORDS {
        revalidate()?;
        ensure!(
            !stopping.load(Ordering::Acquire),
            "model preparation stopped"
        );
        let live = registry.local_model_offers().await?;
        let retirement = {
            let inventory = Inventory::open(data_dir, false)?;
            let mut state = inventory.load()?;
            let record = state
                .records
                .iter()
                .find(|r| r.operation_id == operation)
                .context("pending preparation unavailable")?;
            if record.state != PreparationState::CapacityPending {
                return Ok(false);
            }
            if let Some(retirement) = &state.retirement {
                ensure!(
                    retirement.operation_id == operation,
                    "another retirement owns worker"
                );
                retirement.clone()
            } else {
                if record.cancel_requested || now()? >= record.expires_at {
                    return Ok(false);
                }
                current_entry(data_dir, record)?;
                let charge = if record.operation_id == record.admission_id {
                    preparation_charge(record.total_bytes)?
                } else {
                    0
                };
                if cache_budget_fits(data_dir, &state, charge)? && inventory.space_fits(charge)? {
                    let record = state
                        .records
                        .iter_mut()
                        .find(|r| r.operation_id == operation)
                        .unwrap();
                    record.state = PreparationState::Preparing;
                    record.reserved_bytes = charge;
                    inventory.save(&state)?;
                    return Ok(true);
                }
                let Some(admission_id) = retirement_candidate(data_dir, &state)? else {
                    return Ok(false);
                };
                let owner = state
                    .records
                    .iter()
                    .find(|r| r.operation_id == admission_id)
                    .unwrap();
                let activation = owner.activation.as_ref().unwrap();
                activation.check_root(data_dir, owner)?;
                ensure!(
                    live.iter()
                        .filter(|offer| **offer == activation.summary())
                        .count()
                        == 1,
                    "retirement requires exact active provider evidence"
                );
                let retirement = ModelRetirement {
                    operation_id: operation.into(),
                    admission_id,
                    phase: RetirementPhase::WithdrawalPending,
                };
                state.retirement = Some(retirement.clone());
                inventory.save(&state)?;
                retirement
            }
        };
        if retirement.phase == RetirementPhase::WithdrawalPending {
            let config = {
                let inventory = Inventory::open(data_dir, false)?;
                let state = inventory.load()?;
                ensure!(
                    state.retirement.as_ref() == Some(&retirement),
                    "retirement changed"
                );
                let owner = state
                    .records
                    .iter()
                    .find(|r| r.operation_id == retirement.admission_id)
                    .unwrap();
                ensure!(
                    !retirement_protected(data_dir, &state, owner)?,
                    "retirement owner is protected"
                );
                withdrawal_config(data_dir, &state, &retirement, &live)?
            };
            revalidate()?;
            registry.refresh_local_model_configuration(&config).await?;
            mark_model_withdrawn(data_dir, &retirement)?;
        }
        finish_model_retirement(data_dir)?;
        // Kubo owns its own storage. Only removed Runtime bytes lose their
        // charge; subsequent real capacity checks still include backend usage.
    }
    anyhow::bail!("model capacity remains unavailable")
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
    revalidate().context(PreparationFailurePhase::Authority)?;
    let record = load_operation(data_dir, id)?;
    ensure!(
        record.active()
            && !record.cancel_requested
            && !stop.load(Ordering::Acquire)
            && now()? < record.expires_at,
        "preparation stopped"
    );
    current_entry(data_dir, &record).context(PreparationFailurePhase::Policy)?;
    Ok(record)
}

fn remaining_capacity_charges(
    total_bytes: u64,
    completed_bytes: u64,
    index_bytes: u64,
) -> anyhow::Result<(u64, u64)> {
    ensure!(
        (1..=MAX_PACKAGE_BYTES).contains(&total_bytes)
            && completed_bytes <= total_bytes
            && index_bytes <= 65536,
        "invalid preparation capacity progress"
    );
    let delivered = completed_bytes
        .checked_add(index_bytes)
        .context("delivered byte accounting overflow")?;
    let staging = staging_charge(total_bytes)?;
    let backend = preparation_charge(total_bytes)?
        .checked_sub(staging)
        // Credit one delivered payload copy. Keep the existing second-payload
        // and fixed overhead allowance; this is not allocated-byte attribution.
        .and_then(|charge| charge.checked_sub(delivered))
        .context("capacity charge overflow")?;
    let outstanding_stage = staging
        .checked_sub(delivered)
        .context("staging accounting overflow")?;
    Ok((outstanding_stage, backend))
}

async fn require_capacity(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    record: &PreparationRecord,
) -> anyhow::Result<u64> {
    let (_, backend) = remaining_capacity_charges(
        record.total_bytes,
        record.completed_bytes,
        record.index_bytes,
    )?;
    let observed = registry.check_local_ipfs_capacity(backend).await?;
    require_runtime_capacity(data_dir, record, observed.volume_id)?;
    Ok(observed.volume_id)
}

fn runtime_capacity_charge(record: &PreparationRecord, shared_volume: bool) -> anyhow::Result<u64> {
    let (outstanding_stage, backend) = remaining_capacity_charges(
        record.total_bytes,
        record.completed_bytes,
        record.index_bytes,
    )?;
    if shared_volume {
        outstanding_stage
            .checked_add(backend)
            .context("shared volume charge overflow")
    } else {
        Ok(outstanding_stage)
    }
}

fn require_runtime_capacity(
    data_dir: &Path,
    record: &PreparationRecord,
    backend_volume: u64,
) -> anyhow::Result<()> {
    let inventory = Inventory::open(data_dir, false)?;
    require_cache_budget(data_dir, &inventory.load()?)?;
    let required = runtime_capacity_charge(record, inventory.volume()? == backend_volume)?;
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
    registry
        .prepare_local_ipfs_backend()
        .await
        .context(PreparationFailurePhase::MetadataRead)?;
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
    let mut backend_volume = require_capacity(data_dir, registry, &record)
        .await
        .context(PreparationFailurePhase::Capacity)?;
    require_active(data_dir, id, stop, revalidate)?;
    let index =
        crate::content::fetch_model_part(registry, &entry.cid, "_elastos_object.json", None)
            .await
            .context(PreparationFailurePhase::MetadataRead)?;
    require_active(data_dir, id, stop, revalidate)?;
    let object: serde_json::Value =
        serde_json::from_slice(&index).context(PreparationFailurePhase::MetadataIntegrity)?;
    ensure!(
        object == entry.object_manifest,
        anyhow::anyhow!("fetched index differs from signed catalog")
            .context(PreparationFailurePhase::MetadataIntegrity)
    );
    let stage = Inventory::open(data_dir, false)?
        .stage(true)
        .context(PreparationFailurePhase::MetadataWrite)?;
    let mut file = stage
        .create_file("_elastos_object.json")
        .context(PreparationFailurePhase::MetadataWrite)?;
    file.write_all(&index)
        .context(PreparationFailurePhase::MetadataWrite)?;
    file.sync_all()
        .context(PreparationFailurePhase::MetadataSync)?;
    update_operation(data_dir, id, |record| {
        record.index_bytes = index.len() as u64
    })?;
    // Backend identity/pressure is observed per byte window. Runtime floor and
    // authority remain per read; the outstanding charge is not an OS reservation.
    let mut window_bytes = index.len() as u64;
    for expected in &closure.files {
        let weights = expected.path == entry.manifest.entrypoint;
        let write_phase = if weights {
            PreparationFailurePhase::WeightsWrite
        } else {
            PreparationFailurePhase::MetadataWrite
        };
        let mut file = stage.create_file(&expected.path).context(write_phase)?;
        let mut digest = Sha256::new();
        let mut offset = 0u64;
        while offset < expected.size {
            let record = require_active(data_dir, id, stop, revalidate)?;
            let length = (expected.size - offset).min(65536);
            if window_bytes + length > CAPACITY_WINDOW_BYTES {
                backend_volume = require_capacity(data_dir, registry, &record)
                    .await
                    .context(PreparationFailurePhase::Capacity)?;
                window_bytes = 0;
            } else {
                require_runtime_capacity(data_dir, &record, backend_volume)
                    .context(PreparationFailurePhase::Capacity)?;
            }
            require_active(data_dir, id, stop, revalidate)?;
            let bytes = crate::content::fetch_model_part(
                registry,
                &entry.cid,
                &expected.path,
                Some((offset, length)),
            )
            .await
            .context(if weights {
                PreparationFailurePhase::WeightsRead
            } else {
                PreparationFailurePhase::MetadataRead
            })?;
            require_active(data_dir, id, stop, revalidate)?;
            if weights && offset == 0 {
                ensure!(
                    bytes.len() >= 8
                        && &bytes[..4] == b"GGUF"
                        && matches!(u32::from_le_bytes(bytes[4..8].try_into()?), 2 | 3),
                    anyhow::anyhow!("invalid GGUF header")
                        .context(PreparationFailurePhase::WeightsHeader)
                );
            }
            stage
                .check_file(&expected.path, &file, offset)
                .context(write_phase)?;
            file.write_all(&bytes).context(write_phase)?;
            digest.update(&bytes);
            offset += length;
            update_operation(data_dir, id, |record| record.completed_bytes += length)?;
            window_bytes += length;
        }
        file.sync_all().context(if weights {
            PreparationFailurePhase::WeightsSync
        } else {
            PreparationFailurePhase::MetadataSync
        })?;
        ensure!(
            hex::encode(digest.finalize()) == expected.sha256,
            anyhow::anyhow!("model file hash mismatch").context(if weights {
                PreparationFailurePhase::WeightsIntegrity
            } else {
                PreparationFailurePhase::MetadataIntegrity
            })
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
    require_capacity(data_dir, registry, &record).await?;
    require_admission(data_dir, id, stop, revalidate, reconcile_existing)?;
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

fn admitted_model_offer(
    entry: &super::VerifiedModelCatalogEntry,
    weights: &crate::content::ContentObjectFile,
    engine: &crate::setup::LocalModelEngineIdentity,
) -> anyhow::Result<serde_json::Value> {
    bound_model_offer(
        &entry.cid,
        &weights.path,
        &weights.sha256,
        &engine.receipt_sha256,
        &engine.sha256,
        &entry.manifest.name,
    )
}

fn bound_model_offer(
    cid: &str,
    entrypoint: &str,
    weights_sha256: &str,
    engine_receipt_sha256: &str,
    engine_sha256: &str,
    title: &str,
) -> anyhow::Result<serde_json::Value> {
    let identity = serde_json::json!({
        "schema":"elastos.model.admitted-offer/v1",
        "cid":cid, "entrypoint":entrypoint, "weights_sha256":weights_sha256,
        "engine_receipt_sha256":engine_receipt_sha256, "engine_sha256":engine_sha256,
    });
    let id = format!(
        "model:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&identity)?))
    );
    Ok(serde_json::json!({
        "id":id, "title":title, "operation":"text.generate",
        "input_modalities":["text/plain"], "output_modalities":["text/plain"],
        "stream_output":true,
        "policy":{
            "schema":"elastos.model.policy/v1",
            "concurrency_limit":1, "input_bytes_limit":32768,
            // Terminal output retains the complete 64 KiB output plus its
            // RunEvent envelope. Streaming uses the same bounded event policy.
            "inline_output_bytes_limit":65536, "event_bytes_limit":65536 + 1024,
            "runtime_ms_limit":120000, "retention_secs":3600,
            "cancel_settlement_timeout_ms":15000
        }
    }))
}

impl ModelActivation {
    fn validate(&self, record: &PreparationRecord) -> anyhow::Result<()> {
        ensure!(
            serde_json::to_vec(self)?.len() <= 16 * 1024,
            "model activation exceeds bound"
        );
        let field = |value: &serde_json::Value| -> anyhow::Result<String> {
            let text = value.as_str().context("invalid activation field")?;
            ensure!(
                !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control),
                "invalid activation field"
            );
            Ok(text.to_owned())
        };
        let adapter = &self.offer["adapter"];
        let engine_path = field(&adapter["engine"]["path"])?;
        let model_path = field(&adapter["model"]["path"])?;
        let engine_sha = field(&adapter["engine"]["sha256"])?;
        let model_sha = field(&adapter["model"]["sha256"])?;
        let title = field(&self.offer["title"])?;
        let digest = |value: &str| {
            value.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        ensure!(
            digest(&engine_sha)
                && digest(&model_sha)
                && digest(&self.engine_receipt_sha256)
                && title.len() <= 256
                && title.trim() == title
                && !self.entrypoint.is_empty()
                && self.entrypoint.len() <= 1024
                && Path::new(&self.entrypoint)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
                && Path::new(&engine_path).is_absolute()
                && Path::new(&model_path).is_absolute()
                && [&engine_path, &model_path]
                    .iter()
                    .all(|p| Path::new(p).components().all(|c| matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )))
                && Path::new(&model_path).ends_with(
                    Path::new(&format!("admitted-{}", record.admission_id)).join(&self.entrypoint)
                ),
            "invalid activation identity"
        );
        let mut expected = bound_model_offer(
            &record.package_cid,
            &self.entrypoint,
            model_sha.strip_prefix("sha256:").unwrap(),
            &self.engine_receipt_sha256,
            &engine_sha,
            &title,
        )?;
        expected.as_object_mut().unwrap().remove("stream_output");
        expected["policy"].as_object_mut().unwrap().remove("schema");
        expected["enabled"] = serde_json::json!(true);
        expected["adapter"] = serde_json::json!({"kind":"local_llama_cpp_text",
            "engine":{"path":engine_path,"sha256":engine_sha},
            "model":{"path":model_path,"sha256":model_sha},
            "settings":local_model_startup_profile("darwin-arm64")?});
        ensure!(
            self.offer == expected,
            "activation descriptor binding changed"
        );
        Ok(())
    }

    fn check_root(&self, data_dir: &Path, record: &PreparationRecord) -> anyhow::Result<()> {
        self.validate(record)?;
        let expected = data_dir
            .canonicalize()?
            .join("model-preparation")
            .join(format!("admitted-{}", record.admission_id))
            .join(&self.entrypoint);
        ensure!(
            self.offer["adapter"]["model"]["path"]
                .as_str()
                .is_some_and(|p| Path::new(p) == expected),
            "activation model owner changed"
        );
        Ok(())
    }

    fn summary(&self) -> serde_json::Value {
        let mut offer = self.offer.clone();
        offer.as_object_mut().unwrap().remove("adapter");
        offer.as_object_mut().unwrap().remove("enabled");
        offer["stream_output"] = serde_json::json!(true);
        offer["policy"]["schema"] = serde_json::json!("elastos.model.policy/v1");
        offer
    }
}

/// Caller-scoped dispatch facts, not engine warmth or inference evidence. The
/// inventory and signed catalog remain the owners; this projection stores nothing.
pub(in crate::api) fn unavailable_model_runtime_projection() -> serde_json::Value {
    serde_json::json!({"admitted":false,"kept":false,
        "dispatch_ready":false,"offer_id":null,"preparation":null})
}

#[derive(PartialEq)]
struct ExpectedModelOffer {
    offer: serde_json::Value,
    files: Vec<storage::Stamp>,
}

pub(in crate::api) async fn model_runtime_projection(
    data_dir: &Path,
    registry: Option<&elastos_runtime::provider::ProviderRegistry>,
    context: &HomeLaunchTokenContext,
    cid: &str,
    operation: Option<&str>,
) -> serde_json::Value {
    let unavailable = unavailable_model_runtime_projection;
    let snapshot = || -> anyhow::Result<(serde_json::Value, Option<ExpectedModelOffer>)> {
        ensure!(
            context_is_bounded(context) && canonical_cid(cid, 0x70),
            "model selection unavailable"
        );
        let inventory = Inventory::open(data_dir, false)?;
        let state = inventory.snapshot()?;
        let record = state
            .records
            .iter()
            .filter(|r| {
                r.request_binding.principal == context.principal_id
                    && r.package_cid == cid
                    && operation.is_none_or(|id| r.operation_id == id)
            })
            .max_by_key(|r| {
                (
                    r.state == PreparationState::Admitted,
                    r.created_at,
                    &r.operation_id,
                )
            });
        let Some(record) = record else {
            return Ok((unavailable(), None));
        };
        let mut projection = unavailable();
        projection["kept"] = serde_json::json!(state.kept(&context.principal_id, cid));
        projection["preparation"] = record.projection();
        if record.state != PreparationState::Admitted {
            return Ok((projection, None));
        }
        let stage = inventory.admitted(&record.admission_id)?;
        stage.check()?;
        projection["admitted"] = serde_json::json!(true);
        if state
            .retirement
            .as_ref()
            .is_some_and(|r| r.admission_id == record.admission_id)
        {
            return Ok((projection, None));
        }
        let expected = (|| -> anyhow::Result<ExpectedModelOffer> {
            let entry = current_entry(data_dir, record)?;
            local_model_startup_profile(&crate::setup::detect_platform())?;
            let manifest = serde_json::from_slice(&super::read_model_catalog_file(
                data_dir,
                "components.json",
                4 * 1024 * 1024,
            )?)?;
            let engine = crate::setup::local_model_engine_receipt_identity(data_dir, &manifest)?;
            let closure = crate::content::parse_content_object_manifest(
                &entry.cid,
                &serde_json::to_vec(&entry.object_manifest)?,
            )?;
            let weights = closure
                .files
                .iter()
                .find(|file| file.path == entry.manifest.entrypoint)
                .context("model entrypoint unavailable")?;
            let offer = admitted_model_offer(&entry, weights, &engine)?;
            let owner = state
                .records
                .iter()
                .find(|r| r.operation_id == record.admission_id)
                .context("activation owner unavailable")?;
            let activation = owner
                .activation
                .as_ref()
                .context("activation binding unavailable")?;
            activation.check_root(data_dir, owner)?;
            ensure!(
                activation.summary() == offer,
                "activation binding differs from current readiness"
            );
            Ok(ExpectedModelOffer {
                offer,
                files: closure
                    .files
                    .iter()
                    .map(|file| stage.file_stamp(file))
                    .collect::<anyhow::Result<_>>()?,
            })
        })()
        .ok();
        Ok((projection, expected))
    };
    let (projection, expected) = match snapshot() {
        Ok(value) => value,
        Err(_) => return unavailable(),
    };
    let Some(expected) = expected else {
        return projection;
    };
    let Some(registry) = registry else {
        return projection;
    };
    // All inventory guards are dropped before I/O. Re-read current trust and
    // admission afterward, including independent Keep changes during the read.
    let offers = registry.local_model_offers().await;
    let (mut projection, current) = match snapshot() {
        Ok(value) => value,
        Err(_) => return unavailable(),
    };
    if current.as_ref() == Some(&expected) {
        if let Ok(offers) = offers {
            let matching: Vec<_> = offers
                .iter()
                .filter(|offer| offer["id"] == expected.offer["id"])
                .collect();
            if matching.len() == 1 && matching[0] == &expected.offer {
                projection["dispatch_ready"] = serde_json::json!(true);
                projection["offer_id"] = expected.offer["id"].clone();
            }
        }
    }
    projection
}

/// Compose admitted offers for Runtime-owned model-provider Init. This function
/// has no capsule route; public inference still uses the existing model grant.
/// The caller retains the returned worker guard through provider spawn/Init.
pub async fn append_admitted_model_startup_offers(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    config: &mut elastos_runtime::provider::BridgeProviderConfig,
) -> anyhow::Result<Option<std::fs::File>> {
    // A Home without preparation inventory keeps its operator offers unchanged.
    match std::fs::symlink_metadata(data_dir.join("model-preparation")) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
        Ok(_) => {}
    }
    let worker = Inventory::open(data_dir, false)?.worker_lock()?;
    let retirement = Inventory::open(data_dir, false)?.load()?.retirement;
    if retirement
        .as_ref()
        .is_some_and(|r| r.phase == RetirementPhase::Withdrawn)
    {
        finish_model_retirement(data_dir)?;
    }
    append_admitted_model_offers_locked(data_dir, registry, config, &worker).await?;
    let snapshot = Inventory::open(data_dir, false)?.load()?;
    if let Some(retirement) = &snapshot.retirement {
        let owner = snapshot
            .records
            .iter()
            .find(|r| r.operation_id == retirement.admission_id)
            .context("startup retirement owner unavailable")?;
        ensure!(
            !retirement_protected(data_dir, &snapshot, owner)?,
            "retirement owner is protected"
        );
        let activation = owner
            .activation
            .as_ref()
            .context("startup activation unavailable")?;
        activation.check_root(data_dir, owner)?;
        let offers = config.extra["offers"]
            .as_array_mut()
            .context("startup offers unavailable")?;
        ensure!(
            offers.len() < 64 && !offers.iter().any(|o| o["id"] == activation.offer["id"]),
            "startup retirement offer collision"
        );
        offers.push(activation.offer.clone());
        config.extra["runtime_admitted_offers"]
            .as_array_mut()
            .context("startup admissions unavailable")?
            .push(serde_json::json!({"offer_id":activation.offer["id"]}));
    }
    ensure!(
        serde_json::to_vec(&config.extra)?.len() <= 256 * 1024,
        "startup config exceeds bound"
    );
    Ok(Some(worker))
}

/// The startup bridge stays private until its journal has accepted exact
/// retirement. An old catalog descriptor is never registered for inference.
pub async fn settle_pending_model_startup(
    data_dir: &Path,
    bridge: &elastos_runtime::provider::ProviderBridge,
    config: &elastos_runtime::provider::BridgeProviderConfig,
    worker: Option<&std::fs::File>,
) -> anyhow::Result<()> {
    if worker.is_none() {
        return Ok(());
    }
    let snapshot = Inventory::open(data_dir, false)?.load()?;
    let Some(retirement) = &snapshot.retirement else {
        return Ok(());
    };
    ensure!(
        retirement.phase == RetirementPhase::WithdrawalPending,
        "invalid startup retirement phase"
    );
    let owner = snapshot
        .records
        .iter()
        .find(|r| r.operation_id == retirement.admission_id)
        .unwrap();
    ensure!(
        !retirement_protected(data_dir, &snapshot, owner)?,
        "retirement owner is protected"
    );
    let activation = owner.activation.as_ref().unwrap();
    activation.check_root(data_dir, owner)?;
    let mut proposed = config.clone();
    let offers = proposed.extra["offers"]
        .as_array_mut()
        .context("startup offers unavailable")?;
    ensure!(
        offers
            .iter()
            .filter(|offer| **offer == activation.offer)
            .count()
            == 1,
        "startup retirement descriptor differs"
    );
    offers.retain(|offer| offer["id"] != activation.offer["id"]);
    proposed.extra["runtime_admitted_offers"]
        .as_array_mut()
        .context("startup admissions unavailable")?
        .retain(|a| a["offer_id"] != activation.offer["id"]);
    let response = bridge
        .request(elastos_runtime::provider::bridge::ProviderRequest::Init { config: proposed })
        .await?;
    let response = serde_json::to_value(response)?;
    ensure!(
        response["status"] == "ok"
            && response["data"]["provider"] == "model-provider"
            && response["data"]["protocol_version"] == "elastos.model-provider/v1"
            && response["data"]["offers_ready"]
                .as_u64()
                .is_some_and(|n| n <= 64),
        "startup model withdrawal unconfirmed"
    );
    mark_model_withdrawn(data_dir, retirement)?;
    finish_model_retirement(data_dir)
}

async fn append_admitted_model_offers_locked(
    data_dir: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    config: &mut elastos_runtime::provider::BridgeProviderConfig,
    _worker: &std::fs::File,
) -> anyhow::Result<()> {
    let snapshot = Inventory::open(data_dir, false)?.load()?;
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
    let mut admitted = Vec::new();
    for entry in entries {
        // Aliases authorize reuse of one artifact; they do not create duplicate
        // offers or transfer ownership between preparation request records.
        let Some(record) = snapshot.records.iter().find(|r| {
            r.state == PreparationState::Admitted
                && r.package_cid == entry.cid
                && r.catalog_head_cid == trust.head_cid
                && snapshot
                    .retirement
                    .as_ref()
                    .is_none_or(|retirement| retirement.admission_id != r.admission_id)
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
        // The local verifier is lazy and may be cold after Runtime restart.
        registry.prepare_local_ipfs_backend().await?;
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
            current.records == snapshot.records,
            "model admission changed during startup"
        );
        let weights = closure
            .files
            .iter()
            .find(|f| f.path == entry.manifest.entrypoint)
            .context("model entrypoint is unavailable")?;
        let mut offer = admitted_model_offer(&entry, weights, &engine)?;
        ensure!(
            offers.len() < 64 && !offers.iter().any(|existing| existing["id"] == offer["id"]),
            "admitted model offer conflicts with operator configuration"
        );
        // The provider's execution binding includes this CID-bound offer ID,
        // artifact digests and policy. Paths and request aliases are not identity.
        offer.as_object_mut().unwrap().remove("stream_output");
        offer["policy"].as_object_mut().unwrap().remove("schema");
        offer["enabled"] = serde_json::json!(true);
        offer["adapter"] = serde_json::json!({
                "kind":"local_llama_cpp_text",
                "engine":{"path":engine.path, "sha256":engine.sha256},
                "model":{"path":stage.path.join(&weights.path), "sha256":format!("sha256:{}",weights.sha256)},
                "settings":settings
        });
        let activation = ModelActivation {
            offer: offer.clone(),
            entrypoint: weights.path.clone(),
            engine_receipt_sha256: engine.receipt_sha256.clone(),
        };
        let inventory = Inventory::open(data_dir, false)?;
        let mut durable = inventory.load()?;
        ensure!(
            durable.records == snapshot.records,
            "model admission changed before Init"
        );
        let owner = durable
            .records
            .iter_mut()
            .find(|r| r.operation_id == record.admission_id)
            .context("activation owner unavailable")?;
        activation.check_root(data_dir, owner)?;
        if let Some(previous) = &owner.activation {
            ensure!(
                previous == &activation,
                "activation descriptor changed on retry"
            );
        } else {
            owner.activation = Some(activation);
            inventory.save(&durable)?;
        }
        admitted.push(serde_json::json!({"offer_id":offer["id"]}));
        offers.push(offer);
    }
    let mut extra = config.extra.clone();
    extra["offers"] = serde_json::Value::Array(offers);
    extra["runtime_admitted_offers"] = serde_json::Value::Array(admitted);
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
    snapshot.prune_retention();
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
        capacity_requests: Mutex<Vec<u64>>,
        capacity_volume: u64,
        fail_capacity_at: Option<usize>,
        read_sizes: Mutex<Vec<usize>>,
        after_weight_read: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
        hold_drain: AtomicBool,
        fail_drain: AtomicBool,
        cold_backend: AtomicBool,
        hold_read: AtomicBool,
        hold_hash: AtomicBool,
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
                        (
                            "weights.gguf",
                            Some("file_unavailable" | "file_unavailable_undrained"),
                        ) => {
                            self.fail_drain.store(
                                fault == Some("file_unavailable_undrained"),
                                Ordering::Release,
                            );
                            return Err(elastos_runtime::provider::ProviderError::Provider(
                                "private fixture cause /secret/provider?credential=hidden".into(),
                            ));
                        }
                        ("weights.gguf", Some("header")) => bytes[..4].copy_from_slice(b"FAIL"),
                        ("capsule.json", Some("metadata_tamper")) => {
                            *bytes.last_mut().unwrap() ^= 1;
                        }
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
                    self.read_sizes
                        .lock()
                        .unwrap()
                        .push(if path == "_elastos_object.json" {
                            self.files[path].len()
                        } else {
                            let range = &request["_runtime_invocation"]["range"];
                            (range["end"].as_u64().unwrap() - range["start"].as_u64().unwrap() + 1)
                                as usize
                        });
                    if path == "weights.gguf" {
                        if let Some(hook) = self.after_weight_read.lock().unwrap().as_ref() {
                            hook();
                        }
                    }
                    Ok(serde_json::json!({"status":"ok","data":response}))
                }
                "runtime_check_capacity" => {
                    self.capacity_requests
                        .lock()
                        .unwrap()
                        .push(request["required_bytes"].as_u64().unwrap());
                    if self.fail_capacity_at == Some(self.capacity_requests.lock().unwrap().len()) {
                        return Err(elastos_runtime::provider::ProviderError::Provider(
                            "fixture backend capacity refused".into(),
                        ));
                    }
                    Ok(serde_json::json!({"status":"ok","data":{
                        "volume_id":self.capacity_volume,"capacity_bytes":1_u64 << 40,"available_bytes":1_u64 << 39,"required_bytes":request["required_bytes"]
                    }}))
                }
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
                    self.cold_backend.store(false, Ordering::Release);
                    Ok(serde_json::json!({"status":"ok"}))
                }
                "runtime_hash_staged_directory" => {
                    if self.cold_backend.load(Ordering::Acquire) {
                        return Err(elastos_runtime::provider::ProviderError::Unavailable(
                            "directory hash backend unavailable".into(),
                        ));
                    }
                    if self.hold_hash.swap(false, Ordering::AcqRel) {
                        self.entered.notify_one();
                        self.release.notified().await;
                    }
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
            install_engine_bytes(root, b"fixture engine bytes; never executed")
        }

        fn install_engine_bytes(root: &Path, bytes: &[u8]) -> EngineFixture {
            let relative = "libexec/fixture-engine";
            let bundle = root.join(relative);
            std::fs::create_dir_all(&bundle).unwrap();
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

        #[derive(Default)]
        struct ReadinessOffersFixture {
            response: Mutex<serde_json::Value>,
            calls: Mutex<Vec<serde_json::Value>>,
            hold: AtomicBool,
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }

        #[async_trait::async_trait]
        impl elastos_runtime::provider::Provider for ReadinessOffersFixture {
            fn name(&self) -> &'static str {
                "catalog-model-fixture"
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
                panic!("catalog reads must not dispatch model runs or content effects")
            }
            async fn send_raw(
                &self,
                request: &serde_json::Value,
            ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
                assert_eq!(request, &serde_json::json!({"op":"offers_list"}));
                self.calls.lock().unwrap().push(request.clone());
                if self.hold.load(Ordering::Acquire) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                Ok(self.response.lock().unwrap().clone())
            }
        }

        #[tokio::test]
        async fn model_catalog_readiness_matches_admitted_offer_without_mutating_preparation() {
            use axum::body::{to_bytes, Body};
            use axum::http::{Request, StatusCode};
            use elastos_runtime::auth::AuthSessionGrantV1;
            use std::os::unix::fs::MetadataExt as _;
            use tower::ServiceExt as _;

            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let mut configured = config(root.path());
            // Derive the expected ID/policy once through the actual composer.
            // A catalog poll must not repeat its payload verification or Init.
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut configured)
                .await
                .unwrap();
            let mut offer = configured.extra["offers"][1].clone();
            offer.as_object_mut().unwrap().remove("adapter");
            offer.as_object_mut().unwrap().remove("enabled");
            offer["stream_output"] = serde_json::json!(true);
            offer["policy"]["schema"] = serde_json::json!("elastos.model.policy/v1");
            let provider = Arc::new(ReadinessOffersFixture {
                response: Mutex::new(serde_json::json!({"status":"ok", "data":{
                    "schema":"elastos.model.offers-list/v1", "provider":"model-provider",
                    "protocol_version":"elastos.model-provider/v1",
                    "offers":[offer.clone()]
                }})),
                calls: Mutex::new(Vec::new()),
                ..Default::default()
            });
            registry
                .register_sub_provider("model", provider.clone())
                .await
                .unwrap();
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();

            let context = context();
            let timestamp = now().unwrap();
            crate::auth::store_session_grant(
                root.path(),
                AuthSessionGrantV1 {
                    schema: AuthSessionGrantV1::SCHEMA.into(),
                    grant_id: context.grant_id.clone(),
                    session_id: context.session_id.clone(),
                    principal_id: context.principal_id.clone(),
                    proof_binding_id: context.proof_binding_id.clone().unwrap(),
                    issued_at: timestamp,
                    expires_at: timestamp + 3600,
                    apps: vec!["marketplace".into()],
                },
            )
            .unwrap();
            let token = crate::api::gateway::issue_home_launch_token_with_context(
                root.path(),
                "marketplace",
                &context,
            )
            .unwrap();
            let app = crate::api::gateway::gateway_router(crate::api::gateway::GatewayState {
                provider_registry: Some(registry),
                collaboration_chat_product_port: None,
                collaboration_presence_product_port: None,
                collaboration_discovery_service: None,
                identity_manager: Arc::new(std::sync::OnceLock::new()),
                cache_dir: root.path().to_path_buf(),
                data_dir: root.path().to_path_buf(),
            });
            let inventory_path = root.path().join("model-preparation/state.json");
            let before = std::fs::read(&inventory_path).unwrap();
            let stage = Inventory::open(root.path(), false)
                .unwrap()
                .admitted(&record.admission_id)
                .unwrap();
            let weights = stage.path.join("weights.gguf");
            let bytes = std::fs::read(&weights).unwrap();
            let inode = std::fs::metadata(&weights).unwrap().ino();
            let calls = backend.calls.lock().unwrap().clone();
            let orphan = root.path().join("model-preparation/state.next");
            std::fs::write(&orphan, &before).unwrap();
            std::fs::set_permissions(&orphan, std::fs::Permissions::from_mode(0o600)).unwrap();
            let orphan_inode = std::fs::metadata(&orphan).unwrap().ino();

            for _ in 0..2 {
                let request = Request::builder()
                    .uri("/api/capsules/catalog")
                    .header("host", "localhost:61180")
                    .header("origin", "null")
                    .header("x-elastos-home-token", &token)
                    .body(Body::empty())
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                let status = response.status();
                let catalog: serde_json::Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                )
                .unwrap();
                assert_eq!(status, StatusCode::OK, "{catalog}");
                assert_eq!(catalog["model_catalog_state"], "verified");
                let model = catalog["capsules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["cid"] == record.package_cid)
                    .unwrap();
                let readiness = &model["model_runtime"];
                assert_eq!(readiness["dispatch_ready"], true, "{model}");
                assert_eq!(readiness["admitted"], true);
                assert_eq!(readiness["kept"], true);
                assert_eq!(readiness["offer_id"], offer["id"]);
                assert_eq!(
                    readiness["preparation"]["operation_id"],
                    record.operation_id
                );
                assert_eq!(
                    readiness["preparation"]["completed_bytes"],
                    record.total_bytes
                );
                assert_eq!(model["launchable"], false);
                assert_eq!(
                    model["installed"], false,
                    "content admission does not change app install counts"
                );
                assert_eq!(model["state"], "ready");
                assert_eq!(model["cid_state"], "admission-verified");
                assert_eq!(model["trust_state"], "publisher-verified-admitted");
                assert_eq!(
                    model["projection"]["audit_mirror"]["note"],
                    "Content admitted; a matching current model offer is available for dispatch."
                );
                let public = serde_json::to_string(readiness).unwrap();
                for private in [
                    root.path().to_str().unwrap(),
                    &context.principal_id,
                    "adapter",
                    "inference_ready",
                    "engine_warm",
                ] {
                    assert!(!public.contains(private), "{private}: {public}");
                }
            }
            assert_eq!(provider.calls.lock().unwrap().len(), 2);
            assert_eq!(
                *backend.calls.lock().unwrap(),
                calls,
                "poll must not rehash or fetch model bytes"
            );
            assert_eq!(std::fs::read(inventory_path).unwrap(), before);
            assert_eq!(std::fs::read(&weights).unwrap(), bytes);
            assert_eq!(std::fs::metadata(weights).unwrap().ino(), inode);
            assert_eq!(
                std::fs::read(&orphan).unwrap(),
                before,
                "catalog must not perform owner recovery cleanup"
            );
            assert_eq!(std::fs::metadata(orphan).unwrap().ino(), orphan_inode);
        }

        async fn readiness_provider(
            root: &Path,
            registry: &elastos_runtime::provider::ProviderRegistry,
        ) -> (serde_json::Value, Arc<ReadinessOffersFixture>) {
            let mut configured = config(root);
            let _ = append_admitted_model_startup_offers(root, registry, &mut configured)
                .await
                .unwrap();
            let mut offer = configured.extra["offers"][1].clone();
            offer.as_object_mut().unwrap().remove("adapter");
            offer.as_object_mut().unwrap().remove("enabled");
            offer["stream_output"] = serde_json::json!(true);
            offer["policy"]["schema"] = serde_json::json!("elastos.model.policy/v1");
            let response = serde_json::json!({"status":"ok", "data":{
                "schema":"elastos.model.offers-list/v1", "provider":"model-provider",
                "protocol_version":"elastos.model-provider/v1", "offers":[offer]
            }});
            let provider = Arc::new(ReadinessOffersFixture {
                response: Mutex::new(response.clone()),
                ..Default::default()
            });
            registry
                .register_sub_provider("model", provider.clone())
                .await
                .unwrap();
            (response, provider)
        }

        fn readiness_catalog_app(
            root: &Path,
            registry: Arc<elastos_runtime::provider::ProviderRegistry>,
            context: &HomeLaunchTokenContext,
        ) -> (axum::Router, String) {
            let dir = root.join("capsules/marketplace");
            std::fs::create_dir_all(&dir).unwrap();
            let mut manifest: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../capsules/marketplace/capsule.json"
            )))
            .unwrap();
            assert_eq!(
                manifest["interfaces"][0]["methods"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|method| method["id"] == "content.status"
                        && method["operation"] == "status"
                        && method["resource"] == RESOURCE
                        && method["risk"] == "read"
                        && method["approval"] == "runtime_policy")
                    .count(),
                1
            );
            // Status uses the real capsule declaration; only catalog.list is fixture-specific.
            manifest["interfaces"][0]["methods"].as_array_mut().unwrap().push(serde_json::json!({
                "id":"catalog.list", "operation":"list", "resource":RESOURCE, "risk":"read", "approval":"runtime_policy", "audit":"summary"
            }));
            std::fs::write(
                dir.join("capsule.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            change_config(
                root,
                |config| {
                    config["external"]["marketplace"] =
                        serde_json::json!({"install_path":"capsules/marketplace","platforms":{}})
                },
            );
            let timestamp = now().unwrap();
            crate::auth::store_session_grant(
                root,
                elastos_runtime::auth::AuthSessionGrantV1 {
                    schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.into(),
                    grant_id: context.grant_id.clone(),
                    session_id: context.session_id.clone(),
                    principal_id: context.principal_id.clone(),
                    proof_binding_id: context.proof_binding_id.clone().unwrap(),
                    issued_at: timestamp,
                    expires_at: timestamp + 3600,
                    apps: vec!["marketplace".into()],
                },
            )
            .unwrap();
            let token = crate::api::gateway::issue_home_launch_token_with_context(
                root,
                "marketplace",
                context,
            )
            .unwrap();
            (
                crate::api::gateway::gateway_router(crate::api::gateway::GatewayState {
                    provider_registry: Some(registry),
                    collaboration_chat_product_port: None,
                    collaboration_presence_product_port: None,
                    collaboration_discovery_service: None,
                    identity_manager: Arc::new(std::sync::OnceLock::new()),
                    cache_dir: root.to_path_buf(),
                    data_dir: root.to_path_buf(),
                }),
                token,
            )
        }

        async fn readiness_catalog_request(
            app: &axum::Router,
            token: &str,
            input: Option<serde_json::Value>,
        ) -> (axum::http::StatusCode, serde_json::Value) {
            use tower::ServiceExt as _;
            let request = axum::http::Request::builder()
                .header("host", "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .header("content-type", "application/json");
            let request = if let Some(input) = input {
                request
                    .method("POST")
                    .uri("/api/capsules/interfaces/invoke")
                    .body(axum::body::Body::from(serde_json::to_vec(&input).unwrap()))
            } else {
                request
                    .uri("/api/capsules/catalog")
                    .body(axum::body::Body::empty())
            }
            .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            (
                status,
                serde_json::from_slice(
                    &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .unwrap(),
                )
                .unwrap(),
            )
        }

        #[tokio::test]
        async fn model_catalog_readiness_rejects_missing_duplicate_wrong_and_malformed_offers() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let (valid, provider) = readiness_provider(root.path(), &registry).await;
            let (app, token) = readiness_catalog_app(root.path(), registry, &context());
            let ordinary = readiness_catalog_request(&app, &token, None).await.1["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["name"] == "marketplace")
                .unwrap()
                .clone();
            let offer = &valid["data"]["offers"][0];
            let mut cases = Vec::new();
            for (pointer, value) in [
                ("/data/offers", serde_json::json!([])),
                ("/data/offers", serde_json::json!([offer, offer])),
                (
                    "/data/offers/0/id",
                    serde_json::json!("model:same-name-other-cid"),
                ),
                (
                    "/data/offers/0/operation",
                    serde_json::json!("image.generate"),
                ),
                (
                    "/data/offers/0/input_modalities",
                    serde_json::json!(["image/png"]),
                ),
                (
                    "/data/offers/0/output_modalities",
                    serde_json::json!(["image/png"]),
                ),
                ("/data/offers/0/stream_output", serde_json::json!(false)),
                (
                    "/data/offers/0/policy/input_bytes_limit",
                    serde_json::json!(1),
                ),
                ("/data/offers/0/policy/schema", serde_json::json!("unknown")),
                ("/data/schema", serde_json::json!("unknown")),
            ] {
                let mut response = valid.clone();
                *response.pointer_mut(pointer).unwrap() = value;
                cases.push(response);
            }
            cases.push(serde_json::json!({"status":"ok","data":{"provider":"model-provider","protocol_version":"elastos.model-provider/v1","offers_ready":1}}));
            cases.push(serde_json::json!({"status":"error","error":"private /engine/path"}));
            let mut extra = valid.clone();
            extra["data"]["offers"][0]["adapter"] = serde_json::json!("private /engine/path");
            cases.push(extra);
            let before = std::fs::read(root.path().join("model-preparation/state.json")).unwrap();
            let calls = backend.calls.lock().unwrap().clone();
            for response in cases {
                *provider.response.lock().unwrap() = response;
                let (status, catalog) = readiness_catalog_request(&app, &token, None).await;
                assert_eq!(status, axum::http::StatusCode::OK);
                let model = catalog["capsules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["cid"] == record.package_cid)
                    .unwrap();
                assert_eq!(model["model_runtime"]["admitted"], true);
                assert_eq!(model["model_runtime"]["dispatch_ready"], false);
                assert!(model["model_runtime"]["offer_id"].is_null());
                assert_eq!(model["state"], "admitted");
                assert_eq!(
                    catalog["capsules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["name"] == "marketplace")
                        .unwrap(),
                    &ordinary
                );
                assert!(!catalog.to_string().contains("private /engine/path"));
            }
            assert_eq!(
                std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
                before
            );
            assert_eq!(*backend.calls.lock().unwrap(), calls);
        }

        #[tokio::test]
        async fn model_catalog_readiness_is_principal_scoped_and_shared_with_typed_status() {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let (_, provider) = readiness_provider(root.path(), &registry).await;
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
            let (app, token) = readiness_catalog_app(root.path(), registry.clone(), &context());
            for (method, input) in [
                ("catalog.list", serde_json::json!({})),
                (
                    "content.status",
                    serde_json::json!({"operation_id":record.operation_id}),
                ),
            ] {
                let (status, response) = readiness_catalog_request(&app, &token, Some(serde_json::json!({
                    "request_id":"readiness-status", "capsule":"marketplace", "interface":"elastos.marketplace.catalog", "method":method, "input":input
                }))).await;
                assert_eq!(status, axum::http::StatusCode::OK, "{response}");
                let value = if method == "catalog.list" {
                    response["output"]["catalog"]["capsules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["cid"] == record.package_cid)
                        .unwrap()["model_runtime"]
                        .clone()
                } else {
                    response["output"].clone()
                };
                assert_eq!(value["dispatch_ready"], true, "{response}");
                assert_eq!(value["kept"], true);
                assert!(!value.to_string().contains("inference_ready"));
            }
            assert_eq!(provider.calls.lock().unwrap().len(), 2);
            let mut other = context();
            other.principal_id = "person:other-reader".into();
            other.session_id = "session-other".into();
            other.grant_id = "grant-other".into();
            let (other_app, other_token) =
                readiness_catalog_app(root.path(), registry.clone(), &other);
            let local = root.path().join("capsules/model-fixture");
            std::fs::create_dir_all(&local).unwrap();
            std::fs::write(
                local.join("capsule.json"),
                serde_json::to_vec(&current_entry(root.path(), &record).unwrap().manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(local.join("weights.gguf"), b"unverified same-name content").unwrap();
            let (_, catalog) = readiness_catalog_request(&other_app, &other_token, None).await;
            let model = catalog["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["cid"] == record.package_cid)
                .unwrap();
            assert_eq!(
                model["model_runtime"],
                unavailable_model_runtime_projection()
            );
            assert!(!catalog.to_string().contains(&record.operation_id));
            assert!(!catalog.to_string().contains(&context().principal_id));
            assert_eq!(
                provider.calls.lock().unwrap().len(),
                2,
                "unadmitted caller must not read offers"
            );
            let alias = reserve_at(
                root.path(),
                &caller(&other, &method("use")),
                "other-use",
                &record.package_cid,
                now().unwrap(),
            )
            .unwrap();
            let pending = model_runtime_projection(
                root.path(),
                Some(&registry),
                &other,
                &record.package_cid,
                None,
            )
            .await;
            assert_eq!(pending["dispatch_ready"], false);
            assert_eq!(pending["preparation"]["operation_id"], alias.operation_id);
            update_operation(root.path(), &alias.operation_id, |alias| {
                alias.state = PreparationState::Admitted;
                alias.index_bytes = record.index_bytes;
                alias.completed_bytes = record.total_bytes;
            })
            .unwrap();
            let value = model_runtime_projection(
                root.path(),
                Some(&registry),
                &other,
                &record.package_cid,
                None,
            )
            .await;
            assert_eq!(value["dispatch_ready"], true);
            assert_eq!(value["kept"], false);
            assert_eq!(value["preparation"]["operation_id"], alias.operation_id);
            let unavailable =
                model_runtime_projection(root.path(), None, &other, &record.package_cid, None)
                    .await;
            assert_eq!(unavailable["dispatch_ready"], false);
        }

        #[tokio::test]
        async fn model_catalog_readiness_revalidates_authority_and_catalog_after_offer_read() {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let (_, provider) = readiness_provider(root.path(), &registry).await;
            let (app, token) = readiness_catalog_app(root.path(), registry.clone(), &context());
            provider.hold.store(true, Ordering::Release);
            let request_app = app.clone();
            let request_token = token.clone();
            let request = tokio::spawn(async move {
                readiness_catalog_request(&request_app, &request_token, None).await
            });
            provider.entered.notified().await;
            // This succeeds while the read is held: no inventory/worker lock spans provider I/O.
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
            let config_path = root.path().join("components.json");
            let original = std::fs::read(&config_path).unwrap();
            change_config(root.path(), |config| {
                config["model_catalog"]["publisher_dids"] = serde_json::json!([])
            });
            provider.release.notify_one();
            let (status, catalog) = request.await.unwrap();
            assert_eq!(status, axum::http::StatusCode::OK);
            assert_eq!(catalog["model_catalog_state"], "unavailable");
            assert!(catalog["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "marketplace"));
            assert!(!catalog["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["cid"] == record.package_cid));
            std::fs::write(config_path, original).unwrap();
            let request =
                tokio::spawn(async move { readiness_catalog_request(&app, &token, None).await });
            provider.entered.notified().await;
            crate::auth::revoke_session_grant(root.path(), &context().session_id, now().unwrap())
                .unwrap();
            provider.release.notify_one();
            assert_eq!(request.await.unwrap().0, axum::http::StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn model_catalog_readiness_rejects_changed_cid_and_oversized_catalog_membership() {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let (_, provider) = readiness_provider(root.path(), &registry).await;
            let (mut payload, files) = package_fixture(b"GGUF\x03\0\0\0different".to_vec());
            // Synthetic catalog identity follows this fixture's changed closure
            // metadata; real DAG-PB package hashing has separate provider proof.
            let digest = Sha256::digest(&files["_elastos_object.json"]);
            let hash = cid::multihash::Multihash::<64>::wrap(0x12, &digest).unwrap();
            payload["entries"][0]["cid"] =
                serde_json::json!(cid::Cid::new_v1(0x70, hash).to_string());
            write_preparation_catalog(root.path(), &payload);
            let (app, token) = readiness_catalog_app(root.path(), registry.clone(), &context());
            let (_, catalog) = readiness_catalog_request(&app, &token, None).await;
            let model = catalog["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["model_content"].is_object())
                .unwrap();
            assert_ne!(model["cid"], record.package_cid);
            assert_eq!(
                model["model_runtime"],
                unavailable_model_runtime_projection()
            );
            let second = payload["entries"][0].clone();
            payload["entries"].as_array_mut().unwrap().push(second);
            write_preparation_catalog(root.path(), &payload);
            let (app, token) = readiness_catalog_app(root.path(), registry, &context());
            let (_, catalog) = readiness_catalog_request(&app, &token, None).await;
            assert_eq!(catalog["model_catalog_state"], "unavailable");
            assert!(catalog["capsules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "marketplace"));
            assert!(provider.calls.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn model_catalog_readiness_receipt_identity_rejects_replacement_and_unsafe_metadata()
        {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let engine = install_engine(root.path());
            let (response, _) = readiness_provider(root.path(), &registry).await;
            let receipt = engine.0.join(".elastos-engine.json");
            let bytes = std::fs::read(&receipt).unwrap();
            let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let expected_id = &response["data"]["offers"][0]["id"];
            let projection = model_runtime_projection(
                root.path(),
                Some(&registry),
                &context(),
                &record.package_cid,
                None,
            )
            .await;
            assert_eq!(
                &projection["offer_id"], expected_id,
                "receipt-derived ID must equal full startup composer ID"
            );
            for (field, replacement) in [
                ("version", serde_json::json!("changed")),
                ("schema", serde_json::json!("unknown")),
                ("entries", serde_json::json!([])),
            ] {
                let mut changed = original.clone();
                changed[field] = replacement;
                std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(&receipt, serde_json::to_vec(&changed).unwrap()).unwrap();
                std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o400)).unwrap();
                assert_eq!(
                    model_runtime_projection(
                        root.path(),
                        Some(&registry),
                        &context(),
                        &record.package_cid,
                        None
                    )
                    .await["dispatch_ready"],
                    false
                );
            }
            let mut changed = original.clone();
            changed["entries"][0]["sha256"] =
                serde_json::json!(format!("sha256:{}", "b".repeat(64)));
            // Replace the receipt inode as well as its validly shaped digest.
            std::fs::set_permissions(&engine.0, std::fs::Permissions::from_mode(0o700)).unwrap();
            let replacement = engine.0.join("replacement");
            std::fs::write(&replacement, serde_json::to_vec(&changed).unwrap()).unwrap();
            std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o400)).unwrap();
            std::fs::rename(&replacement, &receipt).unwrap();
            std::fs::set_permissions(&engine.0, std::fs::Permissions::from_mode(0o500)).unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                false
            );
            let manifest = serde_json::from_slice(
                &std::fs::read(root.path().join("components.json")).unwrap(),
            )
            .unwrap();
            assert!(
                crate::setup::verified_local_model_engine(root.path(), &manifest).is_err(),
                "Init retains full payload verification"
            );
            std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&receipt, &bytes).unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                false
            );
            std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o400)).unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                true
            );
        }

        #[tokio::test]
        async fn model_catalog_readiness_checks_declared_artifact_metadata_without_rehash() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let (_, provider) = readiness_provider(root.path(), &registry).await;
            let stage = Inventory::open(root.path(), false)
                .unwrap()
                .admitted(&record.admission_id)
                .unwrap();
            let weights = stage.path.join("weights.gguf");
            let original = std::fs::read(&weights).unwrap();
            let saved = root.path().join("fixture-weights");
            std::fs::rename(&weights, &saved).unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                false
            );
            std::os::unix::fs::symlink(&saved, &weights).unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                false
            );
            std::fs::remove_file(&weights).unwrap();
            std::fs::rename(&saved, &weights).unwrap();
            std::fs::write(&weights, b"GGUF").unwrap();
            assert_eq!(
                model_runtime_projection(
                    root.path(),
                    Some(&registry),
                    &context(),
                    &record.package_cid,
                    None
                )
                .await["dispatch_ready"],
                false
            );
            std::fs::write(&weights, &original).unwrap();
            let calls = backend.calls.lock().unwrap().clone();
            provider.hold.store(true, Ordering::Release);
            let read_root = root.path().to_path_buf();
            let read_registry = registry.clone();
            let cid = record.package_cid.clone();
            let pending = tokio::spawn(async move {
                model_runtime_projection(&read_root, Some(&read_registry), &context(), &cid, None)
                    .await
            });
            provider.entered.notified().await;
            // Same-size replacement during observation is detected by metadata,
            // without persisting stamps or claiming detection of all byte tampering.
            std::fs::write(&saved, &original).unwrap();
            std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::rename(&saved, &weights).unwrap();
            provider.release.notify_one();
            assert_eq!(pending.await.unwrap()["dispatch_ready"], false);
            assert_eq!(*backend.calls.lock().unwrap(), calls);
        }

        #[derive(Default)]
        struct ModelActivationFixture {
            busy: AtomicBool,
            lose_reply: AtomicBool,
            configured: Mutex<Option<serde_json::Value>>,
            calls: Mutex<Vec<serde_json::Value>>,
            hold: AtomicBool,
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }

        #[async_trait::async_trait]
        impl elastos_runtime::provider::Provider for ModelActivationFixture {
            fn name(&self) -> &'static str {
                "model-activation-fixture"
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
                panic!("configuration uses the private Init boundary")
            }
            async fn send_raw(
                &self,
                request: &serde_json::Value,
            ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
                if request["op"] == "offers_list" {
                    let mut offers = self
                        .configured
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|c| c["extra"]["offers"].as_array())
                        .cloned()
                        .unwrap_or_default();
                    offers.retain(|o| o["enabled"] == true);
                    for offer in &mut offers {
                        offer.as_object_mut().unwrap().remove("adapter");
                        offer.as_object_mut().unwrap().remove("enabled");
                        offer["stream_output"] = serde_json::json!(true);
                        offer["policy"]["schema"] = serde_json::json!("elastos.model.policy/v1");
                    }
                    return Ok(serde_json::json!({"status":"ok","data":{
                        "schema":"elastos.model.offers-list/v1","provider":"model-provider",
                        "protocol_version":"elastos.model-provider/v1","offers":offers}}));
                }
                assert_eq!(request["op"], "init");
                self.calls.lock().unwrap().push(request.clone());
                if self.hold.load(Ordering::Acquire) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                Ok(if self.busy.load(Ordering::Acquire) {
                    serde_json::json!({"status":"error", "code":"selection_unavailable"})
                } else {
                    *self.configured.lock().unwrap() = Some(request["config"].clone());
                    if self.lose_reply.swap(false, Ordering::AcqRel) {
                        return Err(elastos_runtime::provider::ProviderError::Provider(
                            "lost fixture Init reply".into(),
                        ));
                    }
                    serde_json::json!({"status":"ok", "data":{"provider":"model-provider",
                        "protocol_version":"elastos.model-provider/v1", "offers_ready":1}})
                })
            }
        }

        struct RetirementFixture {
            _engine: EngineFixture,
            root: tempfile::TempDir,
            old: PreparationRecord,
            pending: PreparationRecord,
            registry: Arc<elastos_runtime::provider::ProviderRegistry>,
            model: Arc<ModelActivationFixture>,
        }

        async fn retirement_fixture() -> RetirementFixture {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            let engine = install_engine(root.path());
            admit(root.path(), &record);
            let model = Arc::new(ModelActivationFixture::default());
            registry
                .register_sub_provider("model", model.clone())
                .await
                .unwrap();
            let worker = Inventory::open(root.path(), false)
                .unwrap()
                .worker_lock()
                .unwrap();
            let check: Revalidate = Arc::new(|| Ok(()));
            activate_admitted_model(
                root.path(),
                &registry,
                &record.operation_id,
                &AtomicBool::new(false),
                &check,
                &worker,
            )
            .await
            .unwrap();
            drop(worker);
            let old = load_operation(root.path(), &record.operation_id).unwrap();
            let alias = reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "old-alias",
                &old.package_cid,
            )
            .unwrap();
            update_operation(root.path(), &alias.operation_id, |r| {
                r.state = PreparationState::Admitted;
                r.index_bytes = old.index_bytes;
                r.completed_bytes = old.completed_bytes;
                r.activation_pending = false;
            })
            .unwrap();
            let cancelled = reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "cancelled-alias",
                &old.package_cid,
            )
            .unwrap();
            cancel(
                root.path(),
                &caller(&context(), &method("cancel")),
                &cancelled.operation_id,
            )
            .unwrap();
            let (mut catalog, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            let cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
            catalog["entries"][0]["cid"] = serde_json::json!(cid);
            write_preparation_catalog(root.path(), &catalog);
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"]["max_cache_bytes"] =
                    serde_json::json!(2 * preparation_charge(old.total_bytes).unwrap() - 1);
            });
            let pending = reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "retirement-use",
                cid,
            )
            .unwrap();
            RetirementFixture {
                _engine: engine,
                root,
                old,
                pending,
                registry,
                model,
            }
        }

        #[tokio::test]
        async fn model_retirement_descriptor_and_all_principal_keep_are_exact() {
            let f = retirement_fixture().await;
            let inventory = Inventory::open(f.root.path(), false).unwrap();
            let mut state = inventory.load().unwrap();
            assert_eq!(
                retirement_candidate(f.root.path(), &state).unwrap(),
                Some(f.old.operation_id.clone())
            );
            let mut altered = f.old.activation.clone().unwrap();
            altered.offer["adapter"]["settings"]["threads"] = serde_json::json!(1);
            assert!(altered.validate(&f.old).is_err());
            state.records[0].activation = None;
            assert!(
                state.validate().is_err(),
                "successful activation requires original evidence"
            );
            state.records[0].activation_pending = true;
            assert!(
                inventory.save(&state).is_err(),
                "stored evidence cannot be erased"
            );
            let mut state = inventory.load().unwrap();
            let alias = state
                .records
                .iter_mut()
                .find(|r| {
                    r.admission_id == f.old.operation_id && r.operation_id != f.old.operation_id
                })
                .unwrap();
            alias.request_binding.principal = "person:other-keeper".into();
            alias.operation_id = operation_id(alias).unwrap();
            let principal = alias.request_binding.principal.clone();
            state.retention_claims.push(RetentionClaim {
                principal,
                cid: f.old.package_cid.clone(),
            });
            inventory.save(&state).unwrap();
            assert_eq!(retirement_candidate(f.root.path(), &state).unwrap(), None);
            assert_eq!(state.records[0].reserved_bytes, f.old.reserved_bytes);
            assert!(f.old.projection().get("activation").is_none());
        }

        #[tokio::test]
        async fn model_retirement_failure_pins_exact_victim_until_withdrawal_and_removal() {
            for lost_reply in [false, true] {
                let f = retirement_fixture().await;
                f.model.busy.store(!lost_reply, Ordering::Release);
                f.model.lose_reply.store(lost_reply, Ordering::Release);
                let path = f.root.path().join(format!(
                    "model-preparation/admitted-{}/weights.gguf",
                    f.old.operation_id
                ));
                let bytes = std::fs::read(&path).unwrap();
                let worker = Inventory::open(f.root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .unwrap();
                let stop = AtomicBool::new(false);
                let check: Revalidate = Arc::new(|| Ok(()));
                assert!(prepare_capacity(
                    f.root.path(),
                    &f.registry,
                    &f.pending.operation_id,
                    &stop,
                    &check,
                    &worker
                )
                .await
                .is_err());
                let state = Inventory::open(f.root.path(), false)
                    .unwrap()
                    .load()
                    .unwrap();
                let retirement = state.retirement.clone().unwrap();
                assert_eq!(retirement.admission_id, f.old.operation_id);
                assert_eq!(retirement.phase, RetirementPhase::WithdrawalPending);
                assert_eq!(state.records[0], f.old);
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert!(
                    retention_intent(f.root.path(), &context(), &f.old.package_cid, true).is_err()
                );
                let cancelled = cancel(
                    f.root.path(),
                    &caller(&context(), &method("cancel")),
                    &f.pending.operation_id,
                )
                .unwrap();
                assert!(cancelled.cancel_requested);
                assert_eq!(cancelled.state, PreparationState::CapacityPending);
                let expired = manage(
                    f.root.path(),
                    &caller(&context(), &method("status")),
                    &f.pending.operation_id,
                    false,
                    f.pending.expires_at,
                )
                .unwrap();
                assert_eq!(expired.state, PreparationState::CapacityPending);
                f.model.busy.store(false, Ordering::Release);
                assert!(!prepare_capacity(
                    f.root.path(),
                    &f.registry,
                    &f.pending.operation_id,
                    &stop,
                    &check,
                    &worker
                )
                .await
                .unwrap());
                let state = Inventory::open(f.root.path(), false)
                    .unwrap()
                    .load()
                    .unwrap();
                assert!(state.retirement.is_none());
                assert!(state
                    .records
                    .iter()
                    .filter(|r| r.admission_id == f.old.operation_id)
                    .all(|r| r.reserved_bytes == 0
                        && if r.request_binding.request_id == "cancelled-alias" {
                            r.state == PreparationState::Cancelled
                        } else {
                            r.state == PreparationState::Reclaimed
                        }));
                assert!(!path.exists());
                let replay = reserve(
                    f.root.path(),
                    &caller(&context(), &method("use")),
                    &f.old.request_binding.request_id,
                    &f.old.package_cid,
                )
                .unwrap();
                assert_eq!(replay.state, PreparationState::Reclaimed);
                assert_eq!(
                    f.model.calls.lock().unwrap().len(),
                    3,
                    "one original Init and two exact withdrawal attempts"
                );
            }
        }

        #[tokio::test(start_paused = true)]
        async fn model_retirement_withheld_reply_keeps_worker_bytes_and_exact_target() {
            let f = retirement_fixture().await;
            f.model.hold.store(true, Ordering::Release);
            let worker = Inventory::open(f.root.path(), false)
                .unwrap()
                .worker_lock()
                .unwrap();
            let check: Revalidate = Arc::new(|| Ok(()));
            assert!(prepare_capacity(
                f.root.path(),
                &f.registry,
                &f.pending.operation_id,
                &AtomicBool::new(false),
                &check,
                &worker
            )
            .await
            .is_err());
            let state = Inventory::open(f.root.path(), false)
                .unwrap()
                .load()
                .unwrap();
            assert_eq!(
                state.retirement.as_ref().unwrap().admission_id,
                f.old.operation_id
            );
            assert_eq!(
                state.retirement.as_ref().unwrap().phase,
                RetirementPhase::WithdrawalPending
            );
            assert_eq!(state.records[0], f.old);
            assert!(Inventory::open(f.root.path(), false)
                .unwrap()
                .worker_lock()
                .is_err());
        }

        #[tokio::test]
        async fn model_retirement_operator_artifact_inside_admission_is_protected() {
            let f = retirement_fixture().await;
            let directory = f.root.path().join("providers/model-provider");
            std::fs::create_dir_all(&directory).unwrap();
            for path in [&directory, &f.root.path().join("providers")] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            let mut operator = f.old.activation.as_ref().unwrap().offer.clone();
            operator["id"] = serde_json::json!("operator-model");
            let path = directory.join("config.json");
            std::fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({"offers":[operator]})).unwrap(),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let state = Inventory::open(f.root.path(), false)
                .unwrap()
                .load()
                .unwrap();
            assert_eq!(retirement_candidate(f.root.path(), &state).unwrap(), None);
        }

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_MODEL_PROVIDER_PATH; private startup retirement recovery"]
        async fn model_retirement_native_startup_withdraws_before_registration() {
            let f = retirement_fixture().await;
            f.model.busy.store(true, Ordering::Release);
            let worker = Inventory::open(f.root.path(), false)
                .unwrap()
                .worker_lock()
                .unwrap();
            let check: Revalidate = Arc::new(|| Ok(()));
            assert!(prepare_capacity(
                f.root.path(),
                &f.registry,
                &f.pending.operation_id,
                &AtomicBool::new(false),
                &check,
                &worker
            )
            .await
            .is_err());
            drop(worker);
            let registry = elastos_runtime::provider::ProviderRegistry::new();
            let mut config = crate::api::model_provider_bridge_config(f.root.path()).unwrap();
            let worker =
                append_admitted_model_startup_offers(f.root.path(), &registry, &mut config)
                    .await
                    .unwrap();
            let binary = std::path::PathBuf::from(
                std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH").unwrap(),
            );
            assert!(binary.is_absolute());
            let bridge = elastos_runtime::provider::ProviderBridge::spawn(&binary, config.clone())
                .await
                .unwrap();
            let settled =
                settle_pending_model_startup(f.root.path(), &bridge, &config, worker.as_ref())
                    .await;
            let offers = bridge
                .send_raw(&serde_json::json!({"op":"offers_list"}))
                .await;
            bridge.shutdown().await.unwrap();
            settled.unwrap();
            assert!(!registry
                .sub_provider_schemes()
                .await
                .iter()
                .any(|scheme| scheme == "model"));
            assert_eq!(offers.unwrap()["data"]["offers"], serde_json::json!([]));
            assert_eq!(
                load_operation(f.root.path(), &f.old.operation_id)
                    .unwrap()
                    .state,
                PreparationState::Reclaimed
            );
        }

        #[tokio::test]
        async fn model_retirement_withdrawn_partial_removal_and_unsafe_tree_recover_exactly() {
            use std::os::unix::fs::symlink;
            for fault in ["partial", "symlink", "hardlink", "mode"] {
                let f = retirement_fixture().await;
                let worker = Inventory::open(f.root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .unwrap();
                let inventory = Inventory::open(f.root.path(), false).unwrap();
                let mut state = inventory.load().unwrap();
                state.retirement = Some(ModelRetirement {
                    operation_id: f.pending.operation_id.clone(),
                    admission_id: f.old.operation_id.clone(),
                    phase: RetirementPhase::Withdrawn,
                });
                inventory.save(&state).unwrap();
                drop(inventory);
                let directory = f
                    .root
                    .path()
                    .join(format!("model-preparation/admitted-{}", f.old.operation_id));
                let weights = directory.join("weights.gguf");
                let protected = f.root.path().join("protected-fixture");
                std::fs::write(&protected, b"preserved").unwrap();
                let injected = directory.join("injected");
                match fault {
                    "partial" => std::fs::remove_file(&weights).unwrap(),
                    "symlink" => symlink(&protected, &injected).unwrap(),
                    "hardlink" => std::fs::hard_link(&protected, &injected).unwrap(),
                    _ => std::fs::set_permissions(&weights, std::fs::Permissions::from_mode(0o644))
                        .unwrap(),
                }
                if fault != "partial" {
                    assert!(finish_model_retirement(f.root.path()).is_err());
                    let remaining = load_operation(f.root.path(), &f.old.operation_id).unwrap();
                    assert_eq!(remaining.reserved_bytes, f.old.reserved_bytes);
                    assert_eq!(std::fs::read(&protected).unwrap(), b"preserved");
                    if fault == "mode" {
                        std::fs::set_permissions(&weights, std::fs::Permissions::from_mode(0o600))
                            .unwrap();
                    } else {
                        std::fs::remove_file(&injected).unwrap();
                    }
                }
                finish_model_retirement(f.root.path()).unwrap();
                assert!(!directory.exists());
                assert_eq!(
                    load_operation(f.root.path(), &f.old.operation_id)
                        .unwrap()
                        .state,
                    PreparationState::Reclaimed
                );
                drop(worker);
            }
        }

        #[tokio::test]
        async fn model_preparation_enabled_activation_hashes_each_verification_phase() {
            let root = tempfile::tempdir().unwrap();
            let (payload, files) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            write_preparation_catalog(root.path(), &payload);
            let cid = payload["entries"][0]["cid"].as_str().unwrap().to_owned();
            let backend = Arc::new(PreparationBackend::new(files, cid.clone()));
            let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
            registry
                .register_sub_provider("ipfs", backend.clone())
                .await
                .unwrap();
            register_content(&registry, root.path()).await;
            let _engine = install_engine(root.path());
            let owner = PreparationOwner::default();
            let input = serde_json::json!({"cid":cid});
            let first = owner
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(&context(), &method("use")),
                    "phase-first",
                    &input,
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            join_worker(&owner).await;
            let hashes = || {
                backend
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|op| *op == "runtime_hash_staged_directory")
                    .count()
            };
            let record =
                load_operation(root.path(), first["operation_id"].as_str().unwrap()).unwrap();
            assert_eq!(record.state, PreparationState::Admitted);
            assert!(record.activation_pending); // Exact config verified, but model slot is absent.
            assert_eq!(hashes(), 2); // Admission, then activation composer.
            let reads = backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|op| *op == "cat")
                .count();
            let model = Arc::new(ModelActivationFixture::default());
            registry
                .register_sub_provider("model", model.clone())
                .await
                .unwrap();
            let reopened = PreparationOwner::default();
            let reused = reopened
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(&context(), &method("use")),
                    "phase-reuse",
                    &input,
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            join_worker(&reopened).await;
            let alias =
                load_operation(root.path(), reused["operation_id"].as_str().unwrap()).unwrap();
            assert_eq!(alias.admission_id, record.admission_id);
            assert_eq!(alias.reserved_bytes, 0);
            assert!(!alias.activation_pending);
            assert_eq!(model.calls.lock().unwrap().len(), 1);
            assert_eq!(hashes(), 4); // Reuse admission, then activation composer.
            for expected in [5, 6] {
                let (_, guard) = crate::api::model_provider_config(root.path(), &registry)
                    .await
                    .unwrap();
                drop(guard);
                assert_eq!(hashes(), expected); // Each provider startup/restart composition.
            }
            assert_eq!(
                backend
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|op| *op == "cat")
                    .count(),
                reads
            );
        }

        #[tokio::test]
        async fn model_refresh_holds_inventory_worker_through_projected_registry_init() {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            let _engine = install_engine(root.path());
            let operator = serde_json::json!({
                "id":"operator-owned", "title":"Operator model", "enabled":false,
                "operation":"image.generate", "input_modalities":["application/json"],
                "output_modalities":["application/json"],
                "policy":{"concurrency_limit":1,"input_bytes_limit":32768,
                    "inline_output_bytes_limit":65536,"event_bytes_limit":4096,
                    "runtime_ms_limit":120000,"retention_secs":3600,
                    "cancel_settlement_timeout_ms":15000},
                "adapter":{"kind":"http_job_artifact", "create_url":"https://operator.invalid/create",
                    "status_url":"https://operator.invalid/status", "cancel_url":null,
                    "bearer_token":null, "poll_interval_ms":1000}
            });
            let config_dir = root.path().join("providers/model-provider");
            std::fs::create_dir_all(&config_dir).unwrap();
            for dir in [root.path().join("providers"), config_dir.clone()] {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            let config_path = config_dir.join("config.json");
            let operator_bytes =
                serde_json::to_vec(&serde_json::json!({"offers":[operator.clone()]})).unwrap();
            std::fs::write(&config_path, &operator_bytes).unwrap();
            std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let model = Arc::new(ModelActivationFixture::default());
            model.hold.store(true, Ordering::Release);
            registry
                .register_sub_provider("model", model.clone())
                .await
                .unwrap();
            let slot = registry
                .registration_for_uri("elastos://model/")
                .await
                .unwrap();
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
            let entered =
                tokio::time::timeout(std::time::Duration::from_secs(5), model.entered.notified())
                    .await;
            let worker_held = Inventory::open(root.path(), false)
                .unwrap()
                .worker_lock()
                .is_err();
            let status = load_operation(root.path(), &record.operation_id).unwrap();
            let kept = retention_intent(root.path(), &context(), &record.package_cid, true);
            let calls = model.calls.lock().unwrap().clone();
            // Poll the real replacement operation once; it must wait on Init's slot guard.
            let replacement_blocked = tokio::select! {
                biased;
                _ = registry.unregister_sub_provider("model") => false,
                _ = std::future::ready(()) => true,
            };
            // Always release and join before RED assertions so fixture cleanup owns all work.
            model.release.notify_one();
            join_worker(&owner).await;
            assert!(
                entered.is_ok(),
                "activation did not enter the Registry Init barrier"
            );
            assert_eq!(calls.len(), 1);
            assert!(
                replacement_blocked,
                "Registry replacement passed an in-flight Init"
            );
            assert_eq!(status.state, PreparationState::Admitted);
            assert_eq!(
                kept.unwrap()["kept"],
                true,
                "Keep must remain responsive during Init"
            );
            let offers = calls[0]["config"]["extra"]["offers"].as_array().unwrap();
            assert_eq!(offers.len(), 2);
            assert_eq!(offers[0], operator);
            let projection = &calls[0]["config"]["extra"]["runtime_admitted_offers"];
            let expected = serde_json::json!([{"offer_id":offers[1]["id"]}]);
            assert_eq!(std::fs::read(&config_path).unwrap(), operator_bytes);
            assert_eq!(
                registry
                    .registration_for_uri("elastos://model/")
                    .await
                    .unwrap(),
                slot
            );
            assert!(Inventory::open(root.path(), false)
                .unwrap()
                .worker_lock()
                .is_ok());
            assert!(worker_held && projection == &expected,
                "activation must retain inventory worker through Init and project only the admitted offer: worker_held={worker_held}, projection={projection}, expected={expected}");
        }

        #[tokio::test]
        async fn model_startup_guard_retained_across_init_result_and_retry() {
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            // Composition failure releases the existing worker guard for retry.
            assert!(crate::api::model_provider_config(root.path(), &registry)
                .await
                .is_err());
            assert!(Inventory::open(root.path(), false)
                .unwrap()
                .worker_lock()
                .is_ok());
            let _engine = install_engine(root.path());
            let model = Arc::new(ModelActivationFixture::default());
            model.hold.store(true, Ordering::Release);
            registry
                .register_sub_provider("model", model.clone())
                .await
                .unwrap();
            let bytes = std::fs::read(
                root.path()
                    .join("model-preparation")
                    .join(format!("admitted-{}", record.admission_id))
                    .join("weights.gguf"),
            )
            .unwrap();
            for busy in [true, false] {
                model.busy.store(busy, Ordering::Release);
                let (config, worker) = crate::api::model_provider_config(root.path(), &registry)
                    .await
                    .unwrap();
                assert!(worker.is_some());
                assert!(Inventory::open(root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .is_err());
                let init = registry.refresh_local_model_configuration(&config);
                let during_init = async {
                    let entered = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        model.entered.notified(),
                    )
                    .await;
                    let held = Inventory::open(root.path(), false)
                        .unwrap()
                        .worker_lock()
                        .is_err();
                    let snapshot = Inventory::open(root.path(), false).unwrap().load();
                    model.release.notify_one();
                    assert!(entered.is_ok());
                    assert!(held);
                    assert!(
                        snapshot.is_ok(),
                        "short inventory lock must remain available"
                    );
                };
                let (result, ()) = tokio::join!(init, during_init);
                assert_eq!(result.is_err(), busy);
                // Startup's caller owns this File until the Init result is handled.
                assert!(Inventory::open(root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .is_err());
                drop(worker);
                assert!(Inventory::open(root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .is_ok());
                assert_eq!(
                    load_operation(root.path(), &record.operation_id)
                        .unwrap()
                        .reserved_bytes,
                    record.reserved_bytes
                );
            }
            let calls = model.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0], calls[1]);
            assert_eq!(
                calls[0]["config"]["extra"]["runtime_admitted_offers"],
                serde_json::json!([{"offer_id":calls[0]["config"]["extra"]["offers"][0]["id"]}])
            );
            assert_eq!(
                std::fs::read(
                    root.path()
                        .join("model-preparation")
                        .join(format!("admitted-{}", record.admission_id))
                        .join("weights.gguf")
                )
                .unwrap(),
                bytes
            );
        }

        #[tokio::test]
        async fn model_refresh_admission_pending_retry_reuses_exact_artifact_and_slot() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            let _engine = install_engine(root.path());
            let model = Arc::new(ModelActivationFixture::default());
            model.busy.store(true, Ordering::Release);
            registry
                .register_sub_provider("model", model.clone())
                .await
                .unwrap();
            let slot = registry
                .registration_for_uri("elastos://model/")
                .await
                .unwrap();
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
            let pending = load_operation(root.path(), &record.operation_id).unwrap();
            assert!(Inventory::open(root.path(), false)
                .unwrap()
                .worker_lock()
                .is_ok());
            assert_eq!(pending.state, PreparationState::Admitted);
            assert_eq!(pending.projection()["activation_pending"], true);
            assert_eq!(pending.reserved_bytes, record.reserved_bytes);
            let stage = Inventory::open(root.path(), false)
                .unwrap()
                .admitted(&record.admission_id)
                .unwrap();
            let original = std::fs::read(stage.path.join("weights.gguf")).unwrap();
            let first = model.calls.lock().unwrap()[0].clone();
            model.busy.store(false, Ordering::Release);
            for _ in 0..2 {
                owner
                    .invoke(
                        root.path(),
                        Some(registry.clone()),
                        caller(&context(), &method("use")),
                        &record.request_binding.request_id,
                        &serde_json::json!({"cid":record.package_cid}),
                        Arc::new(|| Ok(())),
                    )
                    .unwrap();
                join_worker(&owner).await;
                let admitted = load_operation(root.path(), &record.operation_id).unwrap();
                assert!(Inventory::open(root.path(), false)
                    .unwrap()
                    .worker_lock()
                    .is_ok());
                assert_eq!(admitted.state, PreparationState::Admitted);
                assert_eq!(admitted.projection()["activation_pending"], false);
                assert!(admitted.projection().get("inference_ready").is_none());
                assert_eq!(
                    std::fs::read(stage.path.join("weights.gguf")).unwrap(),
                    original
                );
                assert_eq!(
                    slot,
                    registry
                        .registration_for_uri("elastos://model/")
                        .await
                        .unwrap()
                );
            }
            assert!(backend.calls.lock().unwrap().iter().all(|op| op != "cat"));
            assert_eq!(
                *model.calls.lock().unwrap(),
                vec![first.clone(), first.clone(), first]
            );
        }

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_MODEL_PROVIDER_PATH; actual same-process Init refresh proof"]
        async fn model_refresh_real_provider_admission_uses_existing_init_and_slot() {
            use elastos_runtime::provider::{CapsuleProvider, ProviderBridge};
            let binary = std::path::PathBuf::from(
                std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH")
                    .expect("explicit provider binary"),
            );
            assert!(binary.is_absolute());
            let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
            let _engine = install_engine(root.path());
            let initial = crate::api::model_provider_bridge_config(root.path()).unwrap();
            assert_eq!(
                std::path::Path::new(&initial.base_path),
                root.path().canonicalize().unwrap()
            );
            let bridge = Arc::new(ProviderBridge::spawn(&binary, initial).await.unwrap());
            registry
                .register_sub_provider(
                    "model",
                    Arc::new(CapsuleProvider::with_scheme(bridge.clone(), "model")),
                )
                .await
                .unwrap();
            let slot = registry
                .registration_for_uri("elastos://model/")
                .await
                .unwrap();
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
            let admitted = load_operation(root.path(), &record.operation_id).unwrap();
            let first = bridge
                .send_raw(&serde_json::json!({"op":"offers_list"}))
                .await;
            let registry_offers = registry.local_model_offers().await;
            let first_projection = model_runtime_projection(
                root.path(),
                Some(&registry),
                &context(),
                &record.package_cid,
                Some(&record.operation_id),
            )
            .await;
            owner
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(&context(), &method("use")),
                    &record.request_binding.request_id,
                    &serde_json::json!({"cid":record.package_cid}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            join_worker(&owner).await;
            let second = bridge
                .send_raw(&serde_json::json!({"op":"offers_list"}))
                .await;
            let second_projection = model_runtime_projection(
                root.path(),
                Some(&registry),
                &context(),
                &record.package_cid,
                Some(&record.operation_id),
            )
            .await;
            // A remains active in this real provider after the one-entry signed
            // catalog moves to B. Its retirement cannot use B's catalog facts.
            let (mut next_catalog, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            let next_cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
            next_catalog["entries"][0]["cid"] = serde_json::json!(next_cid);
            write_preparation_catalog(root.path(), &next_catalog);
            let charge = preparation_charge(record.total_bytes).unwrap();
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"]["max_cache_bytes"] =
                    serde_json::json!(2 * charge - 1);
            });
            let pressure_use = owner.invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("use")),
                "next-catalog-pressure",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(())),
            );
            let pressure_worker = owner.worker.lock().unwrap().take();
            let pressure_settled = if let Some(mut task) = pressure_worker {
                match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
                    Ok(result) => result.is_ok(),
                    Err(_) => {
                        task.abort();
                        let _ = task.await;
                        false
                    }
                }
            } else {
                true
            };
            let retired = load_operation(root.path(), &record.operation_id);
            let after_pressure = registry.local_model_offers().await;
            let old_directory_exists = root
                .path()
                .join(format!(
                    "model-preparation/admitted-{}",
                    record.admission_id
                ))
                .exists();
            bridge.shutdown().await.unwrap();
            assert_eq!(admitted.state, PreparationState::Admitted);
            assert!(!admitted.activation_pending);
            assert_eq!(
                slot,
                registry
                    .registration_for_uri("elastos://model/")
                    .await
                    .unwrap()
            );
            let first = first.unwrap();
            assert_eq!(first["data"]["offers"].as_array().unwrap().len(), 1);
            assert_eq!(
                registry_offers.expect("Registry accepts the real native offers envelope"),
                *first["data"]["offers"].as_array().unwrap()
            );
            for projection in [first_projection, second_projection] {
                assert_eq!(projection["dispatch_ready"], true);
                assert_eq!(projection["offer_id"], first["data"]["offers"][0]["id"]);
            }
            assert_eq!(first, second.unwrap());
            assert!(
                pressure_settled,
                "pressure worker must settle within its bound"
            );
            assert_eq!(pressure_use.unwrap()["cid"], next_cid);
            let retired = retired.unwrap();
            assert_eq!(
                serde_json::to_value(&retired.state).unwrap(),
                "reclaimed",
                "pressure Use must retire the exact prior-catalog admission"
            );
            assert_eq!(retired.reserved_bytes, 0);
            assert!(!old_directory_exists);
            assert!(after_pressure
                .unwrap()
                .iter()
                .all(|offer| { offer["id"] != first["data"]["offers"][0]["id"] }));
        }

        #[tokio::test]
        async fn model_startup_binding_requires_admission_and_preserves_operator_config() {
            let empty = tempfile::tempdir().unwrap();
            let registry = elastos_runtime::provider::ProviderRegistry::new();
            let mut actual = config(empty.path());
            let before = serde_json::to_value(&actual).unwrap();
            let _ = append_admitted_model_startup_offers(empty.path(), &registry, &mut actual)
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(&actual).unwrap(), before);
            assert!(!empty.path().join("model-preparation").exists());

            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            let mut actual = config(root.path());
            let before = actual.extra.clone();
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
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
        async fn model_startup_prepares_cold_verifier_before_reusing_admitted_package() {
            for unavailable in [false, true] {
                let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
                admit(root.path(), &record);
                let _engine = install_engine(root.path());
                backend.cold_backend.store(true, Ordering::Release);
                backend.fail_drain.store(unavailable, Ordering::Release);
                let mut actual = config(root.path());
                let before = actual.extra.clone();
                let result =
                    append_admitted_model_startup_offers(root.path(), &registry, &mut actual).await;
                if unavailable {
                    assert!(result.is_err());
                    assert_eq!(
                        actual.extra, before,
                        "failed readiness preserves configuration"
                    );
                    assert_eq!(*backend.calls.lock().unwrap(), ["runtime_prepare_backend"]);
                } else {
                    assert!(
                        result.is_ok(),
                        "cold verifier must become ready before hashing"
                    );
                    assert_eq!(actual.extra["offers"].as_array().unwrap().len(), 2);
                    assert_eq!(
                        *backend.calls.lock().unwrap(),
                        ["runtime_prepare_backend", "runtime_hash_staged_directory"]
                    );
                }
                assert!(
                    backend.read_sizes.lock().unwrap().is_empty(),
                    "restart reuses admitted bytes without Content reads"
                );
                let stage = root
                    .path()
                    .join("model-preparation")
                    .join(format!("admitted-{}", record.admission_id));
                for (name, bytes) in &backend.files {
                    assert_eq!(
                        &std::fs::read(stage.join(name)).unwrap(),
                        bytes,
                        "admitted package remains intact"
                    );
                }
            }
        }

        #[tokio::test]
        async fn model_startup_binding_consumes_admission_with_stable_restart_and_alias_identity() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let mut first = config(root.path());
            let operator = first.extra["offers"][0].clone();
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut first)
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
                ["runtime_prepare_backend", "runtime_hash_staged_directory"]
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
                alias.activation = None;
                state.records.push(alias);
                inventory.save(&state).unwrap();
            }
            let state_path = root.path().join("model-preparation/state.json");
            let persisted = std::fs::read(&state_path).unwrap();
            let mut restarted = config(root.path());
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut restarted)
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
        async fn model_retention_change_during_startup_verification_preserves_offer_binding() {
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            let _engine = install_engine(root.path());
            let mut expected = config(root.path());
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut expected)
                .await
                .unwrap();
            let records = Inventory::open(root.path(), false)
                .unwrap()
                .load()
                .unwrap()
                .records;
            let mut actual = config(root.path());
            backend.hold_hash.store(true, Ordering::Release);
            let verify = append_admitted_model_startup_offers(root.path(), &registry, &mut actual);
            let keep = async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend.entered.notified(),
                )
                .await
                .unwrap();
                let reply =
                    retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
                assert_eq!(reply["kept"], true);
                backend.release.notify_one();
            };
            let (result, ()) = tokio::join!(verify, keep);
            let _ =
                result.expect("independent Keep must not invalidate unchanged startup admission");
            assert_eq!(actual.extra, expected.extra);
            let current = Inventory::open(root.path(), false).unwrap().load().unwrap();
            assert_eq!(current.records, records);
            assert!(current.kept(&context().principal_id, &record.package_cid));
            assert_eq!(
                backend.calls.lock().unwrap().as_slice(),
                [
                    "runtime_prepare_backend",
                    "runtime_hash_staged_directory",
                    "runtime_prepare_backend",
                    "runtime_hash_staged_directory"
                ]
            );
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
                let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
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
            let _ = append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
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
            let worker = append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                .await
                .unwrap();
            let id = actual.extra["offers"][0]["id"].clone();
            let bridge = ProviderBridge::spawn(&binary, actual).await.unwrap();
            drop(worker);
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

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_MODEL_PROVIDER_PATH and /usr/bin/python3; isolated small process proof"]
        async fn model_startup_binding_real_provider_long_output_cancel_restart() {
            use elastos_model_contract::{
                model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding,
                RUNTIME_ACCESS_BINDING_SCHEMA, RUNTIME_CREATE_BINDING_SCHEMA,
            };
            use elastos_runtime::provider::ProviderBridge;
            use std::time::Duration;

            let binary = std::path::PathBuf::from(
                std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH")
                    .expect("explicit model-provider prerequisite required"),
            );
            assert!(binary.is_absolute() && std::fs::symlink_metadata(&binary).unwrap().is_file());
            let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
            admit(root.path(), &record);
            // Same verified receipt/path admission as the Init fixture; only the
            // engine's isolated HTTP response is deterministic, not model inference.
            let _engine = install_engine_bytes(root.path(), br#"#!/usr/bin/python3
import http.server, json, sys, threading
def arg(name):
    return sys.argv[sys.argv.index(name) + 1]
alias = arg('--alias')
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        body = {'status': 'ok'} if self.path == '/health' else {'data': [{'id': alias}]}
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert self.path == '/v1/chat/completions' and body['model'] == alias
        assert body['chat_template_kwargs'] == {'enable_thinking': False}
        prompt = body['messages'][0]['content']
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        empty = json.dumps({'schema': 'elastos.model.output.text/v1', 'text': ''}, separators=(',', ':'))
        text = 'x' * (65536 - len(empty)) if prompt == 'complete' else 'x' * 8192
        for offset in range(0, len(text), 8192):
            payload = json.dumps({'choices': [{'delta': {'content': text[offset:offset+8192]}}]})
            self.wfile.write(('data: ' + payload + '\n\n').encode())
            self.wfile.flush()
        if prompt == 'cancel':
            threading.Event().wait() # No backend stop acknowledgement is claimed.
        else:
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
server = http.server.ThreadingHTTPServer(('127.0.0.1', int(arg('--port'))), Handler)
server.daemon_threads = True
server.serve_forever()
"#);
            let mut actual = config(root.path());
            actual.extra["offers"] = serde_json::json!([]);
            let worker = append_admitted_model_startup_offers(root.path(), &registry, &mut actual)
                .await
                .unwrap();
            let offer = actual.extra["offers"][0]["id"].as_str().unwrap().to_owned();
            let policy = actual.extra["offers"][0]["policy"].clone();
            assert_eq!(policy["inline_output_bytes_limit"], 65536);
            let bridge = Arc::new(
                ProviderBridge::spawn(&binary, actual.clone())
                    .await
                    .unwrap(),
            );
            drop(worker);
            let active = bridge.clone();
            let mut task = tokio::spawn(async move {
                let ctx = context();
                let mut replays = Vec::new();
                for prompt in ["complete", "cancel"] {
                    let input =
                        serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":prompt});
                    let binding = RuntimeCreateBinding {
                        schema: RUNTIME_CREATE_BINDING_SCHEMA.into(),
                        principal_id: ctx.principal_id.clone(),
                        session_id: ctx.session_id.clone(),
                        capsule_id: "assistant".into(),
                        grant_id: ctx.grant_id.clone(),
                        request_id: format!("small-output-{prompt}"),
                        offer_id: offer.clone(),
                        operation: "text.generate".into(),
                        input_hash: model_input_hash(&input).unwrap(),
                    };
                    let created = active
                        .send_raw(&serde_json::json!({"op":"runs_create","offer_id":offer,
                        "operation":"text.generate","input":input,"runtime_binding":binding}))
                        .await
                        .unwrap();
                    assert_eq!(created["status"], "ok");
                    let id = created["data"]["run_id"].as_str().unwrap();
                    let access = RuntimeAccessBinding {
                        schema: RUNTIME_ACCESS_BINDING_SCHEMA.into(),
                        principal_id: binding.principal_id,
                        session_id: binding.session_id,
                        capsule_id: binding.capsule_id,
                        grant_id: binding.grant_id,
                        request_id: binding.request_id,
                        run_id: id.into(),
                    };
                    let get =
                        serde_json::json!({"op":"runs_get","run_id":id,"runtime_binding":access});
                    let events = serde_json::json!({"op":"runs_events","run_id":id,"after_sequence":0,"runtime_binding":access});
                    let cancel = serde_json::json!({"op":"runs_cancel","run_id":id,"runtime_binding":access});
                    let mut cancel_sent = false;
                    let terminal = loop {
                        let current = active.send_raw(&get).await.unwrap();
                        assert_eq!(current["status"], "ok");
                        if !matches!(
                            current["data"]["status"].as_str(),
                            Some("prepared" | "running" | "reconciling")
                        ) {
                            break current;
                        }
                        if prompt == "cancel"
                            && !cancel_sent
                            && current["data"]["status"] == "running"
                        {
                            let page = active.send_raw(&events).await.unwrap();
                            if page["data"]["events"].as_array().unwrap().iter().any(|e| {
                                e["kind"] == "text_delta"
                                    && !e["data"]["text"].as_str().unwrap().is_empty()
                            }) {
                                assert_eq!(active.send_raw(&cancel).await.unwrap()["status"], "ok");
                                cancel_sent = true;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    };
                    let page = active.send_raw(&events).await.unwrap();
                    assert_eq!(page["status"], "ok");
                    assert_eq!(page["data"]["has_more"], false);
                    let recorded = page["data"]["events"].as_array().unwrap();
                    assert_eq!(
                        recorded
                            .iter()
                            .filter(|e| e["kind"] == "dispatched")
                            .count(),
                        1
                    );
                    for event in recorded {
                        // Actual provider envelope at the maximum JS-exact sequence,
                        // including the full typed terminal output, must fit policy.
                        let mut largest = event.clone();
                        largest["sequence"] = serde_json::json!(9_007_199_254_740_991u64);
                        assert!(
                            serde_json::to_vec(&largest).unwrap().len() as u64
                                <= policy["event_bytes_limit"].as_u64().unwrap()
                        );
                    }
                    if prompt == "complete" {
                        assert_eq!(terminal["data"]["status"], "completed");
                        let output = &terminal["data"]["terminal"]["output"];
                        assert_eq!(serde_json::to_vec(output).unwrap().len(), 65536);
                        let text: String = recorded
                            .iter()
                            .filter(|e| e["kind"] == "text_delta")
                            .map(|e| e["data"]["text"].as_str().unwrap())
                            .collect();
                        assert_eq!(text, output["text"].as_str().unwrap());
                        assert_eq!(
                            recorded.iter().find(|e| e["kind"] == "output").unwrap()["data"],
                            *output
                        );
                    } else {
                        assert!(cancel_sent, "must cancel only after an active delta");
                        assert_eq!(terminal["data"]["status"], "settlement_unknown");
                        assert_eq!(
                            terminal["data"]["terminal"]["error"]["class"],
                            "settlement_unknown"
                        );
                    }
                    for request in [get, cancel] {
                        assert_eq!(active.send_raw(&request).await.unwrap(), terminal);
                        replays.push((request, terminal.clone()));
                    }
                    assert_eq!(active.send_raw(&events).await.unwrap(), page);
                    replays.push((events, page));
                }
                replays
            });
            let result = tokio::time::timeout(Duration::from_secs(60), &mut task).await;
            if result.is_err() {
                task.abort();
                let _ = task.await;
            }
            bridge
                .shutdown()
                .await
                .expect("reap first provider and its fixture engine before assertions");
            let replays = result.unwrap().unwrap();
            let mut restarted_config = config(root.path());
            restarted_config.extra["offers"] = serde_json::json!([]);
            let worker =
                append_admitted_model_startup_offers(root.path(), &registry, &mut restarted_config)
                    .await
                    .unwrap();
            assert_eq!(restarted_config.extra, actual.extra);
            let restarted = Arc::new(
                ProviderBridge::spawn(&binary, restarted_config)
                    .await
                    .unwrap(),
            );
            drop(worker);
            let mut results = Vec::new();
            for (request, expected) in replays {
                results.push((restarted.send_raw(&request).await, expected));
            }
            registry
                .register_sub_provider(
                    "model",
                    Arc::new(elastos_runtime::provider::CapsuleProvider::with_scheme(
                        restarted.clone(),
                        "model",
                    )),
                )
                .await
                .unwrap();
            let (mut next_catalog, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            let next_cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
            next_catalog["entries"][0]["cid"] = serde_json::json!(next_cid);
            write_preparation_catalog(root.path(), &next_catalog);
            change_config(root.path(), |config| {
                config["model_catalog"]["local_use"]["max_cache_bytes"] =
                    serde_json::json!(2 * preparation_charge(record.total_bytes).unwrap() - 1);
            });
            let owner = PreparationOwner::default();
            let pressure = owner.invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("use")),
                "unknown-run-pressure",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(())),
            );
            join_worker(&owner).await;
            let retained = Inventory::open(root.path(), false).unwrap().load().unwrap();
            restarted
                .shutdown()
                .await
                .expect("reap restarted provider before assertions");
            pressure.unwrap();
            assert_eq!(
                retained.retirement.as_ref().unwrap().phase,
                RetirementPhase::WithdrawalPending
            );
            assert_eq!(
                retained.retirement.as_ref().unwrap().admission_id,
                record.admission_id
            );
            assert_eq!(retained.records[0].state, PreparationState::Admitted);
            assert_eq!(retained.records[0].reserved_bytes, record.reserved_bytes);
            let startup_registry = elastos_runtime::provider::ProviderRegistry::new();
            let mut recovery_config =
                crate::api::model_provider_bridge_config(root.path()).unwrap();
            let worker = append_admitted_model_startup_offers(
                root.path(),
                &startup_registry,
                &mut recovery_config,
            )
            .await
            .unwrap();
            let recovering = ProviderBridge::spawn(&binary, recovery_config.clone())
                .await
                .unwrap();
            let withdrawal = settle_pending_model_startup(
                root.path(),
                &recovering,
                &recovery_config,
                worker.as_ref(),
            )
            .await;
            recovering.shutdown().await.unwrap();
            assert!(
                withdrawal.is_err(),
                "unknown run stays protected through private startup retirement"
            );
            assert!(!startup_registry
                .sub_provider_schemes()
                .await
                .iter()
                .any(|scheme| scheme == "model"));
            assert!(Inventory::open(root.path(), false).unwrap().load().unwrap() == retained);
            for (result, expected) in results {
                assert_eq!(result.unwrap(), expected);
            }
            assert!(
                !backend.calls.lock().unwrap().iter().any(|op| op == "cat"),
                "startup/replay must not read Content bytes"
            );
        }
    }

    impl PreparationBackend {
        fn new(files: std::collections::BTreeMap<String, Vec<u8>>, cid: String) -> Self {
            Self {
                files,
                cid: Mutex::new(cid),
                calls: Mutex::new(vec![]),
                capacity_requests: Mutex::new(vec![]),
                capacity_volume: 7,
                fail_capacity_at: None,
                read_sizes: Mutex::new(vec![]),
                after_weight_read: Mutex::new(None),
                hold_drain: AtomicBool::new(false),
                fail_drain: AtomicBool::new(false),
                cold_backend: AtomicBool::new(false),
                hold_read: AtomicBool::new(false),
                hold_hash: AtomicBool::new(false),
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
        let (root, admitted, _, registry) = staged_fixture(now().unwrap(), true).await;
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
        let owner = PreparationOwner::default();
        let pending = owner
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("use")),
                "next-package",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        assert_eq!(pending["state"], "capacity_pending");
        assert!(owner.worker.lock().unwrap().is_none());
        let pending_bytes = std::fs::read(&state_path).unwrap();

        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] = serde_json::json!(2 * charge)
        });
        let retried = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "next-package",
            next_cid,
        )
        .unwrap();
        assert_eq!(retried.state, PreparationState::CapacityPending);
        assert_eq!(retried.reserved_bytes, 0);
        assert_eq!(std::fs::read(&state_path).unwrap(), pending_bytes);
        let started = owner
            .invoke(
                root.path(),
                Some(registry),
                caller(&context(), &method("use")),
                "next-package",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        assert_eq!(started["operation_id"], pending["operation_id"]);
        assert_eq!(started["state"], "preparing");
        let reserved = load_operation(root.path(), &retried.operation_id).unwrap();
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
        join_worker(&owner).await;
    }

    #[tokio::test]
    async fn model_preparation_pressure_persists_exact_zero_charge_pending_use() {
        let (root, admitted, backend, registry) = staged_fixture(now().unwrap(), true).await;
        update_operation(root.path(), &admitted.operation_id, |record| {
            record.state = PreparationState::Admitted
        })
        .unwrap();
        let charge = preparation_charge(admitted.total_bytes).unwrap();
        let (mut payload, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
        let next_cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
        assert_ne!(next_cid, admitted.package_cid);
        payload["entries"][0]["cid"] = serde_json::json!(next_cid);
        write_preparation_catalog(root.path(), &payload);
        let state_path = root.path().join("model-preparation/state.json");
        let original = std::fs::read(&state_path).unwrap();
        let weights_path = root.path().join(format!(
            "model-preparation/admitted-{}/weights.gguf",
            admitted.admission_id
        ));
        let original_weights = std::fs::read(&weights_path).unwrap();

        // Pressure cannot relax the unchanged identity and per-model gates.
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] = serde_json::json!(charge - 1)
        });
        assert!(reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "oversized",
            next_cid
        )
        .is_err());
        assert_eq!(std::fs::read(&state_path).unwrap(), original);
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"]["max_cache_bytes"] =
                serde_json::json!(2 * charge - 1)
        });
        assert!(reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "../invalid",
            next_cid
        )
        .is_err());
        assert_eq!(std::fs::read(&state_path).unwrap(), original);

        // The new package fits by itself. Existing unkept bytes cause aggregate
        // pressure: record intent, not a claimed reservation or premature delete.
        let pending = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "pressure-use",
            next_cid,
        )
        .expect("valid pressure Use needs durable capacity-pending ownership");
        assert_eq!(
            serde_json::to_value(&pending.state).unwrap(),
            "capacity_pending"
        );
        assert_eq!(pending.reserved_bytes, 0);
        assert_eq!(pending.completed_bytes, 0);
        assert_eq!(pending.admission_id, pending.operation_id);
        let persisted = std::fs::read(&state_path).unwrap();
        assert_eq!(
            reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "pressure-use",
                next_cid
            )
            .unwrap(),
            pending
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
        let snapshot = Inventory::open(root.path(), false).unwrap().load().unwrap();
        assert_eq!(snapshot.records.len(), 2);
        assert_eq!(
            snapshot
                .records
                .iter()
                .map(|r| r.reserved_bytes)
                .sum::<u64>(),
            charge
        );
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|r| r.operation_id == admitted.operation_id)
                .unwrap()
                .state,
            PreparationState::Admitted
        );
        assert_eq!(std::fs::read(&weights_path).unwrap(), original_weights);
        assert!(
            backend.calls.lock().unwrap().is_empty(),
            "reservation must not dispatch effects"
        );
        for _ in 0..2 {
            let owner = PreparationOwner::default();
            let repeated = owner
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(&context(), &method("use")),
                    "pressure-use",
                    &serde_json::json!({"cid":next_cid}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            assert_eq!(repeated["state"], "capacity_pending");
            assert_eq!(repeated["operation_id"], pending.operation_id);
            assert!(owner.worker.lock().unwrap().is_none());
            assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
        }
        let inventory = Inventory::open(root.path(), false).unwrap();
        let held_worker = inventory.worker_lock().unwrap();
        drop(inventory);
        assert!(PreparationOwner::default()
            .invoke(
                root.path(),
                Some(registry.clone()),
                caller(&context(), &method("use")),
                "pressure-use",
                &serde_json::json!({"cid":next_cid}),
                Arc::new(|| Ok(())),
            )
            .is_err());
        drop(held_worker);
        assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
        assert!(reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "competing-use",
            next_cid,
        )
        .is_err());
        let mut other = context();
        other.principal_id = "person:another-owner".into();
        assert!(status(
            root.path(),
            &caller(&other, &method("status")),
            &pending.operation_id,
        )
        .is_err());
        assert!(cancel(
            root.path(),
            &caller(&other, &method("cancel")),
            &pending.operation_id,
        )
        .is_err());
        for field in ["reserved_bytes", "index_bytes", "completed_bytes"] {
            let mut malformed: serde_json::Value = serde_json::from_slice(&persisted).unwrap();
            malformed["records"][1][field] = serde_json::json!(1);
            assert!(serde_json::from_value::<PreparationInventory>(malformed)
                .unwrap()
                .validate()
                .is_err());
        }
        assert_eq!(std::fs::read(&state_path).unwrap(), persisted);
        let cancelled = cancel(
            root.path(),
            &caller(&context(), &method("cancel")),
            &pending.operation_id,
        )
        .unwrap();
        assert_eq!(cancelled.state, PreparationState::Cancelled);
        assert_eq!(cancelled.reserved_bytes, 0);
        assert_eq!(
            reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "pressure-use",
                next_cid,
            )
            .unwrap(),
            cancelled
        );
        let next = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "after-cancel",
            next_cid,
        )
        .unwrap();
        let expired = manage(
            root.path(),
            &caller(&context(), &method("status")),
            &next.operation_id,
            false,
            next.expires_at,
        )
        .unwrap();
        assert_eq!(expired.state, PreparationState::Expired);
        assert_eq!(expired.reserved_bytes, 0);
        assert_eq!(std::fs::read(&weights_path).unwrap(), original_weights);
        assert!(backend.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn model_preparation_failure_phase_survives_cleanup_with_real_home_revalidation() {
        use crate::api::gateway::{
            issue_home_launch_token_for_auth_grant, require_home_launch_token_for_any_app_context,
        };
        use elastos_runtime::auth::AuthSessionGrantV1;

        for (fault, phase) in [
            (
                "metadata_tamper",
                PreparationFailurePhase::MetadataIntegrity,
            ),
            ("file_unavailable", PreparationFailurePhase::WeightsRead),
            (
                "file_unavailable_undrained",
                PreparationFailurePhase::WeightsRead,
            ),
            ("header", PreparationFailurePhase::WeightsHeader),
            ("authority_revoked", PreparationFailurePhase::Authority),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (payload, files) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            let metadata_bytes: u64 = files
                .iter()
                .filter(|(path, _)| {
                    !["_elastos_object.json", "weights.gguf"].contains(&path.as_str())
                })
                .map(|(_, bytes)| bytes.len() as u64)
                .sum();
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

            let context = context();
            let grant = AuthSessionGrantV1 {
                schema: AuthSessionGrantV1::SCHEMA.into(),
                principal_id: context.principal_id.clone(),
                session_id: context.session_id.clone(),
                proof_binding_id: context.proof_binding_id.clone().unwrap(),
                grant_id: context.grant_id.clone(),
                issued_at: now().unwrap(),
                expires_at: now().unwrap() + 3600,
                apps: vec!["marketplace".into()],
            };
            crate::auth::store_session_grant(root.path(), grant.clone()).unwrap();
            let token =
                issue_home_launch_token_for_auth_grant(root.path(), "marketplace", &grant).unwrap();
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("host", "localhost:61180".parse().unwrap());
            headers.insert("origin", "null".parse().unwrap());
            headers.insert("x-elastos-home-token", token.parse().unwrap());
            let (_, admitted) = require_home_launch_token_for_any_app_context(
                root.path(),
                &headers,
                &["marketplace"],
            )
            .unwrap();
            assert_eq!(admitted, context);
            let path = root.path().to_path_buf();
            let revalidate: Revalidate = Arc::new(move || {
                if fault == "authority_revoked"
                    && Inventory::open(&path, false)
                        .and_then(|inventory| inventory.load())
                        .is_ok_and(|state| {
                            state
                                .records
                                .iter()
                                .any(|r| r.completed_bytes == metadata_bytes)
                        })
                {
                    crate::auth::revoke_session_grant(&path, &grant.session_id, now()?)?;
                }
                let (_, current) = require_home_launch_token_for_any_app_context(
                    &path,
                    &headers,
                    &["marketplace"],
                )?;
                ensure!(current == admitted, "preparation authority changed");
                Ok(())
            });
            let owner = PreparationOwner::default();
            let output = owner
                .invoke(
                    root.path(),
                    Some(registry),
                    caller(&context, &method("use")),
                    "failure-phase",
                    &serde_json::json!({"cid":cid}),
                    revalidate,
                )
                .unwrap();
            join_worker(&owner).await;
            let record =
                load_operation(root.path(), output["operation_id"].as_str().unwrap()).unwrap();
            let undrained = fault == "file_unavailable_undrained";
            assert_eq!(
                record.state,
                if undrained {
                    PreparationState::Uncertain
                } else {
                    PreparationState::Failed
                },
                "{fault}"
            );
            assert_eq!(record.failure_phase, Some(phase), "{fault}");
            assert_eq!(record.completed_bytes, metadata_bytes, "{fault}");
            assert_eq!(
                record.reserved_bytes,
                if undrained {
                    preparation_charge(record.total_bytes).unwrap()
                } else {
                    0
                }
            );
            assert_eq!(
                root.path().join("model-preparation/stage").exists(),
                undrained
            );
            assert_eq!(record.projection()["failure_class"], phase.public_class());
            assert_eq!(
                backend.calls.lock().unwrap().last().unwrap(),
                "runtime_prepare_backend"
            );
            assert!(!backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|op| op == "runtime_hash_staged_directory"));
            let durable = serde_json::to_string(&record).unwrap();
            for private in ["/secret", "credential", "hidden", "provider?"] {
                assert!(!durable.contains(private));
                assert!(!record.projection().to_string().contains(private));
            }
            // Old failed receipts retain unknown cause; reading adds no invented phase.
            let mut old = serde_json::to_value(&record).unwrap();
            old.as_object_mut().unwrap().remove("failure_phase");
            let old: PreparationRecord = serde_json::from_value(old).unwrap();
            assert_eq!(old.projection()["failure_class"], serde_json::Value::Null);
        }
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

        include!("preparation/qwen_process_proof.rs");

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

        fn volume_bytes(dir: &File) -> (u128, u128) {
            let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            assert_eq!(
                unsafe { libc::fstatvfs(dir.as_raw_fd(), stats.as_mut_ptr()) },
                0
            );
            let stats = unsafe { stats.assume_init() };
            (
                u128::from(stats.f_blocks) * u128::from(stats.f_frsize),
                u128::from(stats.f_bavail) * u128::from(stats.f_frsize),
            )
        }

        fn loopback_listener(address: &str) {
            let port = address
                .strip_prefix("/ip4/127.0.0.1/tcp/")
                .and_then(|port| port.parse::<u16>().ok())
                .unwrap();
            assert_ne!(port, 0, "fixture listener must have a bound loopback port");
        }

        async fn require_peer(
            kubo: &Path,
            root: &Path,
            repo: &Path,
            expected: &str,
            deadline: Instant,
        ) {
            loop {
                let peers = run(
                    command(kubo, root, repo, root, &["swarm", "peers"]),
                    root,
                    deadline,
                )
                .await;
                let peers: Vec<_> = peers.lines().collect();
                if peers.is_empty() {
                    assert!(Instant::now() < deadline, "fixture peer readiness deadline");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                assert_eq!(peers.len(), 1, "fixture must have only its exact peer");
                let (address, peer) = peers[0].rsplit_once("/p2p/").unwrap();
                loopback_listener(address);
                assert_eq!(peer, expected);
                return;
            }
        }

        async fn start_daemon(
            kubo: &Path,
            root: &Path,
            repo: &Path,
            cold: bool,
            deadline: Instant,
        ) -> (OwnedChild, File, File, u16) {
            let mut args = vec!["daemon", "--routing=none", "--enable-gc=false"];
            if !cold {
                args.push("--offline");
            }
            let stdout = tempfile::tempfile_in(root).unwrap();
            let stderr = tempfile::tempfile_in(root).unwrap();
            let mut daemon = OwnedChild(
                command(kubo, root, repo, root, &args)
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
            (daemon, stdout, stderr, port)
        }

        #[tokio::test]
        #[ignore = "requires explicit ELASTOS_TEST_KUBO_PATH and ELASTOS_TEST_IPFS_PROVIDER_PATH; parent runs isolated process proof"]
        async fn model_preparation_real_content_native_process_use_admit_and_reuse() {
            preparation_process(false).await;
        }

        #[tokio::test]
        #[ignore = "requires explicit pinned binaries and isolated loopback peers; parent runs cold process proof"]
        async fn model_preparation_real_content_native_process_cold_64mib() {
            preparation_process(true).await;
        }

        async fn preparation_process(cold: bool) {
            preparation_process_with_qwen(cold, None).await;
        }

        async fn preparation_process_with_qwen(cold: bool, qwen: Option<QwenProof>) {
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
            let setup_started = Instant::now();
            // Fixed synthetic sizes only. This whole-buffer fixture is not a
            // large-model publisher or a product-memory measurement.
            let weights_size = if qwen.is_some() {
                QWEN_BYTES as usize
            } else if cold {
                64 * 1024 * 1024 - 65536
            } else {
                8 * 1024 * 1024
            };
            let package_bound = weights_size as u64 + 65536;
            let layout_charge = preparation_charge(package_bound).unwrap()
                + (if qwen.is_some() { 2 } else { 3 }) * package_bound
                + 32 * 1024 * 1024;
            let dir = File::open(&root_path).unwrap();
            let (volume_capacity, initial_free) = volume_bytes(&dir);
            storage::require_space_floor(volume_capacity, initial_free, u128::from(layout_charge))
                .unwrap();
            let data = root_path.join("data");
            let seed = root_path.join("seed");
            let repo = data.join("ipfs-repo");
            let publisher_repo = root_path.join("publisher-repo");
            let seed_repo = if cold { &publisher_repo } else { &repo };
            for path in [&data, &seed, &data.join("bin")] {
                fs::create_dir(path).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            // The native provider resolves the explicit fixture tool locally;
            // only this isolated directory contains the test link.
            std::os::unix::fs::symlink(&kubo, data.join("bin/kubo")).unwrap();
            let deadline =
                Instant::now() + Duration::from_secs(if qwen.is_some() { 3500 } else { 90 });
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
            for fixture_repo in if cold {
                vec![&repo, &publisher_repo]
            } else {
                vec![&repo]
            } {
                run(
                    command(
                        &kubo,
                        &root_path,
                        fixture_repo,
                        &root_path,
                        &["init", "--empty-repo"],
                    ),
                    &root_path,
                    deadline,
                )
                .await;
                if cold {
                    for profile in ["test", "autoconf-off", "announce-off"] {
                        run(
                            command(
                                &kubo,
                                &root_path,
                                fixture_repo,
                                &root_path,
                                &["config", "profile", "apply", profile],
                            ),
                            &root_path,
                            deadline,
                        )
                        .await;
                    }
                }
                let config_path = fixture_repo.join("config");
                let mut config: serde_json::Value =
                    serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
                config["Bootstrap"] = serde_json::json!([]);
                config["Addresses"]["API"] = serde_json::json!("/ip4/127.0.0.1/tcp/0");
                config["Addresses"]["Gateway"] = serde_json::json!("");
                config["Addresses"]["Swarm"] = if cold {
                    serde_json::json!(["/ip4/127.0.0.1/tcp/0"])
                } else {
                    serde_json::json!([])
                };
                config["Routing"]["Type"] = serde_json::json!("none");
                config["Discovery"]["MDNS"]["Enabled"] = serde_json::json!(false);
                config["AutoConf"]["Enabled"] = serde_json::json!(false);
                if cold {
                    config["Swarm"]["DisableNatPortMap"] = serde_json::json!(true);
                    config["Swarm"]["RelayClient"]["Enabled"] = serde_json::json!(false);
                    config["Swarm"]["RelayService"]["Enabled"] = serde_json::json!(false);
                    config["Swarm"]["EnableHolePunching"] = serde_json::json!(false);
                    config["AutoNAT"]["ServiceMode"] = serde_json::json!("disabled");
                    config["Swarm"]["Transports"]["Network"] = serde_json::json!({
                        "TCP":true,"Relay":false,"QUIC":false,"Websocket":false,
                        "WebTransport":false,"WebRTCDirect":false
                    });
                }
                config["Import"] = proof_import_profile();
                fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
            }
            let consumer_baseline = if cold {
                let blocks = run(
                    command(&kubo, &root_path, &repo, &root_path, &["refs", "local"]),
                    &root_path,
                    deadline,
                )
                .await;
                // Pinned Kubo creates this four-byte initialization block even
                // with --empty-repo. It is unrelated to the selected package.
                assert_eq!(
                    blocks,
                    "bafkreiczsscdsbs7ffqz55asqdf3smv6klcw3gofszvwlyarci47bgf354"
                );
                let stat = run(
                    command(
                        &kubo,
                        &root_path,
                        &repo,
                        &root_path,
                        &["block", "stat", "--enc=json", &blocks],
                    ),
                    &root_path,
                    deadline,
                )
                .await;
                let stat: serde_json::Value = serde_json::from_str(&stat).unwrap();
                assert_eq!(stat["Size"], 4);
                Some(blocks)
            } else {
                None
            };
            let (mut payload, files) = if let Some(qwen) = &qwen {
                qwen.package()
            } else {
                let mut weights = vec![0; weights_size];
                let mut random = 0x7b16_984d_3c20_a5e1u64;
                for bytes in weights.chunks_mut(8) {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    let len = bytes.len();
                    bytes.copy_from_slice(&random.to_le_bytes()[..len]);
                }
                weights[..8].copy_from_slice(b"GGUF\x03\0\0\0");
                package_fixture(weights)
            };
            assert!(files.values().map(|bytes| bytes.len() as u64).sum::<u64>() <= package_bound);
            for (path, bytes) in &files {
                fs::write(seed.join(path), bytes).unwrap();
            }
            let mut args = PACKAGE_ADD_ARGS.to_vec();
            args.extend(files.keys().map(String::as_str));
            // Import the borrowed real weights through stdin, never a seed copy,
            // symlink or whole-file JSON/base64 request. Metadata remains small.
            let cid = if let Some(qwen) = &qwen {
                import_streamed_package(
                    &kubo,
                    &root_path,
                    seed_repo,
                    &seed,
                    &args,
                    &qwen.weights,
                    deadline,
                )
                .await
            } else {
                run(
                    command(&kubo, &root_path, seed_repo, &seed, &args),
                    &root_path,
                    deadline,
                )
                .await
            };
            assert!(canonical_cid(&cid, 0x70));
            payload["entries"][0]["cid"] = serde_json::json!(cid);
            write_preparation_catalog(&data, &payload);
            let engine_cleanup = if let Some(qwen) = &qwen {
                Some(qwen.install_engine(&data, &root_path, deadline).await)
            } else {
                None
            };
            if cold {
                change_config(&data, |config| {
                    config["model_catalog"]["local_use"]["max_cache_bytes"] =
                        serde_json::json!(preparation_charge(package_bound).unwrap());
                });
                let baseline = consumer_baseline.as_deref().unwrap();
                assert!(
                    !baseline.lines().any(|block| block == cid),
                    "selected package was already present in consumer baseline"
                );
                assert_eq!(
                    run(
                        command(&kubo, &root_path, &repo, &root_path, &["refs", "local"]),
                        &root_path,
                        deadline
                    )
                    .await,
                    baseline,
                    "consumer block set changed before peer connection"
                );
            }
            assert!(
                !data.join("model-preparation").exists(),
                "Use starts with an empty preparation inventory"
            );
            let backend_before = disk_bytes(&repo);
            let seed_disk = disk_bytes(&seed);
            let (mut daemon, stdout, stderr, port) =
                start_daemon(&kubo, &root_path, &repo, cold, deadline).await;
            let mut publisher = if cold {
                Some(start_daemon(&kubo, &root_path, &publisher_repo, true, deadline).await)
            } else {
                None
            };
            let mut peer_ids = Vec::new();
            if cold {
                let mut listeners = Vec::new();
                for fixture_repo in [&repo, &publisher_repo] {
                    let config: serde_json::Value =
                        serde_json::from_slice(&fs::read(fixture_repo.join("config")).unwrap())
                            .unwrap();
                    peer_ids.push(config["Identity"]["PeerID"].as_str().unwrap().to_owned());
                    let listener = run(
                        command(
                            &kubo,
                            &root_path,
                            fixture_repo,
                            &root_path,
                            &["swarm", "addrs", "listen"],
                        ),
                        &root_path,
                        deadline,
                    )
                    .await;
                    loopback_listener(&listener);
                    listeners.push(listener);
                    assert!(run(
                        command(
                            &kubo,
                            &root_path,
                            fixture_repo,
                            &root_path,
                            &["swarm", "peers"]
                        ),
                        &root_path,
                        deadline
                    )
                    .await
                    .is_empty());
                }
                let address = format!("{}/p2p/{}", listeners[1], peer_ids[1]);
                run(
                    command(
                        &kubo,
                        &root_path,
                        &repo,
                        &root_path,
                        &["swarm", "connect", &address],
                    ),
                    &root_path,
                    deadline,
                )
                .await;
                require_peer(&kubo, &root_path, &repo, &peer_ids[1], deadline).await;
                require_peer(&kubo, &root_path, &publisher_repo, &peer_ids[0], deadline).await;
            }
            let setup_ms = setup_started.elapsed().as_millis();
            let publisher_before = if cold && qwen.is_none() {
                disk_bytes(&publisher_repo)
            } else {
                (0, 0)
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
            let test_volume = dir.try_clone().unwrap();
            let test_publisher_repo = publisher_repo.clone();
            let real_qwen = qwen.is_some();
            let qwen_provider = qwen.as_ref().map(|q| q.provider.clone());
            let model_children = Arc::new(Mutex::new(Vec::new()));
            let test_model_children = model_children.clone();
            let mut logs = vec![stdout.try_clone().unwrap(), stderr.try_clone().unwrap()];
            if let Some((_, out, err, _)) = &publisher {
                logs.extend([out.try_clone().unwrap(), err.try_clone().unwrap()]);
            }
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
                let mut publisher_peak = publisher_before;
                let mut combined_allocated_peak = 0;
                let mut minimum_free = volume_bytes(&test_volume).1;
                let mut samples = 0;
                let mut last_sample = Instant::now();
                let mut max_sample_gap_ms = 0;
                let mut rss_peak_kib = 0;
                while !test_owner
                    .worker
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_finished()
                {
                    // A Qwen backend has tens of thousands of blocks. Sample
                    // volume space and owned-process RSS, not recursive trees.
                    let stage = if real_qwen {
                        (0, 0)
                    } else {
                        disk_bytes(&test_data.join("model-preparation/stage"))
                    };
                    let backend = if real_qwen {
                        (0, 0)
                    } else {
                        disk_bytes(&test_repo)
                    };
                    let publisher = if cold && !real_qwen {
                        disk_bytes(&test_publisher_repo)
                    } else {
                        (0, 0)
                    };
                    staged_peak = (staged_peak.0.max(stage.0), staged_peak.1.max(stage.1));
                    backend_peak = (backend_peak.0.max(backend.0), backend_peak.1.max(backend.1));
                    publisher_peak = (
                        publisher_peak.0.max(publisher.0),
                        publisher_peak.1.max(publisher.1),
                    );
                    combined_allocated_peak = combined_allocated_peak
                        .max(stage.1 + backend.1 + publisher.1 + seed_disk.1);
                    minimum_free = minimum_free.min(volume_bytes(&test_volume).1);
                    storage::require_space_floor(volume_capacity, minimum_free, 0).unwrap();
                    assert!(
                        logs.iter()
                            .all(|log| log.metadata().unwrap().len() <= 65536),
                        "daemon log bound"
                    );
                    max_sample_gap_ms = max_sample_gap_ms.max(last_sample.elapsed().as_millis());
                    last_sample = Instant::now();
                    samples += 1;
                    if real_qwen {
                        rss_peak_kib = rss_peak_kib.max(proof_rss_kib());
                    }
                    tokio::time::sleep(Duration::from_millis(if real_qwen { 1000 } else { 10 }))
                        .await;
                }
                join_worker(&test_owner).await;
                let elapsed_ms = started.elapsed().as_millis();
                let record = load_operation(&test_data, &id).unwrap();
                assert_eq!(record.state, PreparationState::Admitted);
                assert_eq!(record.completed_bytes, record.total_bytes);
                assert_eq!(
                    record.reserved_bytes,
                    preparation_charge(record.total_bytes).unwrap()
                );
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
                if real_qwen {
                    assert_qwen_file(&admitted.path.join("weights.gguf"));
                }
                let backend_after = if real_qwen {
                    (0, 0)
                } else {
                    disk_bytes(&test_repo)
                };
                let publisher_after = if cold && !real_qwen {
                    disk_bytes(&test_publisher_repo)
                } else {
                    (0, 0)
                };
                backend_peak = (
                    backend_peak.0.max(backend_after.0),
                    backend_peak.1.max(backend_after.1),
                );
                publisher_peak = (
                    publisher_peak.0.max(publisher_after.0),
                    publisher_peak.1.max(publisher_after.1),
                );
                if cold && !real_qwen {
                    assert!(
                        backend_after.0 >= backend_before.0 + weights_size as u64,
                        "cold transfer must grow consumer backend logical bytes"
                    );
                    assert!(
                        backend_after.1 > backend_before.1,
                        "cold transfer must allocate consumer backend blocks"
                    );
                }
                combined_allocated_peak = combined_allocated_peak
                    .max(admitted_disk.1 + backend_after.1 + publisher_after.1 + seed_disk.1);
                minimum_free = minimum_free.min(volume_bytes(&test_volume).1);
                storage::require_space_floor(volume_capacity, minimum_free, 0).unwrap();
                let requests = test_backend.calls.lock().unwrap().clone();
                let reads = requests.iter().filter(|op| *op == "cat").count();
                let expected_reads =
                    1 + if real_qwen {
                        QWEN_BYTES.div_ceil(65536) as usize
                    } else {
                        0
                    } + test_backend
                        .files
                        .iter()
                        .filter(|(path, _)| *path != "_elastos_object.json")
                        .map(|(_, bytes)| bytes.len().div_ceil(65536))
                        .sum::<usize>();
                assert_eq!(reads, expected_reads);
                // The real engine enables activation composition after admission;
                // both independently verify the exact stored package CID.
                let admission_and_activation_hashes = if real_qwen { 2 } else { 1 };
                assert_eq!(
                    requests
                        .iter()
                        .filter(|op| *op == "runtime_hash_staged_directory")
                        .count(),
                    admission_and_activation_hashes
                );
                let reopened = PreparationOwner::default();
                if real_qwen {
                    retention_intent(&test_data, &context(), &cid, true).unwrap();
                }
                let reuse_started = Instant::now();
                let alias = reopened
                    .invoke(
                        &test_data,
                        Some(test_registry.clone()),
                        caller(&context(), &method("use")),
                        "process-reuse",
                        &input,
                        Arc::new(|| Ok(())),
                    )
                    .unwrap();
                if real_qwen {
                    // Reuse rehashes the full closure twice; use the existing
                    // whole-proof deadline, not the tiny-fixture five-second join.
                    let worker = reopened.worker.lock().unwrap().take().unwrap();
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), worker)
                        .await
                        .unwrap()
                        .unwrap();
                } else {
                    join_worker(&reopened).await;
                }
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
                assert!(reused.projection().get("inference_ready").is_none());
                assert_eq!(
                    test_backend
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|op| *op == "runtime_hash_staged_directory")
                        .count(),
                    2 * admission_and_activation_hashes
                );
                let reuse_ms = reuse_started.elapsed().as_millis();
                let inference = if let Some(binary) = qwen_provider {
                    qwen_reply_and_restart(
                        &test_data,
                        &test_registry,
                        &binary,
                        &cid,
                        deadline,
                        &test_model_children,
                    )
                    .await
                } else {
                    serde_json::Value::Null
                };
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
                assert_eq!(
                    test_backend
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|op| *op == "runtime_hash_staged_directory")
                        .count(),
                    2 * admission_and_activation_hashes + if real_qwen { 2 } else { 0 }
                );
                assert!(
                    logs.iter()
                        .all(|log| log.metadata().unwrap().len() <= 65536),
                    "daemon log bound"
                );
                serde_json::json!({"cold":cold,"setup_ms":setup_ms,"elapsed_preparation_ms":elapsed_ms,
                    "reuse_ms":reuse_ms,"content_reads":reads,
                    "layout_preflight_bytes":layout_charge,"minimum_sampled_free_bytes":minimum_free,
                    "sample_interval_ms":if real_qwen {1000} else {10},"samples":samples,"max_sample_gap_ms":max_sample_gap_ms,
                    "test_process_sampled_rss_peak_kib":rss_peak_kib,"inference":inference,
                    "publisher_before":publisher_before,"publisher_sampled_peak":publisher_peak,
                    "publisher_after":publisher_after,
                    "combined_sampled_allocated_peak":combined_allocated_peak,
                    "provider_requests":test_backend.calls.lock().unwrap().len(),"total_bytes":record.total_bytes,
                    "index_bytes":record.index_bytes,"reserved_bytes":record.reserved_bytes,
                    "seed_disk":seed_disk,"backend_before":backend_before,"backend_sampled_peak":backend_peak,
                    "staging_sampled_peak":staged_peak,"admitted_disk":admitted_disk,"backend_after":backend_after,
                    "reuse_content_reads":0,"inference_executed":real_qwen})
            });
            let outcome =
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut work).await;
            owner.stopping.store(true, Ordering::Release);
            if outcome.is_err() {
                work.abort();
                let _ = work.await;
            }
            let models = std::mem::take(&mut *model_children.lock().unwrap());
            let mut cleanup_errors = Vec::new();
            for model in models {
                if let Err(error) = model.shutdown().await {
                    cleanup_errors.push(format!("model provider shutdown: {error}"));
                }
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
            // Perform peer assertions in a task so an assertion still reaches
            // the exact-child cleanup below.
            let peer_check = if cold {
                let kubo = kubo.clone();
                let root = root_path.clone();
                let repo = repo.clone();
                let publisher_repo = publisher_repo.clone();
                Some(
                    tokio::spawn(async move {
                        require_peer(&kubo, &root, &repo, &peer_ids[1], deadline).await;
                        require_peer(&kubo, &root, &publisher_repo, &peer_ids[0], deadline).await;
                    })
                    .await,
                )
            } else {
                None
            };
            for result in [daemon.0.kill(), daemon.0.wait().map(|_| ())] {
                if let Err(error) = result {
                    cleanup_errors.push(format!("consumer cleanup: {error}"));
                }
            }
            drop(daemon);
            if let Some((child, _, _, _)) = &mut publisher {
                for result in [child.0.kill(), child.0.wait().map(|_| ())] {
                    if let Err(error) = result {
                        cleanup_errors.push(format!("publisher cleanup: {error}"));
                    }
                }
            }
            drop(publisher);
            drop(dir);
            drop(stdout);
            drop(stderr);
            drop(engine_cleanup);
            let root_cleanup = root.close();
            if let Err(error) = shutdown {
                cleanup_errors.push(format!("content provider shutdown: {error}"));
            }
            if let Err(error) = root_cleanup {
                cleanup_errors.push(format!("fixture root cleanup: {error}"));
            }
            assert!(cleanup_errors.is_empty(), "{cleanup_errors:?}");
            assert!(!root_path.exists());
            if let Some(peer_check) = peer_check {
                peer_check.unwrap();
            }
            let mut receipt = outcome
                .expect("production preparation deadline")
                .expect("production preparation task");
            receipt["fixture_cleanup"] = serde_json::json!(true);
            receipt["native_shutdown_reap"] = serde_json::json!(true);
            receipt["kubo_reaped"] = serde_json::json!(true);
            receipt["kubo_children_reaped"] = serde_json::json!(if cold { 2 } else { 1 });
            receipt["consumer_package_absent_before_connect"] = serde_json::json!(cold);
            receipt["consumer_init_block"] = serde_json::json!(consumer_baseline);
            if qwen.is_some() {
                // Volume samples replace recursive large-backend scans. Do not
                // represent the skipped measurements as zero allocations.
                for key in [
                    "publisher_before",
                    "publisher_sampled_peak",
                    "publisher_after",
                    "combined_sampled_allocated_peak",
                    "backend_before",
                    "backend_sampled_peak",
                    "staging_sampled_peak",
                    "backend_after",
                ] {
                    receipt[key] = serde_json::Value::Null;
                }
                receipt["rss_scope"] = serde_json::json!(
                    "sampled test-process high-water RSS only; full process-tree peak remains open"
                );
                receipt["source_weights_sha256"] = serde_json::json!(QWEN_SHA);
                receipt["publisher_authority"] = serde_json::json!(
                    "isolated test signing key; local operator attestation, not upstream signature"
                );
            }
            receipt["proof_limit"] = serde_json::json!(if qwen.is_some() {
                "Exact Qwen, isolated publisher-attested catalog, cold loopback Content admission and native model reply/restart. Volume/RSS are samples, not continuous peaks; GUI, installed acceptance and eviction remain open."
            } else if cold {
                "64 MiB-capped signed synthetic cold loopback delivery through Content/native Use/admission/reuse. Allocations are sampled peaks; whole-buffer fixture memory is harness-only. Public-network, exact Qwen and inference remain open."
            } else {
                "8 MiB signed synthetic seeded offline Kubo proof through Content/native Use/admission/reuse. Allocations are sampled peaks; whole-buffer fixture memory is harness-only. Cold-network, exact Qwen and inference remain open."
            });
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
        let observed_backend = backend.clone();
        let guard: Revalidate = Arc::new(move || {
            ensure!(
                !observed_backend
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|op| op == "runtime_hash_staged_directory"),
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

    fn retention_intent(
        root: &Path,
        principal: &HomeLaunchTokenContext,
        cid: &str,
        keep: bool,
    ) -> anyhow::Result<serde_json::Value> {
        PreparationOwner::default().invoke(
            root,
            None,
            caller(principal, &method("retention")),
            "retention-choice",
            &serde_json::json!({"cid":cid,"keep":keep}),
            Arc::new(|| Ok(())),
        )
    }

    async fn finish_retention_fixture(
        root: &Path,
        record: &PreparationRecord,
        registry: Arc<elastos_runtime::provider::ProviderRegistry>,
    ) {
        let owner = PreparationOwner::default();
        owner
            .invoke(
                root,
                Some(registry),
                caller(&context(), &method("status")),
                "settle",
                &serde_json::json!({"operation_id":record.operation_id}),
                Arc::new(|| Ok(())),
            )
            .unwrap();
        join_worker(&owner).await;
        assert_eq!(
            load_operation(root, &record.operation_id).unwrap().state,
            PreparationState::Admitted
        );
    }

    #[tokio::test]
    async fn model_retention_pending_replay_restart_and_admission_preserve_intent() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        let calls = backend.calls.lock().unwrap().clone();
        let path = root.path().join("model-preparation/state.json");
        let mut foreign = context();
        foreign.principal_id = "person:other-pending-retention".into();
        for keep in [true, false] {
            let before = std::fs::read(&path).unwrap();
            assert!(retention_intent(root.path(), &foreign, &record.package_cid, keep).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        for keep in [true, true, false, false, true] {
            let reply =
                retention_intent(root.path(), &context(), &record.package_cid, keep).unwrap();
            assert_eq!(
                reply,
                serde_json::json!({"cid":record.package_cid,"kept":keep,"admitted":false})
            );
            let before = std::fs::read(&path).unwrap();
            // A fresh owner reads the same durable intent without starting a worker.
            assert_eq!(
                retention_intent(root.path(), &context(), &record.package_cid, keep).unwrap(),
                reply
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(
                load_operation(root.path(), &record.operation_id).unwrap(),
                record
            );
            assert_eq!(*backend.calls.lock().unwrap(), calls);
        }
        let projection =
            model_runtime_projection(root.path(), None, &context(), &record.package_cid, None)
                .await;
        assert_eq!(projection["kept"], true);
        assert_eq!(projection["admitted"], false);
        assert_eq!(projection["dispatch_ready"], false);
        assert!(projection["offer_id"].is_null());
        settle_failure(root.path(), &record.operation_id, false).unwrap();
        assert_eq!(
            load_operation(root.path(), &record.operation_id)
                .unwrap()
                .state,
            PreparationState::Uncertain
        );
        assert_eq!(
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap()
                ["admitted"],
            false
        );
        assert_eq!(*backend.calls.lock().unwrap(), calls);
        finish_retention_fixture(root.path(), &record, registry).await;
        assert_eq!(
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap(),
            serde_json::json!({"cid":record.package_cid,"kept":true,"admitted":true})
        );
    }

    #[test]
    fn model_retention_pending_cancel_expire_and_fresh_retry_clear_intent() {
        for terminal in ["cancel", "expire", "capacity-cancel", "capacity-expire"] {
            let root = tempfile::tempdir().unwrap();
            let (payload, _) = package_fixture(b"GGUF\x03\0\0\0fixture".to_vec());
            write_preparation_catalog(root.path(), &payload);
            let cid = payload["entries"][0]["cid"].as_str().unwrap();
            let record = reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "original",
                cid,
            )
            .unwrap();
            if terminal.starts_with("capacity-") {
                update_operation(root.path(), &record.operation_id, |pending| {
                    pending.state = PreparationState::CapacityPending;
                    pending.reserved_bytes = 0;
                })
                .unwrap();
            }
            retention_intent(root.path(), &context(), cid, true).unwrap();
            let cancelling = terminal.ends_with("cancel");
            let result = manage(
                root.path(),
                &caller(
                    &context(),
                    &method(if cancelling { "cancel" } else { "status" }),
                ),
                &record.operation_id,
                cancelling,
                if cancelling {
                    record.created_at
                } else {
                    record.expires_at
                },
            )
            .unwrap();
            assert_eq!(
                result.state,
                if cancelling {
                    PreparationState::Cancelled
                } else {
                    PreparationState::Expired
                }
            );
            assert_eq!(result.reserved_bytes, 0);
            let inventory = Inventory::open(root.path(), false).unwrap();
            assert!(!inventory.load().unwrap().kept(&context().principal_id, cid));
            drop(inventory);
            let next = reserve(
                root.path(),
                &caller(&context(), &method("use")),
                "fresh-retry",
                cid,
            )
            .unwrap();
            assert_ne!(next.operation_id, record.operation_id);
            let inventory = Inventory::open(root.path(), false).unwrap();
            assert!(!inventory.load().unwrap().kept(&context().principal_id, cid));
        }
    }

    #[tokio::test]
    async fn model_retention_active_cancel_waits_for_drain_and_failure_clears_intent() {
        for cancelled in [false, true] {
            let (root, record, backend, _) = staged_fixture(now().unwrap(), false).await;
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
            let calls = backend.calls.lock().unwrap().clone();
            if cancelled {
                let pending = cancel(
                    root.path(),
                    &caller(&context(), &method("cancel")),
                    &record.operation_id,
                )
                .unwrap();
                assert!(pending.cancel_requested);
            }
            settle_failure(root.path(), &record.operation_id, false).unwrap();
            {
                let inventory = Inventory::open(root.path(), false).unwrap();
                assert!(inventory
                    .load()
                    .unwrap()
                    .kept(&context().principal_id, &record.package_cid));
            }
            settle_failure(root.path(), &record.operation_id, true).unwrap();
            let settled = load_operation(root.path(), &record.operation_id).unwrap();
            assert_eq!(
                settled.state,
                if cancelled {
                    PreparationState::Cancelled
                } else {
                    PreparationState::Failed
                }
            );
            assert_eq!(settled.reserved_bytes, 0);
            let inventory = Inventory::open(root.path(), false).unwrap();
            assert!(!inventory
                .load()
                .unwrap()
                .kept(&context().principal_id, &record.package_cid));
            assert_eq!(*backend.calls.lock().unwrap(), calls);
        }
    }

    #[tokio::test]
    async fn model_retention_cancelled_shared_preparation_preserves_admitted_claim() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        finish_retention_fixture(root.path(), &record, registry).await;
        retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
        let calls = backend.calls.lock().unwrap().clone();
        let mut foreign = context();
        foreign.principal_id = "person:shared-pending-retention".into();
        for principal in [context(), foreign] {
            let shared = reserve(
                root.path(),
                &caller(&principal, &method("use")),
                "pending-alias",
                &record.package_cid,
            )
            .unwrap();
            assert_eq!(shared.admission_id, record.operation_id);
            let reply =
                retention_intent(root.path(), &principal, &record.package_cid, true).unwrap();
            assert_eq!(
                reply["admitted"],
                principal.principal_id == context().principal_id
            );
            cancel(
                root.path(),
                &caller(&principal, &method("cancel")),
                &shared.operation_id,
            )
            .unwrap();
            let inventory = Inventory::open(root.path(), false).unwrap();
            let state = inventory.load().unwrap();
            assert!(state.kept(&context().principal_id, &record.package_cid));
            assert_eq!(
                state.kept(&principal.principal_id, &record.package_cid),
                principal.principal_id == context().principal_id
            );
            assert_eq!(state.retention_claims.len(), 1);
            assert!(inventory.admitted(&record.operation_id).is_ok());
        }
        assert_eq!(*backend.calls.lock().unwrap(), calls);
    }

    #[tokio::test]
    async fn model_retention_desired_state_replay_restart_preserves_admission_and_bytes() {
        use std::os::unix::fs::MetadataExt as _;
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        finish_retention_fixture(root.path(), &record, registry).await;
        let before = Inventory::open(root.path(), false)
            .unwrap()
            .load()
            .unwrap()
            .records;
        let artifact = Inventory::open(root.path(), false)
            .unwrap()
            .admitted(&record.admission_id)
            .unwrap();
        let inode = std::fs::metadata(&artifact.path).unwrap().ino();
        let calls = backend.calls.lock().unwrap().clone();
        for keep in [true, true, false, false, true] {
            let reply =
                retention_intent(root.path(), &context(), &record.package_cid, keep).unwrap();
            assert_eq!(
                reply,
                serde_json::json!({"cid":record.package_cid,"kept":keep,
                "admitted":true})
            );
            let saved = std::fs::read(root.path().join("model-preparation/state.json")).unwrap();
            assert_eq!(
                retention_intent(root.path(), &context(), &record.package_cid, keep).unwrap(),
                reply
            );
            assert_eq!(
                std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
                saved
            );
            let status = PreparationOwner::default()
                .invoke(
                    root.path(),
                    None,
                    caller(&context(), &method("status")),
                    "read-after-restart",
                    &serde_json::json!({"operation_id":record.operation_id}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            assert_eq!(status["kept"], keep);
            assert!(status.get("inference_ready").is_none());
            assert_eq!(
                Inventory::open(root.path(), false)
                    .unwrap()
                    .load()
                    .unwrap()
                    .records,
                before
            );
            assert_eq!(std::fs::metadata(&artifact.path).unwrap().ino(), inode);
            for (path, bytes) in &backend.files {
                assert_eq!(std::fs::read(artifact.path.join(path)).unwrap(), *bytes);
            }
        }
        // Retention release is local management, independent of model-use permission.
        change_config(root.path(), |config| {
            config["model_catalog"]["local_use"] = serde_json::Value::Null
        });
        assert_eq!(
            retention_intent(root.path(), &context(), &record.package_cid, false).unwrap()["kept"],
            false
        );
        assert_eq!(*backend.calls.lock().unwrap(), calls);
    }

    #[tokio::test]
    async fn model_retention_aliases_share_only_the_current_principal_claim() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        finish_retention_fixture(root.path(), &record, registry.clone()).await;
        let first = context();
        let mut second = context();
        second.principal_id = "person:second-retention".into();
        assert!(retention_intent(root.path(), &second, &record.package_cid, true).is_err());
        assert!(retention_intent(root.path(), &second, &record.package_cid, false).is_err());
        retention_intent(root.path(), &first, &record.package_cid, true).unwrap();
        let mut aliases = Vec::new();
        for (principal, request) in [
            (&first, "same-principal-alias"),
            (&second, "second-principal-alias"),
        ] {
            let owner = PreparationOwner::default();
            let reply = owner
                .invoke(
                    root.path(),
                    Some(registry.clone()),
                    caller(principal, &method("use")),
                    request,
                    &serde_json::json!({"cid":record.package_cid}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            join_worker(&owner).await;
            assert_eq!(reply["kept"], principal.principal_id == first.principal_id);
            aliases.push(reply["operation_id"].as_str().unwrap().to_owned());
        }
        let before = Inventory::open(root.path(), false)
            .unwrap()
            .load()
            .unwrap()
            .records;
        let calls = backend.calls.lock().unwrap().clone();
        for principal in [&first, &second] {
            assert_eq!(
                retention_intent(root.path(), principal, &record.package_cid, true).unwrap()
                    ["kept"],
                true
            );
        }
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["retention_claims"].as_array().unwrap().len(), 2);
        for (principal, id) in [
            (&first, &record.operation_id),
            (&first, &aliases[0]),
            (&second, &aliases[1]),
        ] {
            let reply = PreparationOwner::default()
                .invoke(
                    root.path(),
                    None,
                    caller(principal, &method("status")),
                    "alias-status",
                    &serde_json::json!({"operation_id":id}),
                    Arc::new(|| Ok(())),
                )
                .unwrap();
            assert_eq!(reply["kept"], true);
        }
        retention_intent(root.path(), &first, &record.package_cid, false).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("model-preparation/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["retention_claims"],
            serde_json::json!([{"principal":second.principal_id,"cid":record.package_cid}])
        );
        assert_eq!(
            Inventory::open(root.path(), false)
                .unwrap()
                .load()
                .unwrap()
                .records,
            before
        );
        assert_eq!(*backend.calls.lock().unwrap(), calls);
    }

    #[tokio::test]
    async fn model_retention_invalid_input_and_authority_preserve_inventory() {
        let (root, record, _, registry) = staged_fixture(now().unwrap(), true).await;
        assert_eq!(
            retention_intent(root.path(), &context(), &record.package_cid, true).unwrap()
                ["admitted"],
            false
        );
        finish_retention_fixture(root.path(), &record, registry).await;
        retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
        let path = root.path().join("model-preparation/state.json");
        let saved = std::fs::read(&path).unwrap();
        for input in [
            serde_json::json!({}),
            serde_json::json!({"cid":record.package_cid}),
            serde_json::json!({"cid":record.package_cid,"keep":"false"}),
            serde_json::json!({"cid":record.package_cid,"keep":false,"principal_id":"person:other"}),
            serde_json::json!({"cid":"x".repeat(129),"keep":false}),
            serde_json::json!({"cid":record.package_cid.to_uppercase(),"keep":false}),
        ] {
            assert!(PreparationOwner::default()
                .invoke(
                    root.path(),
                    None,
                    caller(&context(), &method("retention")),
                    "bad",
                    &input,
                    Arc::new(|| Ok(()))
                )
                .is_err());
            assert_eq!(std::fs::read(&path).unwrap(), saved);
        }
        let input = serde_json::json!({"cid":record.package_cid,"keep":false});
        for mismatch in ["risk", "approval", "resource", "id", "operation"] {
            let mut denied = method("retention");
            match mismatch {
                "risk" => denied.risk = AffordanceRisk::Read,
                "approval" => denied.approval = AffordanceApprovalMode::User,
                "resource" => denied.resource = Some("elastos://model/*".into()),
                "id" => denied.id = "content.use".into(),
                _ => denied.operation = Some("keep".into()),
            }
            assert!(PreparationOwner::default()
                .invoke(
                    root.path(),
                    None,
                    caller(&context(), &denied),
                    "bad-policy",
                    &input,
                    Arc::new(|| Ok(()))
                )
                .is_err());
        }
        let mut invalid = context();
        invalid.principal_id.clear();
        assert!(retention_intent(root.path(), &invalid, &record.package_cid, false).is_err());
        assert!(PreparationOwner::default()
            .invoke(
                root.path(),
                None,
                caller(&context(), &method("retention")),
                "revoked",
                &input,
                Arc::new(|| anyhow::bail!("launch revoked"))
            )
            .is_err());
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = checks.clone();
        assert!(PreparationOwner::default()
            .invoke(
                root.path(),
                None,
                caller(&context(), &method("retention")),
                "revoked-before-save",
                &input,
                Arc::new(move || {
                    anyhow::ensure!(
                        checks.fetch_add(1, Ordering::SeqCst) == 0,
                        "launch revoked before save"
                    );
                    Ok(())
                })
            )
            .is_err());
        assert_eq!(observed.load(Ordering::SeqCst), 2);
        assert_eq!(std::fs::read(path).unwrap(), saved);
    }

    #[tokio::test]
    async fn model_retention_stored_claims_fail_closed_without_matching_admission() {
        let (root, record, backend, registry) = staged_fixture(now().unwrap(), true).await;
        finish_retention_fixture(root.path(), &record, registry).await;
        retention_intent(root.path(), &context(), &record.package_cid, true).unwrap();
        let path = root.path().join("model-preparation/state.json");
        let valid: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let claim = valid["retention_claims"][0].clone();
        let foreign_cid = cid::Cid::new_v1(
            0x70,
            cid::multihash::Multihash::<64>::wrap(0x12, &[7; 32]).unwrap(),
        )
        .to_string();
        let corruptions = vec![
            serde_json::json!([claim, claim]),
            serde_json::json!(vec![claim.clone(); MAX_RECORDS + 1]),
            serde_json::json!([{"principal":"person:foreign","cid":record.package_cid}]),
            serde_json::json!([{"principal":context().principal_id,"cid":foreign_cid}]),
            serde_json::json!([{"principal":context().principal_id,"cid":record.package_cid,"keep":true}]),
            serde_json::Value::Null,
        ];
        let calls = backend.calls.lock().unwrap().clone();
        for claims in corruptions {
            let mut changed = valid.clone();
            changed["retention_claims"] = claims;
            assert!(
                serde_json::from_value::<PreparationInventory>(changed.clone())
                    .map(|state| state.validate().is_err())
                    .unwrap_or(true)
            );
            let bytes = serde_json::to_vec(&changed).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(retention_intent(root.path(), &context(), &record.package_cid, false).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let mut missing = valid;
        missing.as_object_mut().unwrap().remove("retention_claims");
        assert!(
            serde_json::from_value::<PreparationInventory>(missing).is_err(),
            "old shape needs no compatibility decoder"
        );
        assert_eq!(*backend.calls.lock().unwrap(), calls);
    }

    #[test]
    fn model_retention_first_party_manifests_declare_local_management() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for name in ["marketplace", "system", "assistant"] {
            let value: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("capsules").join(name).join("capsule.json")).unwrap(),
            )
            .unwrap();
            let methods: Vec<_> = value["interfaces"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|interface| interface["methods"].as_array().unwrap())
                .filter(|method| method["id"] == "content.retention")
                .collect();
            assert_eq!(methods.len(), 1, "{name} must declare one retention intent");
            let descriptor: CapsuleAffordanceDescriptor =
                serde_json::from_value(methods[0].clone()).unwrap();
            authorize(&caller(&context(), &descriptor), "retention").unwrap();
        }
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
        assert!(replay.get("inference_ready").is_none());
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
    fn model_preparation_progress_waits_for_a_short_status_snapshot() {
        let (root, cid) = fixture();
        let record = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "status-contention",
            &cid,
        )
        .unwrap();
        let snapshot = Inventory::open(root.path(), false).unwrap();
        assert_eq!(snapshot.load().unwrap().records, vec![record.clone()]);
        let reader = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(snapshot);
        });
        let update = update_operation(root.path(), &record.operation_id, |record| {
            record.activation_pending = true;
        });
        reader.join().unwrap();
        update.expect("a short status snapshot must not fail preparation progress");
        let mut expected = record.clone();
        expected.activation_pending = true;
        assert_eq!(
            load_operation(root.path(), &record.operation_id).unwrap(),
            expected
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
    fn model_preparation_capacity_cold_growth_stays_within_initial_charge() {
        let payload = 64 * 1024 * 1024;
        let capacity = 1_u128 << 30;
        let floor = capacity.div_ceil(10);
        let margin = 1024 * 1024;
        let charge = preparation_charge(payload).unwrap();
        let initial_free = floor + u128::from(charge) + margin;
        assert!(storage::require_space_floor(capacity, initial_free, charge.into()).is_ok());
        for completed in [0, payload / 4, payload / 2, payload] {
            let index = if completed == 0 { 0 } else { 717 };
            let delivered = u128::from(completed + index);
            let (stage, backend) = remaining_capacity_charges(payload, completed, index).unwrap();
            // Model one cold backend payload copy plus the staged copy. These
            // are deterministic logical-growth facts, not Kubo allocation proof.
            let free = initial_free - 2 * delivered;
            assert!(
                storage::require_space_floor(
                    capacity,
                    free,
                    u128::from(stage) + u128::from(backend)
                )
                .is_ok(),
                "double charged stored backend bytes at completed={completed}"
            );
        }
        assert_eq!(
            preparation_charge(payload).unwrap(),
            charge,
            "quota remains the full charge"
        );
    }

    #[test]
    fn model_preparation_capacity_separate_cold_volume_and_warm_backend() {
        let payload = 64 * 1024 * 1024;
        let capacity = 1_u128 << 30;
        let floor = capacity.div_ceil(10);
        let margin = 1024 * 1024;
        let stage_budget = staging_charge(payload).unwrap();
        let backend_budget = preparation_charge(payload).unwrap() - stage_budget;
        let completed = payload / 4;
        let index = 717;
        let delivered = u128::from(completed + index);
        let (stage, backend) = remaining_capacity_charges(payload, completed, index).unwrap();
        let stage_free = floor + u128::from(stage_budget) + margin - delivered;
        assert!(storage::require_space_floor(capacity, stage_free, stage.into()).is_ok());
        let backend_free = floor + u128::from(backend_budget) + margin - delivered;
        assert!(storage::require_space_floor(capacity, backend_free, backend.into()).is_ok());
        // A warm backend stores nothing new. Logical credit stays conservative.
        let warm_free =
            floor + u128::from(preparation_charge(payload).unwrap()) + margin - delivered;
        assert!(storage::require_space_floor(
            capacity,
            warm_free,
            u128::from(stage) + u128::from(backend)
        )
        .is_ok());
        for required in [
            u128::from(stage),
            u128::from(backend),
            u128::from(stage) + u128::from(backend),
        ] {
            assert!(storage::require_space_floor(capacity, floor + required, required).is_ok());
            assert!(
                storage::require_space_floor(capacity, floor + required - 1, required).is_err()
            );
        }
    }

    #[test]
    fn model_preparation_capacity_invalid_progress_fails_without_overflow() {
        for (total, completed, index) in [
            (u64::MAX, 0, 0),
            (64, u64::MAX, 1),
            (64, 65, 0),
            (64, 0, 65537),
        ] {
            let result =
                std::panic::catch_unwind(|| remaining_capacity_charges(total, completed, index));
            assert!(result.is_ok(), "invalid accounting panicked");
            assert!(result.unwrap().is_err(), "invalid progress was accepted");
        }
    }

    #[tokio::test]
    async fn model_preparation_capacity_provider_receives_only_remaining_growth() {
        let (root, mut record, backend, registry) = staged_fixture(now().unwrap(), false).await;
        record.completed_bytes = record.total_bytes / 4;
        let delivered = record.completed_bytes + record.index_bytes;
        let full_backend = preparation_charge(record.total_bytes).unwrap()
            - staging_charge(record.total_bytes).unwrap();
        require_capacity(root.path(), &registry, &record)
            .await
            .unwrap();
        assert_eq!(
            *backend.capacity_requests.lock().unwrap(),
            vec![full_backend - delivered]
        );
        assert_eq!(
            load_operation(root.path(), &record.operation_id)
                .unwrap()
                .reserved_bytes,
            record.reserved_bytes
        );
    }

    async fn capacity_window_fixture(
        shared_volume: bool,
        fail_capacity_at: Option<usize>,
    ) -> (
        tempfile::TempDir,
        PreparationRecord,
        Arc<PreparationBackend>,
        Arc<elastos_runtime::provider::ProviderRegistry>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let mut weights = vec![9; 1024 * 1024 + 17];
        weights[..8].copy_from_slice(b"GGUF\x03\0\0\0");
        let (payload, files) = package_fixture(weights);
        write_preparation_catalog(root.path(), &payload);
        let cid = payload["entries"][0]["cid"].as_str().unwrap();
        let record = reserve(
            root.path(),
            &caller(&context(), &method("use")),
            "window",
            cid,
        )
        .unwrap();
        update_operation(root.path(), &record.operation_id, |record| {
            record.state = PreparationState::Preparing;
        })
        .unwrap();
        let record = load_operation(root.path(), &record.operation_id).unwrap();
        let volume = Inventory::open(root.path(), false)
            .unwrap()
            .volume()
            .unwrap();
        let mut backend = PreparationBackend::new(files, cid.to_owned());
        backend.capacity_volume = if shared_volume {
            volume
        } else {
            volume.checked_add(1).unwrap()
        };
        backend.fail_capacity_at = fail_capacity_at;
        let backend = Arc::new(backend);
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        registry
            .register_sub_provider("ipfs", backend.clone())
            .await
            .unwrap();
        register_content(&registry, root.path()).await;
        (root, record, backend, registry)
    }

    #[tokio::test]
    async fn model_preparation_capacity_windows_bound_queries_and_final_partial_window() {
        for shared in [true, false] {
            let (root, record, backend, registry) = capacity_window_fixture(shared, None).await;
            let revalidate: Revalidate = Arc::new(|| Ok(()));
            prepare(
                root.path(),
                &registry,
                &record.operation_id,
                &AtomicBool::new(false),
                &revalidate,
            )
            .await
            .unwrap();
            let reads = backend.read_sizes.lock().unwrap();
            let mut delivered = 0u64;
            let mut window_bytes = 0u64;
            let full_backend = preparation_charge(record.total_bytes).unwrap()
                - staging_charge(record.total_bytes).unwrap();
            let mut expected = vec![full_backend];
            for &length in reads.iter() {
                assert!(length <= 65536);
                if window_bytes + length as u64 > 1024 * 1024 {
                    expected.push(full_backend - delivered);
                    window_bytes = 0;
                }
                window_bytes += length as u64;
                delivered += length as u64;
            }
            assert!(window_bytes > 0 && window_bytes < 1024 * 1024);
            expected.push(full_backend - delivered); // Fresh final admission observation.
            assert_eq!(expected.len(), 3);
            assert_eq!(
                *backend.capacity_requests.lock().unwrap(),
                expected,
                "shared={shared}"
            );
            let current = load_operation(root.path(), &record.operation_id).unwrap();
            assert_eq!(current.state, PreparationState::Admitted);
            assert_eq!(current.reserved_bytes, record.reserved_bytes);
            let calls = backend.calls.lock().unwrap();
            let hash = calls
                .iter()
                .position(|op| op == "runtime_hash_staged_directory")
                .unwrap();
            assert_eq!(calls[hash - 1], "runtime_check_capacity");
        }
    }

    #[tokio::test]
    async fn model_preparation_capacity_windows_refuse_before_next_read_or_admission() {
        for fail_at in [1, 2, 3] {
            let (root, record, backend, registry) =
                capacity_window_fixture(true, Some(fail_at)).await;
            let revalidate: Revalidate = Arc::new(|| Ok(()));
            let error = prepare(
                root.path(),
                &registry,
                &record.operation_id,
                &AtomicBool::new(false),
                &revalidate,
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error.downcast_ref::<elastos_runtime::provider::ProviderError>(),
                Some(elastos_runtime::provider::ProviderError::Provider(message))
                    if message == "local content preparation unavailable"
            ));
            assert_eq!(backend.capacity_requests.lock().unwrap().len(), fail_at);
            let reads = backend.read_sizes.lock().unwrap();
            let delivered: usize = reads.iter().sum();
            match fail_at {
                1 => assert_eq!(delivered, 0),
                2 => {
                    assert!(delivered <= 1024 * 1024);
                    assert!(
                        delivered + 65536 > 1024 * 1024,
                        "backend checked before its byte window was consumed: {delivered}"
                    );
                }
                3 => assert_eq!(
                    delivered as u64,
                    record.total_bytes + backend.files["_elastos_object.json"].len() as u64
                ),
                _ => unreachable!(),
            }
            assert!(!backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|op| op == "runtime_hash_staged_directory"));
            assert_eq!(
                load_operation(root.path(), &record.operation_id)
                    .unwrap()
                    .reserved_bytes,
                record.reserved_bytes,
                "failure keeps charge until cleanup settles"
            );
        }
    }

    #[tokio::test]
    async fn model_preparation_capacity_windows_runtime_floor_counts_shared_backend_growth() {
        let (_root, mut record, _, _) = capacity_window_fixture(true, None).await;
        let capacity = 1_u128 << 30;
        let floor = capacity.div_ceil(10);
        for completed in [0, 65536, 1024 * 1024, record.total_bytes] {
            record.completed_bytes = completed;
            record.index_bytes = 717;
            let (stage, backend) =
                remaining_capacity_charges(record.total_bytes, completed, record.index_bytes)
                    .unwrap();
            for shared in [false, true] {
                let required = runtime_capacity_charge(&record, shared).unwrap();
                assert_eq!(required, if shared { stage + backend } else { stage });
                assert!(storage::require_space_floor(
                    capacity,
                    floor + u128::from(required),
                    required.into()
                )
                .is_ok());
                assert!(storage::require_space_floor(
                    capacity,
                    floor + u128::from(required) - 1,
                    required.into()
                )
                .is_err());
            }
            // A Runtime volume with only the stage allowance cannot pass as shared.
            assert!(storage::require_space_floor(
                capacity,
                floor + u128::from(stage),
                runtime_capacity_charge(&record, true).unwrap().into()
            )
            .is_err());
        }
    }

    #[tokio::test]
    async fn model_preparation_capacity_windows_revalidate_between_chunks() {
        for fault in ["cancel", "policy", "actor", "budget"] {
            let (root, record, backend, registry) = capacity_window_fixture(true, None).await;
            let stopped = Arc::new(AtomicBool::new(false));
            let actor_revoked = Arc::new(AtomicBool::new(false));
            let stop = stopped.clone();
            let actor = actor_revoked.clone();
            let path = root.path().to_path_buf();
            *backend.after_weight_read.lock().unwrap() = Some(Arc::new(move || match fault {
                "cancel" => stop.store(true, Ordering::Release),
                "actor" => actor.store(true, Ordering::Release),
                "policy" => change_config(&path, |config| {
                    config["model_catalog"]["local_use"] = serde_json::Value::Null
                }),
                "budget" => change_config(&path, |config| {
                    config["model_catalog"]["local_use"]["max_cache_bytes"] = serde_json::json!(1)
                }),
                _ => unreachable!(),
            }));
            let revalidate: Revalidate = Arc::new(move || {
                ensure!(
                    !actor_revoked.load(Ordering::Acquire),
                    "fixture actor revoked"
                );
                Ok(())
            });
            assert!(
                prepare(
                    root.path(),
                    &registry,
                    &record.operation_id,
                    &stopped,
                    &revalidate
                )
                .await
                .is_err(),
                "{fault}"
            );
            let reads = backend.read_sizes.lock().unwrap();
            assert_eq!(
                reads.iter().filter(|&&length| length == 65536).count(),
                1,
                "{fault}"
            );
            assert_eq!(
                backend.capacity_requests.lock().unwrap().len(),
                1,
                "{fault}: authority checks must remain inside the first capacity window"
            );
            assert!(!backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|op| op == "runtime_hash_staged_directory"));
        }
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
            retention_claims: Vec::new(),
            retirement: None,
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
