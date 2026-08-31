use crate::config::{
    MAX_CANCEL_SETTLEMENT_TIMEOUT_MS, MAX_CONCURRENCY_LIMIT, MAX_EVENT_BYTES_LIMIT,
    MAX_INLINE_OUTPUT_BYTES_LIMIT, MAX_INPUT_BYTES_LIMIT, MAX_MODALITIES_PER_OFFER,
    MAX_MODALITY_BYTES, MAX_OFFER_ID_BYTES, MAX_OFFER_TITLE_BYTES, MAX_OPERATION_BYTES,
    MAX_RETENTION_SECS, MAX_RUNTIME_MS_LIMIT, MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT,
    MAX_RUN_EVENT_COUNT_LIMIT,
};
use crate::contract::{
    hex_hash, model_input_hash, validate_bounded_trimmed, validate_input_hash, validate_run_id,
    OfferSummary, ProviderFault, RunError, RunEvent, RunStatus, RunTerminalOutcome, RunView,
    RuntimeCreateBinding, MAX_EVENT_SEQUENCE, MODEL_POLICY_SCHEMA, RUN_EVENT_SCHEMA,
    RUN_OUTPUT_CONTENT_SCHEMA, RUN_OUTPUT_OBJECT_SCHEMA, RUN_OUTPUT_TEXT_SCHEMA,
};
use elastos_model_contract::{
    MAX_RUNTIME_BINDING_ID_BYTES, MAX_RUNTIME_OPERATION_BYTES, RUNTIME_CREATE_BINDING_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RUN_JOURNAL_SCHEMA: &str = "elastos.model.provider-run-journal/v1";
pub const MAX_RUN_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_RUN_ERROR_CODE_BYTES: usize = 128;
pub const MAX_RUN_ERROR_MESSAGE_BYTES: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRun {
    pub schema: String,
    pub run_id: String,
    pub runtime_binding: RuntimeCreateBinding,
    pub offer: OfferSummary,
    pub execution_binding_hash: String,
    pub input_hash: String,
    pub status: RunStatus,
    pub next_sequence: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub deadline_ms: u64,
    pub terminal_at_ms: Option<u64>,
    pub retention_until_ms: Option<u64>,
    pub output: Option<Value>,
    pub error: Option<RunError>,
    pub backend_state: Option<Value>,
    pub events: Vec<RunEvent>,
}

impl StoredRun {
    pub fn new_prepared(
        run_id: String,
        runtime_binding: RuntimeCreateBinding,
        offer: OfferSummary,
        execution_binding_hash: String,
        created_at_ms: u64,
    ) -> Self {
        let deadline_ms = created_at_ms.saturating_add(offer.policy.runtime_ms_limit);
        Self {
            schema: RUN_JOURNAL_SCHEMA.to_string(),
            run_id,
            input_hash: runtime_binding.input_hash.clone(),
            runtime_binding,
            offer,
            execution_binding_hash,
            status: RunStatus::Prepared,
            next_sequence: 1,
            created_at_ms,
            updated_at_ms: created_at_ms,
            deadline_ms,
            terminal_at_ms: None,
            retention_until_ms: None,
            output: None,
            error: None,
            backend_state: None,
            events: Vec::new(),
        }
    }

    pub fn to_view(&self) -> RunView {
        RunView {
            schema: crate::contract::RUN_SCHEMA.to_string(),
            run_id: self.run_id.clone(),
            offer_id: self.offer.id.clone(),
            operation: self.offer.operation.clone(),
            status: self.status.clone(),
            sequence_cursor: self.next_sequence.saturating_sub(1),
            terminal: self.status.is_terminal().then(|| RunTerminalOutcome {
                status: self.status.clone(),
                output: self.output.clone(),
                error: self.error.clone(),
            }),
        }
    }
}

pub struct RunJournal {
    runs_dir: PathBuf,
}

impl RunJournal {
    pub fn open(root: PathBuf) -> Result<Self, ProviderFault> {
        let runs_dir = root.join("runs");
        ensure_journal_dir_owner_only(&root)?;
        ensure_journal_dir_owner_only(&runs_dir)?;
        let journal = Self { runs_dir };
        journal.prune_expired_terminal_runs()?;
        Ok(journal)
    }

    pub fn load_run(&self, run_id: &str) -> Result<StoredRun, ProviderFault> {
        validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
        let path = hashed_path(&self.runs_dir, run_id);
        load_verified_run_file(&path, &self.runs_dir, Some(run_id))
    }

    pub fn load_run_if_present(&self, run_id: &str) -> Result<Option<StoredRun>, ProviderFault> {
        validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
        let path = hashed_path(&self.runs_dir, run_id);
        match fs::symlink_metadata(&path) {
            Ok(_) => self.load_run(run_id).map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ProviderFault::corrupt_journal(format!(
                "failed to inspect model run journal {}: {err}",
                path.display()
            ))),
        }
    }

    pub fn store_run(&self, run: &StoredRun) -> Result<(), ProviderFault> {
        validate_run_id(&run.run_id).map_err(|_| ProviderFault::internal("invalid run_id"))?;
        let path = hashed_path(&self.runs_dir, &run.run_id);
        validate_stored_run(&path, &self.runs_dir, Some(&run.run_id), run)?;
        let bytes = serde_json::to_vec_pretty(run).map_err(|err| {
            ProviderFault::internal(format!("failed to encode run journal: {err}"))
        })?;
        if bytes.len() as u64 > MAX_RUN_JOURNAL_BYTES {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal exceeds {} bytes for {}",
                MAX_RUN_JOURNAL_BYTES,
                path.display()
            )));
        }
        atomic_write_owner_only(&path, &bytes)
    }

    pub fn active_run_count(&self, offer_id: &str) -> Result<usize, ProviderFault> {
        let mut total = 0usize;
        for (_, run) in self.scan_runs()? {
            if run.offer.id == offer_id && !run.status.is_terminal() {
                total = total.saturating_add(1);
            }
        }
        Ok(total)
    }

    pub fn prune_expired_terminal_runs(&self) -> Result<(), ProviderFault> {
        for (path, run) in self.scan_runs()? {
            if is_expired_terminal_run(&run, now_ms()) {
                delete_verified_run_file(&path, &self.runs_dir, &run.run_id)?;
            }
        }
        Ok(())
    }

    pub fn prune_expired_loaded_run(&self, run: &StoredRun) -> Result<bool, ProviderFault> {
        if !is_expired_terminal_run(run, now_ms()) {
            return Ok(false);
        }
        let path = hashed_path(&self.runs_dir, &run.run_id);
        delete_verified_run_file(&path, &self.runs_dir, &run.run_id)?;
        Ok(true)
    }

    pub(crate) fn scan_runs(&self) -> Result<Vec<(PathBuf, StoredRun)>, ProviderFault> {
        let entries = fs::read_dir(&self.runs_dir).map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to scan model run journal {}: {err}",
                self.runs_dir.display()
            ))
        })?;
        let mut runs = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to scan model run journal {}: {err}",
                    self.runs_dir.display()
                ))
            })?;
            let path = entry.path();
            if is_transaction_temp_file(&path) {
                continue;
            }
            let run = load_verified_run_file(&path, &self.runs_dir, None)?;
            runs.push((path, run));
        }
        Ok(runs)
    }
}

pub fn request_fingerprint(
    binding: &RuntimeCreateBinding,
    input_hash: &str,
) -> Result<String, ProviderFault> {
    let value = serde_json::json!({
        "principal_id": binding.principal_id,
        "capsule_id": binding.capsule_id,
        "request_id": binding.request_id,
        "offer_id": binding.offer_id,
        "operation": binding.operation,
        "input_hash": input_hash,
    });
    model_input_hash(&value).map_err(|err| {
        ProviderFault::internal(format!("failed to hash request fingerprint: {err}"))
    })
}

pub fn deterministic_run_id(binding: &RuntimeCreateBinding) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"elastos:model-run:v1\n");
    hasher.update(binding.principal_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(binding.capsule_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(binding.request_id.as_bytes());
    format!("run:sha256:{}", hex_hash(&hasher.finalize()))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn hashed_path(root: &Path, identifier: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(identifier.as_bytes());
    root.join(format!("sha256-{}.json", hex_hash(&hasher.finalize())))
}

fn ensure_journal_dir_owner_only(path: &Path) -> Result<(), ProviderFault> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_existing_directory(path, &metadata)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            create_missing_journal_directory(path)?;
        }
        Err(err) => {
            return Err(ProviderFault::corrupt_journal(format!(
                "failed to inspect provider journal directory {}: {err}",
                path.display()
            )))
        }
    }
    Ok(())
}

fn atomic_write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), ProviderFault> {
    if let Some(parent) = path.parent() {
        ensure_journal_dir_owner_only(parent)?;
    }
    validate_run_path_target(path)?;
    let tmp = allocate_temp_path(path)?;
    {
        let mut file = open_temp_file_owner_only(&tmp)?;
        validate_existing_file(
            &tmp,
            &file.metadata().map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to inspect temporary journal file {}: {err}",
                    tmp.display()
                ))
            })?,
        )?;
        file.write_all(bytes).map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to write temporary journal file {}: {err}",
                tmp.display()
            ))
        })?;
        file.sync_all().map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to sync temporary journal file {}: {err}",
                tmp.display()
            ))
        })?;
    }
    fs::rename(&tmp, path).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to replace provider journal {}: {err}",
            path.display()
        ))
    })?;
    sync_directory(
        path.parent()
            .ok_or_else(|| ProviderFault::corrupt_journal("run journal file missing parent"))?,
    )?;
    Ok(())
}

fn load_verified_run_file(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: Option<&str>,
) -> Result<StoredRun, ProviderFault> {
    if is_transaction_temp_file(path) {
        return Err(ProviderFault::corrupt_journal(format!(
            "temporary model run journal file is not a committed run: {}",
            path.display()
        )));
    }
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to inspect model run journal {}: {err}",
            path.display()
        ))
    })?;
    validate_existing_file(path, &metadata)?;
    if metadata.len() > MAX_RUN_JOURNAL_BYTES {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal exceeds {} bytes at {}",
            MAX_RUN_JOURNAL_BYTES,
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to read model run journal {}: {err}",
            path.display()
        ))
    })?;
    let run = serde_json::from_slice::<StoredRun>(&bytes).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "corrupt model run journal {}: {err}",
            path.display()
        ))
    })?;
    validate_stored_run(path, runs_dir, expected_run_id, &run)?;
    Ok(run)
}

fn validate_stored_run(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: Option<&str>,
    run: &StoredRun,
) -> Result<(), ProviderFault> {
    if run.schema != RUN_JOURNAL_SCHEMA {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal schema mismatch at {}",
            path.display()
        )));
    }
    validate_run_id(&run.run_id).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid run_id at {}",
            path.display()
        ))
    })?;
    if expected_run_id.is_some_and(|expected| expected != run.run_id) {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal identifier mismatch at {}",
            path.display()
        )));
    }
    let expected_path = hashed_path(runs_dir, &run.run_id);
    if path.file_name() != expected_path.file_name() {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal filename mismatch at {}",
            path.display()
        )));
    }
    validate_stored_runtime_binding(path, run)?;
    validate_stored_offer(path, &run.offer)?;
    validate_stored_execution_binding_hash(path, &run.execution_binding_hash)?;
    validate_stored_lifecycle(path, run)?;
    validate_stored_events(path, run)?;
    validate_stored_terminal_state(path, run)?;
    Ok(())
}

fn validate_stored_execution_binding_hash(path: &Path, value: &str) -> Result<(), ProviderFault> {
    validate_input_hash(value).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid execution_binding_hash at {}",
            path.display()
        ))
    })
}

fn validate_stored_lifecycle(path: &Path, run: &StoredRun) -> Result<(), ProviderFault> {
    if run.created_at_ms > run.updated_at_ms {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal chronology is invalid at {}",
            path.display()
        )));
    }
    if run.deadline_ms
        != run
            .created_at_ms
            .saturating_add(run.offer.policy.runtime_ms_limit)
    {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal deadline is invalid at {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_stored_runtime_binding(path: &Path, run: &StoredRun) -> Result<(), ProviderFault> {
    let binding = &run.runtime_binding;
    if binding.schema != RUNTIME_CREATE_BINDING_SCHEMA {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal binding schema mismatch at {}",
            path.display()
        )));
    }
    for (value, label, max) in [
        (
            binding.principal_id.as_str(),
            "principal_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.session_id.as_str(),
            "session_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.capsule_id.as_str(),
            "capsule_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.grant_id.as_str(),
            "grant_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.request_id.as_str(),
            "request_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.offer_id.as_str(),
            "offer_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        ),
        (
            binding.operation.as_str(),
            "operation",
            MAX_RUNTIME_OPERATION_BYTES,
        ),
    ] {
        validate_bounded_trimmed(value, label, max).map_err(|_| {
            ProviderFault::corrupt_journal(format!(
                "model run journal has invalid {label} at {}",
                path.display()
            ))
        })?;
    }
    validate_input_hash(&binding.input_hash).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid input_hash at {}",
            path.display()
        ))
    })?;
    if binding.offer_id != run.offer.id
        || binding.operation != run.offer.operation
        || binding.input_hash != run.input_hash
    {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal binding mismatch at {}",
            path.display()
        )));
    }
    validate_input_hash(&run.input_hash).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid stored input_hash at {}",
            path.display()
        ))
    })?;
    Ok(())
}

fn validate_stored_offer(path: &Path, offer: &OfferSummary) -> Result<(), ProviderFault> {
    validate_bounded_trimmed(&offer.id, "offer id", MAX_OFFER_ID_BYTES).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid offer id at {}",
            path.display()
        ))
    })?;
    validate_bounded_trimmed(&offer.title, "offer title", MAX_OFFER_TITLE_BYTES).map_err(|_| {
        ProviderFault::corrupt_journal(format!(
            "model run journal has invalid offer title at {}",
            path.display()
        ))
    })?;
    validate_bounded_trimmed(&offer.operation, "offer operation", MAX_OPERATION_BYTES).map_err(
        |_| {
            ProviderFault::corrupt_journal(format!(
                "model run journal has invalid offer operation at {}",
                path.display()
            ))
        },
    )?;
    validate_modalities(path, "input", &offer.input_modalities)?;
    validate_modalities(path, "output", &offer.output_modalities)?;
    if offer.policy.schema != MODEL_POLICY_SCHEMA {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal policy schema mismatch at {}",
            path.display()
        )));
    }
    if offer.policy.concurrency_limit == 0 || offer.policy.concurrency_limit > MAX_CONCURRENCY_LIMIT
    {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal concurrency policy is invalid at {}",
            path.display()
        )));
    }
    for (value, max, label) in [
        (
            offer.policy.input_bytes_limit,
            MAX_INPUT_BYTES_LIMIT,
            "input_bytes_limit",
        ),
        (
            offer.policy.inline_output_bytes_limit,
            MAX_INLINE_OUTPUT_BYTES_LIMIT,
            "inline_output_bytes_limit",
        ),
        (
            offer.policy.event_bytes_limit,
            MAX_EVENT_BYTES_LIMIT,
            "event_bytes_limit",
        ),
        (
            offer.policy.runtime_ms_limit,
            MAX_RUNTIME_MS_LIMIT,
            "runtime_ms_limit",
        ),
        (
            offer.policy.retention_secs,
            MAX_RETENTION_SECS,
            "retention_secs",
        ),
        (
            offer.policy.cancel_settlement_timeout_ms,
            MAX_CANCEL_SETTLEMENT_TIMEOUT_MS,
            "cancel_settlement_timeout_ms",
        ),
    ] {
        if value == 0 || value > max {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal {label} is invalid at {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_modalities(path: &Path, label: &str, values: &[String]) -> Result<(), ProviderFault> {
    if values.is_empty() || values.len() > MAX_MODALITIES_PER_OFFER {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal {label} modalities are invalid at {}",
            path.display()
        )));
    }
    for value in values {
        validate_bounded_trimmed(value, label, MAX_MODALITY_BYTES).map_err(|_| {
            ProviderFault::corrupt_journal(format!(
                "model run journal {label} modality is invalid at {}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

fn validate_stored_events(path: &Path, run: &StoredRun) -> Result<(), ProviderFault> {
    if run.events.len() > MAX_RUN_EVENT_COUNT_LIMIT {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal event count exceeds provider limit at {}",
            path.display()
        )));
    }
    let mut aggregate_bytes = 0u64;
    let mut saw_terminal = false;
    for (index, event) in run.events.iter().enumerate() {
        if event.schema != RUN_EVENT_SCHEMA {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal event schema mismatch at {}",
                path.display()
            )));
        }
        let expected_sequence = (index as u64).saturating_add(1);
        if event.sequence != expected_sequence || event.sequence > MAX_EVENT_SEQUENCE {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal event sequence is invalid at {}",
                path.display()
            )));
        }
        let encoded = serde_json::to_vec(event).map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to encode model run journal event {}: {err}",
                path.display()
            ))
        })?;
        let encoded_len = encoded.len() as u64;
        if encoded_len > run.offer.policy.event_bytes_limit || encoded_len > MAX_EVENT_BYTES_LIMIT {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal event exceeds provider limits at {}",
                path.display()
            )));
        }
        aggregate_bytes = aggregate_bytes.saturating_add(encoded_len);
        if aggregate_bytes > MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal event retention exceeds provider limits at {}",
                path.display()
            )));
        }
        if event.terminal {
            if saw_terminal || index + 1 != run.events.len() {
                return Err(ProviderFault::corrupt_journal(format!(
                    "model run journal terminal event ordering is invalid at {}",
                    path.display()
                )));
            }
            saw_terminal = true;
        }
    }
    let expected_next = if run.events.is_empty() {
        1
    } else {
        run.events
            .last()
            .map(|event| event.sequence.saturating_add(1))
            .unwrap_or(1)
    };
    if run.next_sequence != expected_next {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal next_sequence is invalid at {}",
            path.display()
        )));
    }
    if run.status.is_terminal() != saw_terminal {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal terminal event state is invalid at {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_stored_terminal_state(path: &Path, run: &StoredRun) -> Result<(), ProviderFault> {
    if run.status.is_terminal() {
        let terminal_at_ms = run.terminal_at_ms.ok_or_else(|| {
            ProviderFault::corrupt_journal(format!(
                "model run journal terminal timestamp is missing at {}",
                path.display()
            ))
        })?;
        let retention_until_ms = run.retention_until_ms.ok_or_else(|| {
            ProviderFault::corrupt_journal(format!(
                "model run journal retention timestamp is missing at {}",
                path.display()
            ))
        })?;
        if retention_until_ms < terminal_at_ms {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal retention precedes terminal timestamp at {}",
                path.display()
            )));
        }
        if terminal_at_ms < run.created_at_ms
            || terminal_at_ms > run.updated_at_ms
            || retention_until_ms
                != terminal_at_ms
                    .saturating_add(run.offer.policy.retention_secs.saturating_mul(1_000))
        {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal terminal lifecycle is invalid at {}",
                path.display()
            )));
        }
        if run.backend_state.is_some() {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal terminal state retains backend state at {}",
                path.display()
            )));
        }
        match run.status {
            RunStatus::Completed => {
                if run.error.is_some() {
                    return Err(ProviderFault::corrupt_journal(format!(
                        "model run journal completed state has terminal error at {}",
                        path.display()
                    )));
                }
                if let Some(output) = run.output.as_ref() {
                    validate_stored_output(path, run, output)?;
                }
            }
            RunStatus::Failed | RunStatus::Cancelled | RunStatus::SettlementUnknown => {
                if run.output.is_some() || run.error.is_none() {
                    return Err(ProviderFault::corrupt_journal(format!(
                        "model run journal terminal output/error state is invalid at {}",
                        path.display()
                    )));
                }
                validate_run_error(run.error.as_ref().unwrap()).map_err(|_| {
                    ProviderFault::corrupt_journal(format!(
                        "model run journal terminal error is invalid at {}",
                        path.display()
                    ))
                })?;
            }
            _ => {
                return Err(ProviderFault::corrupt_journal(format!(
                    "model run journal has invalid terminal status at {}",
                    path.display()
                )))
            }
        }
        return Ok(());
    }

    match run.status {
        RunStatus::Prepared => {
            if run.backend_state.is_some() {
                return Err(ProviderFault::corrupt_journal(format!(
                    "model run journal prepared state retains backend state at {}",
                    path.display()
                )));
            }
        }
        RunStatus::Running | RunStatus::Reconciling => {
            if run.backend_state.is_none() {
                return Err(ProviderFault::corrupt_journal(format!(
                    "model run journal running state is missing backend state at {}",
                    path.display()
                )));
            }
        }
        _ => {}
    }
    if run.terminal_at_ms.is_some()
        || run.retention_until_ms.is_some()
        || run.output.is_some()
        || run.error.is_some()
        || run.events.iter().any(|event| event.terminal)
    {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal non-terminal state is invalid at {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn validate_run_error(error: &RunError) -> anyhow::Result<()> {
    validate_bounded_trimmed(&error.code, "error code", MAX_RUN_ERROR_CODE_BYTES)?;
    validate_bounded_trimmed(&error.message, "error message", MAX_RUN_ERROR_MESSAGE_BYTES)?;
    Ok(())
}

fn validate_stored_output(
    path: &Path,
    run: &StoredRun,
    output: &Value,
) -> Result<(), ProviderFault> {
    let encoded = serde_json::to_vec(output).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to encode model run journal output {}: {err}",
            path.display()
        ))
    })?;
    let encoded_len = encoded.len() as u64;
    if encoded_len > run.offer.policy.inline_output_bytes_limit
        || encoded_len > MAX_INLINE_OUTPUT_BYTES_LIMIT
    {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal output exceeds provider limits at {}",
            path.display()
        )));
    }
    let schema = output
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderFault::corrupt_journal(format!(
                "model run journal output schema is missing at {}",
                path.display()
            ))
        })?;
    if !matches!(
        schema,
        RUN_OUTPUT_TEXT_SCHEMA | RUN_OUTPUT_OBJECT_SCHEMA | RUN_OUTPUT_CONTENT_SCHEMA
    ) {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal output schema is invalid at {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_expired_terminal_run(run: &StoredRun, now_ms: u64) -> bool {
    run.status.is_terminal()
        && run
            .retention_until_ms
            .is_some_and(|retention_until_ms| now_ms > retention_until_ms)
}

fn delete_verified_run_file(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: &str,
) -> Result<(), ProviderFault> {
    let run = load_verified_run_file(path, runs_dir, Some(expected_run_id))?;
    if !is_expired_terminal_run(&run, now_ms()) {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal is not expired at {}",
            path.display()
        )));
    }
    fs::remove_file(path).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to delete model run journal {}: {err}",
            path.display()
        ))
    })?;
    sync_directory(runs_dir)?;
    Ok(())
}

fn validate_run_path_target(path: &Path) -> Result<(), ProviderFault> {
    if let Some(parent) = path.parent() {
        ensure_journal_dir_owner_only(parent)?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        validate_existing_file(path, &metadata)?;
    }
    Ok(())
}

fn allocate_temp_path(path: &Path) -> Result<PathBuf, ProviderFault> {
    let parent = path.parent().ok_or_else(|| {
        ProviderFault::corrupt_journal(format!(
            "provider journal path has no parent: {}",
            path.display()
        ))
    })?;
    let stem = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ProviderFault::corrupt_journal("invalid journal filename"))?;
    for attempt in 0..128u32 {
        let candidate = parent.join(format!(
            ".{stem}.tmp-{}-{}-{}",
            std::process::id(),
            now_ms(),
            attempt
        ));
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ProviderFault::corrupt_journal(format!(
                        "provider journal temporary path is a symlink: {}",
                        candidate.display()
                    )));
                }
                continue;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(err) => {
                return Err(ProviderFault::corrupt_journal(format!(
                    "failed to inspect provider journal temporary path {}: {err}",
                    candidate.display()
                )))
            }
        }
    }
    Err(ProviderFault::corrupt_journal(format!(
        "failed to allocate temporary journal path for {}",
        path.display()
    )))
}

fn is_transaction_temp_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    name.starts_with(".sha256-") && name.contains(".json.tmp-")
}

fn create_missing_journal_directory(path: &Path) -> Result<(), ProviderFault> {
    let parent = path.parent().ok_or_else(|| {
        ProviderFault::corrupt_journal(format!(
            "provider journal path has no parent: {}",
            path.display()
        ))
    })?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ProviderFault::corrupt_journal(format!(
                    "provider journal parent path is a symlink: {}",
                    parent.display()
                )));
            }
            if !metadata.file_type().is_dir() {
                return Err(ProviderFault::corrupt_journal(format!(
                    "provider journal parent path is not a directory: {}",
                    parent.display()
                )));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            create_missing_journal_directory(parent)?;
        }
        Err(err) => {
            return Err(ProviderFault::corrupt_journal(format!(
                "failed to inspect provider journal parent directory {}: {err}",
                parent.display()
            )))
        }
    }
    fs::create_dir(path).map_err(|create_err| {
        ProviderFault::corrupt_journal(format!(
            "failed to create provider journal directory {}: {create_err}",
            path.display()
        ))
    })?;
    set_owner_only_directory_mode(path)
}

fn open_temp_file_owner_only(path: &Path) -> Result<fs::File, ProviderFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to create temporary journal file {}: {err}",
                    path.display()
                ))
            })
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to create temporary journal file {}: {err}",
                    path.display()
                ))
            })
    }
}

fn validate_existing_directory(path: &Path, metadata: &fs::Metadata) -> Result<(), ProviderFault> {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(ProviderFault::corrupt_journal(format!(
            "provider journal path is a symlink: {}",
            path.display()
        )));
    }
    if !file_type.is_dir() {
        return Err(ProviderFault::corrupt_journal(format!(
            "provider journal path is not a directory: {}",
            path.display()
        )));
    }
    validate_unix_owner_and_mode(path, metadata, 0o700, "directory")
}

fn validate_existing_file(path: &Path, metadata: &fs::Metadata) -> Result<(), ProviderFault> {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(ProviderFault::corrupt_journal(format!(
            "provider journal path is a symlink: {}",
            path.display()
        )));
    }
    if !file_type.is_file() {
        return Err(ProviderFault::corrupt_journal(format!(
            "provider journal path is not a regular file: {}",
            path.display()
        )));
    }
    validate_unix_owner_and_mode(path, metadata, 0o600, "file")
}

fn set_owner_only_directory_mode(path: &Path) -> Result<(), ProviderFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to secure provider journal directory {}: {err}",
                path.display()
            ))
        })?;
        validate_existing_directory(
            path,
            &fs::symlink_metadata(path).map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to inspect provider journal directory {}: {err}",
                    path.display()
                ))
            })?,
        )?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ProviderFault> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to sync provider journal directory {}: {err}",
                    path.display()
                ))
            })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn validate_unix_owner_and_mode(
    path: &Path,
    metadata: &fs::Metadata,
    expected_mode: u32,
    label: &str,
) -> Result<(), ProviderFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode() & 0o777;
        if mode != expected_mode {
            return Err(ProviderFault::corrupt_journal(format!(
                "provider journal {label} has unsafe mode {:o} at {}",
                mode,
                path.display()
            )));
        }
        if metadata.uid() != current_euid() {
            return Err(ProviderFault::corrupt_journal(format!(
                "provider journal {label} has unexpected owner at {}",
                path.display()
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata, expected_mode, label);
    }
    Ok(())
}

#[cfg(unix)]
fn current_euid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OfferPolicy, MAX_EVENT_BYTES_LIMIT};
    use crate::contract::{
        model_input_hash, OfferSummary, RunError, RunEvent, RunStatus, RuntimeCreateBinding,
        RUN_EVENT_SCHEMA, RUN_OUTPUT_TEXT_SCHEMA,
    };
    use elastos_model_contract::RUNTIME_CREATE_BINDING_SCHEMA;
    fn temp_root(label: &str) -> PathBuf {
        crate::test_support::temp_root_path("model-provider-journal", label)
    }

    fn configured_offer() -> crate::config::ConfiguredOffer {
        crate::config::ConfiguredOffer {
            id: "local-text".to_string(),
            title: "Local text".to_string(),
            operation: "text.generate".to_string(),
            input_modalities: vec!["text/plain".to_string()],
            output_modalities: vec!["text/plain".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 1,
                input_bytes_limit: 8 * 1024,
                inline_output_bytes_limit: 8 * 1024,
                event_bytes_limit: 8 * 1024,
                runtime_ms_limit: 30,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 5,
            },
            adapter: crate::config::AdapterConfig::OpenAiCompatibleText {
                api_url: "https://example.invalid/v1/chat/completions".to_string(),
                api_key: None,
                model: "gpt-test".to_string(),
            },
            enabled: true,
        }
    }

    fn binding(request_id: &str) -> RuntimeCreateBinding {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        RuntimeCreateBinding {
            schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            capsule_id: "assistant".to_string(),
            grant_id: "grant:test".to_string(),
            request_id: request_id.to_string(),
            offer_id: "local-text".to_string(),
            operation: "text.generate".to_string(),
            input_hash: model_input_hash(&input).unwrap(),
        }
    }

    fn offer_summary() -> OfferSummary {
        configured_offer().summary()
    }

    fn execution_binding_hash() -> String {
        configured_offer().execution_binding_hash().unwrap()
    }

    fn prepared_run(request_id: &str) -> StoredRun {
        let run_binding = binding(request_id);
        StoredRun::new_prepared(
            deterministic_run_id(&run_binding),
            run_binding.clone(),
            offer_summary(),
            execution_binding_hash(),
            now_ms(),
        )
    }

    fn completed_run(
        request_id: &str,
        created_at_ms: u64,
        terminal_at_ms: u64,
        retention_secs: u64,
    ) -> StoredRun {
        let mut run = prepared_run(request_id);
        let output = serde_json::json!({
            "schema": RUN_OUTPUT_TEXT_SCHEMA,
            "text": "done"
        });
        run.created_at_ms = created_at_ms;
        run.updated_at_ms = terminal_at_ms;
        run.deadline_ms = created_at_ms.saturating_add(run.offer.policy.runtime_ms_limit);
        run.offer.policy.retention_secs = retention_secs;
        run.status = RunStatus::Completed;
        run.next_sequence = 2;
        run.terminal_at_ms = Some(terminal_at_ms);
        run.retention_until_ms =
            Some(terminal_at_ms.saturating_add(retention_secs.saturating_mul(1_000)));
        run.output = Some(output.clone());
        run.events = vec![RunEvent {
            schema: RUN_EVENT_SCHEMA.to_string(),
            sequence: 1,
            kind: "output".to_string(),
            data: output,
            terminal: true,
        }];
        run
    }

    fn failed_run(
        request_id: &str,
        created_at_ms: u64,
        terminal_at_ms: u64,
        retention_secs: u64,
    ) -> StoredRun {
        let mut run = prepared_run(request_id);
        run.created_at_ms = created_at_ms;
        run.updated_at_ms = terminal_at_ms;
        run.deadline_ms = created_at_ms.saturating_add(run.offer.policy.runtime_ms_limit);
        run.offer.policy.retention_secs = retention_secs;
        run.status = RunStatus::Failed;
        run.next_sequence = 2;
        run.terminal_at_ms = Some(terminal_at_ms);
        run.retention_until_ms =
            Some(terminal_at_ms.saturating_add(retention_secs.saturating_mul(1_000)));
        run.error = Some(RunError {
            class: crate::contract::ErrorClass::BackendFailed,
            code: "backend_failed".to_string(),
            message: "model backend failed".to_string(),
        });
        run.events = vec![RunEvent {
            schema: RUN_EVENT_SCHEMA.to_string(),
            sequence: 1,
            kind: "failed".to_string(),
            data: serde_json::json!({
                "code": "backend_failed",
                "class": "backend_failed",
            }),
            terminal: true,
        }];
        run
    }

    fn run_path(root: &Path, run_id: &str) -> PathBuf {
        hashed_path(&root.join("runs"), run_id)
    }

    fn temp_run_path(root: &Path, run_id: &str) -> PathBuf {
        let path = run_path(root, run_id);
        let file_name = path.file_name().and_then(|value| value.to_str()).unwrap();
        path.parent()
            .unwrap()
            .join(format!(".{file_name}.tmp-test"))
    }

    fn file_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    fn overwrite_run_file(path: &Path, run: &StoredRun) {
        fs::write(path, serde_json::to_vec_pretty(run).unwrap()).unwrap();
        #[cfg(unix)]
        set_mode(path, 0o600);
    }

    #[test]
    fn prepared_run_deadline_uses_runtime_ms_limit_as_milliseconds() {
        let created_at_ms = 1_000;
        let run_binding = binding("request:deadline-ms");
        let run = StoredRun::new_prepared(
            deterministic_run_id(&run_binding),
            run_binding,
            offer_summary(),
            execution_binding_hash(),
            created_at_ms,
        );
        assert_eq!(run.deadline_ms, created_at_ms + 30);
    }

    #[test]
    fn deterministic_run_identity_ignores_session_and_grant_but_fingerprint_conflicts_on_request_truth(
    ) {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let base = RuntimeCreateBinding {
            input_hash: model_input_hash(&input).unwrap(),
            ..binding("request:stable")
        };
        let mut rotated = base.clone();
        rotated.session_id = "session:rotated".to_string();
        rotated.grant_id = "grant:rotated".to_string();

        assert_eq!(deterministic_run_id(&base), deterministic_run_id(&rotated));
        assert_eq!(
            request_fingerprint(&base, &base.input_hash).unwrap(),
            request_fingerprint(&rotated, &rotated.input_hash).unwrap()
        );

        let mut changed_offer = rotated.clone();
        changed_offer.offer_id = "other-offer".to_string();
        assert_ne!(
            request_fingerprint(&base, &base.input_hash).unwrap(),
            request_fingerprint(&changed_offer, &changed_offer.input_hash).unwrap()
        );

        let mut changed_operation = rotated.clone();
        changed_operation.operation = "image.generate".to_string();
        assert_ne!(
            request_fingerprint(&base, &base.input_hash).unwrap(),
            request_fingerprint(&changed_operation, &changed_operation.input_hash).unwrap()
        );

        let changed_input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "changed"
        });
        let mut changed_hash = rotated;
        changed_hash.input_hash = model_input_hash(&changed_input).unwrap();
        assert_ne!(
            request_fingerprint(&base, &base.input_hash).unwrap(),
            request_fingerprint(&changed_hash, &changed_hash.input_hash).unwrap()
        );
    }

    #[cfg(unix)]
    fn symlink_path(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn symlink_root_is_rejected() {
        let root = temp_root("symlink-root");
        let target = root.with_extension("target");
        fs::create_dir_all(&target).unwrap();
        symlink_path(&target, &root);
        let error = match RunJournal::open(root) {
            Ok(_) => panic!("symlink root must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(unix)]
    fn existing_non_owner_only_ancestor_is_allowed() {
        let ancestor = temp_root("ancestor");
        fs::create_dir(&ancestor).unwrap();
        set_mode(&ancestor, 0o755);
        let root = ancestor.join("nested").join("provider-journal");
        let journal = RunJournal::open(root.clone()).unwrap();
        assert_eq!(journal.runs_dir, root.join("runs"));
        let ancestor_mode = fs::symlink_metadata(&ancestor).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(ancestor_mode.mode() & 0o777, 0o755);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_run_file_is_rejected() {
        let root = temp_root("symlink-run");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("request:one");
        let linked_run = prepared_run("request:two");
        let real_path = hashed_path(&root.join("runs"), &run.run_id);
        let link_path = hashed_path(&root.join("runs"), &linked_run.run_id);
        fs::write(&real_path, serde_json::to_vec_pretty(&run).unwrap()).unwrap();
        set_mode(&real_path, 0o600);
        symlink_path(&real_path, &link_path);
        let error = journal.load_run(&linked_run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(unix)]
    fn dangling_symlink_run_file_fails_closed() {
        let root = temp_root("dangling-symlink");
        let journal = RunJournal::open(root.clone()).unwrap();
        let linked_run = prepared_run("request:dangling");
        let missing_target = root.join("missing-run.json");
        let link_path = hashed_path(&root.join("runs"), &linked_run.run_id);
        symlink_path(&missing_target, &link_path);
        let error = journal.load_run_if_present(&linked_run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(unix)]
    fn unsafe_directory_mode_is_rejected() {
        let root = temp_root("unsafe-dir");
        fs::create_dir(&root).unwrap();
        set_mode(&root, 0o755);
        let error = match RunJournal::open(root) {
            Ok(_) => panic!("unsafe directory mode must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(unix)]
    fn active_run_scan_verifies_hashed_identifier() {
        let root = temp_root("scan-hash");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("request:scan");
        let wrong_binding = binding("request:other");
        let wrong_path = hashed_path(&root.join("runs"), &deterministic_run_id(&wrong_binding));
        fs::write(&wrong_path, serde_json::to_vec_pretty(&run).unwrap()).unwrap();
        set_mode(&wrong_path, 0o600);
        let error = journal.active_run_count("local-text").unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(unix)]
    fn durable_replace_writes_owner_only_and_cleans_temp_files() {
        let root = temp_root("durable-replace");
        let journal = RunJournal::open(root.clone()).unwrap();
        let mut run = prepared_run("request:replace");
        journal.store_run(&run).unwrap();
        run.updated_at_ms = run.updated_at_ms.saturating_add(1);
        journal.store_run(&run).unwrap();

        let path = hashed_path(&root.join("runs"), &run.run_id);
        let metadata = fs::symlink_metadata(&path).unwrap();
        validate_existing_file(&path, &metadata).unwrap();
        let loaded = journal.load_run(&run.run_id).unwrap();
        assert_eq!(loaded.updated_at_ms, run.updated_at_ms);

        let leftovers = fs::read_dir(root.join("runs"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn open_and_scan_ignore_transaction_temp_files() {
        let root = temp_root("ignore-temp-files");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("request:ignore-temp");
        journal.store_run(&run).unwrap();

        let temp_path = temp_run_path(&root, &run.run_id);
        fs::write(&temp_path, serde_json::to_vec_pretty(&run).unwrap()).unwrap();
        #[cfg(unix)]
        set_mode(&temp_path, 0o600);

        let reopened = RunJournal::open(root.clone()).unwrap();
        assert_eq!(reopened.active_run_count("local-text").unwrap(), 1);
        assert_eq!(reopened.load_run(&run.run_id).unwrap().run_id, run.run_id);
    }

    #[test]
    fn oversized_run_file_is_rejected() {
        let root = temp_root("oversized");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("request:oversized");
        let path = hashed_path(&root.join("runs"), &run.run_id);
        let oversized = vec![b'x'; MAX_RUN_JOURNAL_BYTES as usize + 1];
        fs::write(&path, oversized).unwrap();
        #[cfg(unix)]
        set_mode(&path, 0o600);
        let error = journal.load_run(&run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    fn malformed_sequence_run_fails_closed_and_remains_on_disk() {
        let root = temp_root("bad-sequence");
        let journal = RunJournal::open(root.clone()).unwrap();
        let valid_run = completed_run("request:bad-sequence", 1, 10, 1);
        let mut run = valid_run.clone();
        journal.store_run(&valid_run).unwrap();
        let path = run_path(&root, &run.run_id);
        run.events[0].sequence = 2;
        overwrite_run_file(&path, &run);

        let error = journal.load_run(&run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
        assert!(path.exists());
    }

    #[test]
    fn malformed_terminal_state_fails_closed_and_remains_on_disk() {
        let root = temp_root("bad-terminal");
        let journal = RunJournal::open(root.clone()).unwrap();
        let valid_run = completed_run("request:bad-terminal", 1, 10, 1);
        let mut run = valid_run.clone();
        journal.store_run(&valid_run).unwrap();
        run.events.push(RunEvent {
            schema: RUN_EVENT_SCHEMA.to_string(),
            sequence: 2,
            kind: "progress".to_string(),
            data: serde_json::json!({"step": "late"}),
            terminal: false,
        });
        run.next_sequence = 3;
        let path = run_path(&root, &run.run_id);
        overwrite_run_file(&path, &run);

        let error = journal.load_run(&run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
        assert!(path.exists());
    }

    #[test]
    fn malformed_binding_and_policy_fail_closed_and_remain_on_disk() {
        let root = temp_root("bad-binding-policy");
        let journal = RunJournal::open(root.clone()).unwrap();

        let valid_binding_run = completed_run("request:bad-binding", 1, 10, 1);
        let mut binding_run = valid_binding_run.clone();
        journal.store_run(&valid_binding_run).unwrap();
        binding_run.runtime_binding.offer_id = "other".to_string();
        let binding_path = run_path(&root, &binding_run.run_id);
        overwrite_run_file(&binding_path, &binding_run);
        let binding_error = journal.load_run(&binding_run.run_id).unwrap_err();
        assert_eq!(binding_error.code(), "journal_corrupt");
        assert!(binding_path.exists());

        let valid_policy_run = completed_run("request:bad-policy", 1, 10, 1);
        let mut policy_run = valid_policy_run.clone();
        journal.store_run(&valid_policy_run).unwrap();
        policy_run.offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT + 1;
        let policy_path = run_path(&root, &policy_run.run_id);
        overwrite_run_file(&policy_path, &policy_run);
        let policy_error = journal.load_run(&policy_run.run_id).unwrap_err();
        assert_eq!(policy_error.code(), "journal_corrupt");
        assert!(policy_path.exists());
    }

    #[test]
    fn malformed_execution_binding_hash_fails_closed_and_remains_on_disk() {
        let root = temp_root("bad-execution-binding");
        let journal = RunJournal::open(root.clone()).unwrap();
        let valid_run = completed_run("request:bad-execution-binding", 1, 10, 1);
        journal.store_run(&valid_run).unwrap();

        let mut missing_hash_run = valid_run.clone();
        missing_hash_run.execution_binding_hash.clear();
        let missing_path = run_path(&root, &missing_hash_run.run_id);
        overwrite_run_file(&missing_path, &missing_hash_run);
        let missing_error = journal.load_run(&missing_hash_run.run_id).unwrap_err();
        assert_eq!(missing_error.code(), "journal_corrupt");
        assert!(missing_path.exists());

        let mut malformed_hash_run = valid_run.clone();
        malformed_hash_run.execution_binding_hash = "not-a-hash".to_string();
        let malformed_path = run_path(&root, &malformed_hash_run.run_id);
        overwrite_run_file(&malformed_path, &malformed_hash_run);
        let malformed_error = journal.load_run(&malformed_hash_run.run_id).unwrap_err();
        assert_eq!(malformed_error.code(), "journal_corrupt");
        assert!(malformed_path.exists());
    }

    #[test]
    fn malformed_output_error_and_timestamp_fail_closed_and_remain_on_disk() {
        let root = temp_root("bad-terminal-fields");
        let journal = RunJournal::open(root.clone()).unwrap();

        let valid_failed = failed_run("request:bad-error", 1, 10, 1);
        let mut failed = valid_failed.clone();
        journal.store_run(&valid_failed).unwrap();
        failed.error = None;
        let failed_path = run_path(&root, &failed.run_id);
        overwrite_run_file(&failed_path, &failed);
        let failed_error = journal.load_run(&failed.run_id).unwrap_err();
        assert_eq!(failed_error.code(), "journal_corrupt");
        assert!(failed_path.exists());

        let mut timestamp = completed_run("request:bad-timestamp", 1, 20, 1);
        journal.store_run(&timestamp).unwrap();
        timestamp.retention_until_ms = Some(10);
        let timestamp_path = run_path(&root, &timestamp.run_id);
        overwrite_run_file(&timestamp_path, &timestamp);
        let timestamp_error = journal.load_run(&timestamp.run_id).unwrap_err();
        assert_eq!(timestamp_error.code(), "journal_corrupt");
        assert!(timestamp_path.exists());
    }

    #[test]
    fn malformed_event_bound_file_fails_closed_and_remains_on_disk() {
        let root = temp_root("bad-event-bound");
        let journal = RunJournal::open(root.clone()).unwrap();
        let valid_run = completed_run("request:bad-event-bound", 1, 10, 1);
        let mut run = valid_run.clone();
        journal.store_run(&valid_run).unwrap();
        run.offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        run.events[0].data = serde_json::json!({
            "schema": RUN_OUTPUT_TEXT_SCHEMA,
            "text": "x".repeat(MAX_EVENT_BYTES_LIMIT as usize),
        });
        let path = run_path(&root, &run.run_id);
        overwrite_run_file(&path, &run);

        let error = journal.load_run(&run.run_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
        assert!(path.exists());
    }

    #[test]
    fn expired_terminal_is_removed_on_open() {
        let root = temp_root("expired-prune-open");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = completed_run("request:expired-open", 1, 10, 1);
        let path = run_path(&root, &run.run_id);
        journal.store_run(&run).unwrap();

        let reopened = RunJournal::open(root.clone()).unwrap();
        assert!(!path.exists());
        assert!(reopened.load_run_if_present(&run.run_id).unwrap().is_none());
    }

    #[test]
    fn active_and_unexpired_terminal_runs_remain() {
        let root = temp_root("active-unexpired");
        let journal = RunJournal::open(root.clone()).unwrap();
        let active = prepared_run("request:active");
        let unexpired = completed_run(
            "request:unexpired",
            now_ms().saturating_sub(1),
            now_ms(),
            60,
        );
        let active_path = run_path(&root, &active.run_id);
        let unexpired_path = run_path(&root, &unexpired.run_id);
        journal.store_run(&active).unwrap();
        journal.store_run(&unexpired).unwrap();

        let reopened = RunJournal::open(root.clone()).unwrap();
        assert!(active_path.exists());
        assert!(unexpired_path.exists());
        assert_eq!(
            reopened.load_run(&active.run_id).unwrap().run_id,
            active.run_id
        );
        assert_eq!(
            reopened.load_run(&unexpired.run_id).unwrap().run_id,
            unexpired.run_id
        );
    }

    #[test]
    fn invalid_in_memory_run_cannot_be_stored_and_existing_file_stays_byte_identical() {
        let root = temp_root("store-invalid");
        let journal = RunJournal::open(root.clone()).unwrap();

        let valid = completed_run("request:store-invalid", 70, 100, 60);
        let path = run_path(&root, &valid.run_id);
        journal.store_run(&valid).unwrap();
        let original_bytes = file_bytes(&path);

        let mut invalid_chronology = valid.clone();
        invalid_chronology.updated_at_ms = invalid_chronology.created_at_ms.saturating_sub(1);
        let chronology_error = journal.store_run(&invalid_chronology).unwrap_err();
        assert_eq!(chronology_error.code(), "journal_corrupt");
        assert_eq!(file_bytes(&path), original_bytes);

        let mut invalid_state = prepared_run("request:store-invalid");
        invalid_state.run_id = valid.run_id.clone();
        invalid_state.runtime_binding.request_id = valid.runtime_binding.request_id.clone();
        invalid_state.backend_state = Some(serde_json::json!({"job_id": "bad"}));
        let state_error = journal.store_run(&invalid_state).unwrap_err();
        assert_eq!(state_error.code(), "journal_corrupt");
        assert_eq!(file_bytes(&path), original_bytes);

        let mut invalid_error = failed_run("request:store-invalid", 70, 100, 60);
        invalid_error.run_id = valid.run_id.clone();
        invalid_error.runtime_binding.request_id = valid.runtime_binding.request_id.clone();
        invalid_error.error.as_mut().unwrap().message = "x".repeat(MAX_RUN_ERROR_MESSAGE_BYTES + 1);
        let error_error = journal.store_run(&invalid_error).unwrap_err();
        assert_eq!(error_error.code(), "journal_corrupt");
        assert_eq!(file_bytes(&path), original_bytes);
    }

    #[test]
    fn delete_reloads_exact_current_file_and_keeps_malformed_or_unexpired_entries() {
        let root = temp_root("delete-toctou");
        let journal = RunJournal::open(root.clone()).unwrap();

        let expired = completed_run("request:delete-malformed", 1, 10, 1);
        let malformed_path = run_path(&root, &expired.run_id);
        journal.store_run(&expired).unwrap();
        let mut malformed = expired.clone();
        malformed.events[0].sequence = 2;
        overwrite_run_file(&malformed_path, &malformed);
        let malformed_error =
            delete_verified_run_file(&malformed_path, &root.join("runs"), &expired.run_id)
                .unwrap_err();
        assert_eq!(malformed_error.code(), "journal_corrupt");
        assert!(malformed_path.exists());

        let expired_unexpired = completed_run("request:delete-unexpired", 1, 10, 1);
        let unexpired_path = run_path(&root, &expired_unexpired.run_id);
        journal.store_run(&expired_unexpired).unwrap();
        let mut unexpired = expired_unexpired.clone();
        let terminal_at_ms = now_ms();
        unexpired.terminal_at_ms = Some(terminal_at_ms);
        unexpired.updated_at_ms = terminal_at_ms;
        unexpired.retention_until_ms =
            Some(terminal_at_ms.saturating_add(unexpired.offer.policy.retention_secs * 1_000));
        overwrite_run_file(&unexpired_path, &unexpired);
        let unexpired_error = delete_verified_run_file(
            &unexpired_path,
            &root.join("runs"),
            &expired_unexpired.run_id,
        )
        .unwrap_err();
        assert_eq!(unexpired_error.code(), "journal_corrupt");
        assert!(unexpired_path.exists());
    }

    #[test]
    fn lifecycle_and_error_bounds_accept_boundary_and_reject_boundary_plus_one() {
        let root = temp_root("lifecycle-error-bounds");
        let journal = RunJournal::open(root.clone()).unwrap();

        let mut valid = failed_run("request:lifecycle-boundary", 100, 100, 60);
        valid.created_at_ms = 100;
        valid.updated_at_ms = 100;
        valid.deadline_ms = 130;
        valid.terminal_at_ms = Some(100);
        valid.retention_until_ms = Some(60_100);
        valid.offer.policy.runtime_ms_limit = 30;
        valid.offer.policy.retention_secs = 60;
        valid.error.as_mut().unwrap().code = "c".repeat(MAX_RUN_ERROR_CODE_BYTES);
        valid.error.as_mut().unwrap().message = "m".repeat(MAX_RUN_ERROR_MESSAGE_BYTES);
        journal.store_run(&valid).unwrap();
        assert_eq!(
            journal.load_run(&valid.run_id).unwrap().run_id,
            valid.run_id
        );

        let mut invalid_deadline = valid.clone();
        invalid_deadline.deadline_ms = invalid_deadline.deadline_ms.saturating_add(1);
        let deadline_error = journal.store_run(&invalid_deadline).unwrap_err();
        assert_eq!(deadline_error.code(), "journal_corrupt");

        let mut invalid_error = valid.clone();
        invalid_error.error.as_mut().unwrap().message.push('x');
        let error_error = journal.store_run(&invalid_error).unwrap_err();
        assert_eq!(error_error.code(), "journal_corrupt");
    }

    #[test]
    #[cfg(not(unix))]
    fn non_unix_journal_behavior_is_explicit_and_safe() {
        let root = temp_root("non-unix");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("request:non-unix");
        journal.store_run(&run).unwrap();
        let loaded = journal.load_run(&run.run_id).unwrap();
        assert_eq!(loaded.run_id, run.run_id);
    }
}
