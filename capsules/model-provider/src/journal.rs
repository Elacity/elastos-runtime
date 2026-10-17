use crate::config::{
    MAX_BACKEND_COST_BYTES, MAX_BACKEND_COST_UNIT_BYTES, MAX_CANCEL_SETTLEMENT_TIMEOUT_MS,
    MAX_CONCURRENCY_LIMIT, MAX_EVENT_BYTES_LIMIT, MAX_HOSTED_POLICY_REF_BYTES,
    MAX_HOSTED_PROVIDER_LABEL_BYTES, MAX_INLINE_OUTPUT_BYTES_LIMIT, MAX_INPUT_BYTES_LIMIT,
    MAX_MODALITIES_PER_OFFER, MAX_MODALITY_BYTES, MAX_MODEL_BYTES, MAX_OFFER_ID_BYTES,
    MAX_OFFER_TITLE_BYTES, MAX_OPERATION_BYTES, MAX_RETENTION_SECS, MAX_RUNTIME_MS_LIMIT,
    MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT, MAX_RUN_EVENT_COUNT_LIMIT,
};
use crate::contract::{
    hex_hash, model_input_hash, validate_bounded_trimmed, validate_input_hash, validate_run_id,
    BackendCost, BackendFact, BackendReport, BackendTokenUsage, OfferSummary, ProviderFault,
    RunError, RunEvent, RunStatus, RunTerminalOutcome, RunView, RuntimeCreateBinding,
    BACKEND_REPORT_SCHEMA, HOSTED_PLACEMENT, HOSTED_SELECTION_PINNED, MAX_EVENT_SEQUENCE,
    MODEL_POLICY_SCHEMA, RUN_EVENT_SCHEMA, RUN_OUTPUT_CONTENT_SCHEMA, RUN_OUTPUT_OBJECT_SCHEMA,
    RUN_OUTPUT_TEXT_SCHEMA, SINGLE_DISPATCH_NO_RETRY, UPSTREAM_FALLBACK_OPERATOR_ASSERTED_DISABLED,
};
use elastos_model_contract::{
    MAX_RUNTIME_BINDING_ID_BYTES, MAX_RUNTIME_OPERATION_BYTES, RUNTIME_CREATE_BINDING_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RUN_JOURNAL_SCHEMA: &str = "elastos.model.provider-run-journal/v1";
pub const MAX_RUN_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_NEW_RUN_JOURNAL_BYTES: u64 = 1024 * 1024;
pub const MAX_RUN_HEADER_BYTES: u64 = 32 * 1024;
pub const MAX_RUN_BACKEND_STATE_BYTES: u64 = 4 * 1024;
pub const MAX_NEW_JOURNAL_ENTRIES: usize = 256;
pub const MAX_JOURNAL_GROWTH_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RECOVERY_JOURNAL_ENTRIES: usize = 512;
pub const MAX_RECOVERY_SCAN_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REQUEST_FLOOR_BYTES: u64 = 1024;
const REQUEST_FLOOR_SCHEMA: &str = "elastos.model.pruned-request-floor/v1";
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
                backend_report: self
                    .events
                    .last()
                    .filter(|event| event.terminal)
                    .and_then(|event| event.backend_report.clone()),
            }),
        }
    }
}

pub struct RunJournal {
    runs_dir: PathBuf,
    request_floor_path: PathBuf,
    recovery_dir: PathBuf,
    recovery_ack_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrunedRequestFloor {
    schema: String,
    issued_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryAcknowledgement {
    schema: String,
    archive_device: u64,
    archive_inode: u64,
}

struct JournalEntry {
    path: PathBuf,
    metadata: fs::Metadata,
    transaction: bool,
}

#[derive(Default)]
pub(crate) struct JournalUsage {
    entries: usize,
    charged_bytes: u64,
}

fn run_charge(run: &StoredRun, bytes: u64) -> u64 {
    // The complete compact record reserves the already bounded terminal tail.
    // Terminal facts, including Unknown, are frozen and keep their actual
    // bounded bytes. Every terminal identity has a bounded retention window.
    if !run.status.is_terminal() {
        bytes.max(MAX_NEW_RUN_JOURNAL_BYTES)
    } else {
        bytes
    }
}

fn same_execution_identity(old: &StoredRun, next: &StoredRun) -> bool {
    old.schema == next.schema
        && old.run_id == next.run_id
        && old.runtime_binding == next.runtime_binding
        && old.offer == next.offer
        && old.execution_binding_hash == next.execution_binding_hash
        && old.input_hash == next.input_hash
        && old.created_at_ms == next.created_at_ms
        && old.deadline_ms == next.deadline_ms
}

impl RunJournal {
    pub fn open(root: PathBuf) -> Result<Self, ProviderFault> {
        let runs_dir = root.join("runs");
        ensure_journal_dir_owner_only(&root)?;
        ensure_journal_dir_owner_only(&runs_dir)?;
        let journal = Self {
            runs_dir,
            request_floor_path: root.join("pruned-request-floor.json"),
            recovery_dir: root.join("recovery-runs"),
            recovery_ack_path: root.join("recovery-complete.json"),
        };
        journal.pruned_issued_floor_ms()?;
        journal.archive_recovery_pending()?;
        if journal.recovery_census()?.is_none() {
            journal.archive_legacy_runs()?;
        }
        journal.recover_abandoned_transactions()?;
        journal.prune_expired_terminal_runs()?;
        Ok(journal)
    }

    pub fn pruned_issued_floor_ms(&self) -> Result<u64, ProviderFault> {
        self.load_request_floor().map(|(floor, _)| floor)
    }

    fn load_request_floor(&self) -> Result<(u64, Option<fs::Metadata>), ProviderFault> {
        let parent = self.request_floor_path.parent().ok_or_else(|| {
            ProviderFault::corrupt_journal("model request floor is missing its parent")
        })?;
        ensure_journal_dir_owner_only(parent)?;
        let expected = match fs::symlink_metadata(&self.request_floor_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, None)),
            Err(error) => {
                return Err(ProviderFault::corrupt_journal(format!(
                    "failed to inspect model request floor: {error}"
                )))
            }
        };
        validate_existing_file(&self.request_floor_path, &expected)?;
        if expected.len() > MAX_REQUEST_FLOOR_BYTES {
            return Err(ProviderFault::corrupt_journal(
                "model request floor is too large",
            ));
        }
        let mut file = open_run_readonly(&self.request_floor_path)?;
        let opened = file.metadata().map_err(|error| {
            ProviderFault::corrupt_journal(format!(
                "failed to inspect opened request floor: {error}"
            ))
        })?;
        validate_existing_file(&self.request_floor_path, &opened)?;
        if !same_snapshot(&expected, &opened) {
            return Err(ProviderFault::corrupt_journal(
                "model request floor changed before read",
            ));
        }
        let bytes = read_bounded_record(&mut file, opened.len())?;
        let after = file.metadata().map_err(|error| {
            ProviderFault::corrupt_journal(format!(
                "failed to inspect request floor after read: {error}"
            ))
        })?;
        if !same_snapshot(&opened, &after) {
            return Err(ProviderFault::corrupt_journal(
                "model request floor changed during read",
            ));
        }
        verify_path_snapshot(&self.request_floor_path, &opened)?;
        let floor: PrunedRequestFloor = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderFault::corrupt_journal("model request floor is malformed"))?;
        if floor.schema != REQUEST_FLOOR_SCHEMA || floor.issued_ms == 0 {
            return Err(ProviderFault::corrupt_journal(
                "model request floor is invalid",
            ));
        }
        Ok((floor.issued_ms, Some(opened)))
    }

    fn record_pruned_issuance(&self, request_id: &str) -> Result<(), ProviderFault> {
        let Some(issued_ms) = elastos_model_contract::model_request_issued_ms(request_id) else {
            // Retained legacy IDs cannot authorize a new request.
            return Ok(());
        };
        self.record_pruned_issuance_ms(issued_ms)
    }

    fn record_pruned_issuance_ms(&self, issued_ms: u64) -> Result<(), ProviderFault> {
        #[cfg(unix)]
        let directories = lock_transaction_directories(&self.request_floor_path)?;
        self.record_pruned_issuance_ms_with_locks(
            issued_ms,
            #[cfg(unix)]
            &directories,
        )
    }

    fn record_pruned_issuance_ms_with_locks(
        &self,
        issued_ms: u64,
        #[cfg(unix)] directories: &[(PathBuf, fs::File)],
    ) -> Result<(), ProviderFault> {
        let (previous, expected) = self.load_request_floor()?;
        if issued_ms <= previous {
            return Ok(());
        }
        let bytes = encode_bounded(
            &PrunedRequestFloor {
                schema: REQUEST_FLOOR_SCHEMA.into(),
                issued_ms,
            },
            MAX_REQUEST_FLOOR_BYTES,
        )?;
        // One deterministic transaction per serialized writer. Init can recover
        // an unlocked partial write without trusting partial JSON or its PID.
        let temp = self.request_floor_path.with_file_name(format!(
            ".pruned-request-floor.json.tmp-{}",
            std::process::id()
        ));
        atomic_write_with_temp_with_locks(
            (&self.request_floor_path, temp),
            &bytes,
            expected.as_ref(),
            |file, bytes| {
                file.write_all(bytes)?;
                file.sync_all()
            },
            |from, to| fs::rename(from, to),
            sync_directory,
            #[cfg(unix)]
            directories,
        )
    }

    pub fn load_run(&self, run_id: &str) -> Result<StoredRun, ProviderFault> {
        validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
        let directory = if self.is_archived_run(run_id)? {
            &self.recovery_dir
        } else {
            &self.runs_dir
        };
        load_verified_run_file(&hashed_path(directory, run_id), directory, Some(run_id))
    }

    pub fn load_run_if_present(&self, run_id: &str) -> Result<Option<StoredRun>, ProviderFault> {
        validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
        let path = hashed_path(&self.runs_dir, run_id);
        match fs::symlink_metadata(&path) {
            Ok(_) => self.load_run(run_id).map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if self.is_archived_run(run_id)? {
                    self.load_run(run_id).map(Some)
                } else {
                    Ok(None)
                }
            }
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
        let census = self.census()?;
        let existing = census.iter().find(|entry| entry.path == path);
        let previous = if let Some(entry) = existing {
            let old = load_verified_run_snapshot(
                &path,
                &self.runs_dir,
                Some(&run.run_id),
                &entry.metadata,
            )?;
            if !same_execution_identity(&old, run) {
                return Err(ProviderFault::corrupt_journal(
                    "model run replacement changed its execution binding",
                ));
            }
            Some((run_charge(&old, entry.metadata.len()), old.status))
        } else {
            None
        };
        let recovery = previous.as_ref().is_some_and(|(_, status)| {
            (!status.is_terminal() && run.status.is_terminal())
                || (*status == RunStatus::SettlementUnknown
                    && run.status == RunStatus::SettlementUnknown)
        });
        let limit = match validate_new_run_shape(run) {
            Ok(()) => MAX_NEW_RUN_JOURNAL_BYTES,
            Err(_) if recovery => MAX_RUN_JOURNAL_BYTES,
            Err(fault) => return Err(fault),
        };
        let bytes = encode_bounded(run, limit)?;
        let charge = run_charge(run, bytes.len() as u64);
        if let Some((old_charge, _)) = previous {
            // Existing reservations permit streaming and settlement without
            // decoding every other retained payload on each worker update.
            if recovery && charge > old_charge {
                return Err(journal_budget_fault());
            }
            if charge > old_charge {
                let usage = self.visit_census(&census, |_, _, _| Ok(()))?;
                if usage.entries > MAX_NEW_JOURNAL_ENTRIES
                    || usage.charged_bytes - old_charge + charge > MAX_JOURNAL_GROWTH_BYTES
                {
                    return Err(journal_budget_fault());
                }
            }
            let actual_bytes: u64 = census.iter().map(|entry| entry.metadata.len()).sum();
            if actual_bytes - existing.unwrap().metadata.len() + bytes.len() as u64
                > MAX_RECOVERY_SCAN_BYTES
            {
                return Err(journal_budget_fault());
            }
        } else {
            let usage = self.visit_census(&census, |_, _, _| Ok(()))?;
            if usage.entries >= MAX_NEW_JOURNAL_ENTRIES
                || usage
                    .charged_bytes
                    .checked_add(charge)
                    .is_none_or(|total| total > MAX_JOURNAL_GROWTH_BYTES)
            {
                // This check precedes the durable Prepared record and dispatch.
                return Err(journal_budget_fault());
            }
        }
        self.verify_census(&census)?;
        let expected = existing.map(|entry| &entry.metadata);
        atomic_write_owner_only(&path, &bytes, expected)
    }

    pub fn active_run_count(&self, offer_id: &str) -> Result<usize, ProviderFault> {
        let mut total = 0usize;
        self.visit_runs(|_, run, _| {
            if run.offer.id == offer_id && !run.status.is_terminal() {
                total = total.saturating_add(1);
            }
            Ok(())
        })?;
        Ok(total)
    }

    fn floor_transactions(&self) -> Result<Vec<JournalEntry>, ProviderFault> {
        let root = self.request_floor_path.parent().unwrap();
        let mut transactions = Vec::new();
        for (count, entry) in fs::read_dir(root)
            .map_err(|_| ProviderFault::corrupt_journal("request floor directory is unreadable"))?
            .enumerate()
        {
            if count >= MAX_RECOVERY_JOURNAL_ENTRIES {
                return Err(ProviderFault::corrupt_journal(
                    "request floor directory entry limit exceeded",
                ));
            }
            let path = entry
                .map_err(|_| ProviderFault::corrupt_journal("request floor census failed"))?
                .path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    ProviderFault::corrupt_journal("request floor entry name is invalid")
                })?;
            if name.starts_with(".pruned-request-floor.json.tmp")
                || name.starts_with(".recovery-complete.json.tmp")
            {
                let metadata = fs::symlink_metadata(&path).map_err(|_| {
                    ProviderFault::corrupt_journal("request floor transaction is unreadable")
                })?;
                validate_existing_file(&path, &metadata)?;
                if metadata.len() > MAX_REQUEST_FLOOR_BYTES
                    || transaction_creator(&path, true).is_none()
                {
                    return Err(ProviderFault::corrupt_journal(
                        "request floor transaction is invalid",
                    ));
                }
                transactions.push(JournalEntry {
                    path,
                    metadata,
                    transaction: true,
                });
            }
        }
        Ok(transactions)
    }

    fn recover_abandoned_transactions(&self) -> Result<(), ProviderFault> {
        self.recover_abandoned_transactions_with(|| Ok(()))
    }

    // Init and atomic writes share the same protected directory locks. A PID
    // in a crash filename does not prove that a writer still owns the file.
    // No pathname or partial payload grants deletion of a committed binding.
    fn recover_abandoned_transactions_with(
        &self,
        before_remove: impl FnOnce() -> Result<(), ProviderFault>,
    ) -> Result<(), ProviderFault> {
        let root = self.request_floor_path.parent().unwrap();
        let root_snapshot = fs::symlink_metadata(root)
            .map_err(|_| ProviderFault::corrupt_journal("journal root is unreadable"))?;
        let runs_snapshot = fs::symlink_metadata(&self.runs_dir)
            .map_err(|_| ProviderFault::corrupt_journal("journal runs directory is unreadable"))?;
        #[cfg(not(unix))]
        if !self.census()?.iter().any(|entry| entry.transaction)
            && self.floor_transactions()?.is_empty()
        {
            return Ok(());
        }
        let root_dir = open_journal_directory(root)?;
        lock_journal_directory(&root_dir)?;
        let runs_dir = open_journal_directory(&self.runs_dir)?;
        lock_journal_directory(&runs_dir)?;
        if !same_snapshot(
            &root_snapshot,
            &root_dir
                .metadata()
                .map_err(|_| ProviderFault::corrupt_journal("opened journal root is unreadable"))?,
        ) || !same_snapshot(
            &runs_snapshot,
            &runs_dir.metadata().map_err(|_| {
                ProviderFault::corrupt_journal("opened journal directory is unreadable")
            })?,
        ) {
            return Err(ProviderFault::corrupt_journal(
                "journal directory changed before recovery",
            ));
        }
        let census = self.census()?;
        let floor_transactions = self.floor_transactions()?;
        if !census.iter().any(|entry| entry.transaction) && floor_transactions.is_empty() {
            return Ok(());
        }
        let (floor, floor_snapshot) = self.load_request_floor()?;
        let mut transactions = Vec::new();
        let mut actual_bytes = census.iter().map(|entry| entry.metadata.len()).sum::<u64>();
        if census.len() + floor_transactions.len() > MAX_RECOVERY_JOURNAL_ENTRIES {
            return Err(ProviderFault::corrupt_journal(
                "journal transaction recovery entry limit exceeded",
            ));
        }
        for entry in census
            .iter()
            .filter(|entry| entry.transaction)
            .chain(&floor_transactions)
        {
            let floor_temp = entry.path.parent() == Some(root);
            transaction_creator(&entry.path, floor_temp).ok_or_else(|| {
                ProviderFault::corrupt_journal("journal transaction name is invalid")
            })?;
            if floor_temp {
                actual_bytes += entry.metadata.len();
            }
            if actual_bytes > MAX_RECOVERY_SCAN_BYTES {
                return Err(ProviderFault::corrupt_journal(
                    "journal transaction recovery byte limit exceeded",
                ));
            }
            let held = open_run_readonly(&entry.path)?;
            let opened = held
                .metadata()
                .map_err(|_| ProviderFault::corrupt_journal("journal transaction is unreadable"))?;
            validate_existing_file(&entry.path, &opened)?;
            if !same_snapshot(&entry.metadata, &opened) {
                return Err(ProviderFault::corrupt_journal(
                    "journal transaction changed before recovery",
                ));
            }
            verify_path_snapshot(&entry.path, &opened)?;
            transactions.push(entry);
        }
        // Every committed record and every candidate is verified before the
        // first mutation. A bad later record preserves all crash transactions.
        self.visit_census(&census, |_, _, _| Ok(()))?;
        before_remove()?;
        self.verify_census(&census)?;
        let (current_floor, current_snapshot) = self.load_request_floor()?;
        if current_floor != floor
            || !same_optional_snapshot(floor_snapshot.as_ref(), current_snapshot.as_ref())
        {
            return Err(ProviderFault::corrupt_journal(
                "committed request floor changed during recovery",
            ));
        }
        let current_transactions = self.floor_transactions()?;
        if current_transactions.len() != floor_transactions.len()
            || floor_transactions.iter().any(|old| {
                current_transactions
                    .iter()
                    .find(|next| old.path == next.path)
                    .is_none_or(|next| !same_snapshot(&old.metadata, &next.metadata))
            })
        {
            return Err(ProviderFault::corrupt_journal(
                "request floor transactions changed during recovery",
            ));
        }
        for entry in transactions {
            verify_journal_directory(root, &root_dir)?;
            verify_journal_directory(&self.runs_dir, &runs_dir)?;
            let parent = if entry.path.parent() == Some(root) {
                &root_dir
            } else {
                &runs_dir
            };
            delete_verified_transaction(&entry.path, &entry.metadata, parent)?;
        }
        if !floor_transactions.is_empty() {
            root_dir.sync_all().map_err(|_| {
                ProviderFault::corrupt_journal("request floor recovery sync failed")
            })?;
        }
        runs_dir
            .sync_all()
            .map_err(|_| ProviderFault::corrupt_journal("run transaction recovery sync failed"))?;
        Ok(())
    }

    pub fn prune_expired_terminal_runs(&self) -> Result<(), ProviderFault> {
        self.pruned_issued_floor_ms()?;
        let mut expired = Vec::new();
        self.visit_runs(|path, run, _| {
            if is_expired_terminal_run(run, now_ms()) {
                expired.push((path.to_path_buf(), run.run_id.clone()));
            }
            Ok(())
        })?;
        // Validate the entire bounded journal before any deletion.
        for (path, run_id) in expired {
            delete_verified_run_file(&path, &self.runs_dir, &run_id, |run| {
                self.record_pruned_issuance(&run.runtime_binding.request_id)
            })?;
        }
        Ok(())
    }

    pub fn prune_expired_loaded_run(&self, run: &StoredRun) -> Result<bool, ProviderFault> {
        // Recovery facts remain owner-readable. They never authorize dispatch.
        if self.is_archived_run(&run.run_id)? {
            return Ok(false);
        }
        if !is_expired_terminal_run(run, now_ms()) {
            return Ok(false);
        }
        self.pruned_issued_floor_ms()?;
        let path = hashed_path(&self.runs_dir, &run.run_id);
        delete_verified_run_file(&path, &self.runs_dir, &run.run_id, |run| {
            self.record_pruned_issuance(&run.runtime_binding.request_id)
        })?;
        Ok(true)
    }

    fn census(&self) -> Result<Vec<JournalEntry>, ProviderFault> {
        self.recovery_census()?.ok_or_else(|| {
            ProviderFault::corrupt_journal("model run journal recovery limit exceeded")
        })
    }

    fn recovery_census(&self) -> Result<Option<Vec<JournalEntry>>, ProviderFault> {
        ensure_journal_dir_owner_only(&self.runs_dir)?;
        let entries = fs::read_dir(&self.runs_dir).map_err(|err| {
            ProviderFault::corrupt_journal(format!(
                "failed to scan model run journal {}: {err}",
                self.runs_dir.display()
            ))
        })?;
        let mut census = Vec::new();
        let mut actual_bytes = 0u64;
        for entry in entries {
            let entry = entry.map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to scan model run journal {}: {err}",
                    self.runs_dir.display()
                ))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to inspect model run journal: {err}"
                ))
            })?;
            validate_existing_file(&path, &metadata)?;
            actual_bytes = actual_bytes.checked_add(metadata.len()).ok_or_else(|| {
                ProviderFault::corrupt_journal("model run journal recovery byte limit exceeded")
            })?;
            if metadata.len() > MAX_RUN_JOURNAL_BYTES {
                return Err(ProviderFault::corrupt_journal(
                    "model run journal recovery byte limit exceeded",
                ));
            }
            if census.len() == MAX_RECOVERY_JOURNAL_ENTRIES
                || actual_bytes > MAX_RECOVERY_SCAN_BYTES
            {
                // The protected archive is not execution authority. Unseen
                // legacy files are validated only on bounded exact lookup.
                return Ok(None);
            }
            census.push(JournalEntry {
                transaction: is_transaction_temp_file(&path),
                path,
                metadata,
            });
        }
        Ok(Some(census))
    }

    pub(crate) fn is_archived_run(&self, run_id: &str) -> Result<bool, ProviderFault> {
        validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
        match fs::symlink_metadata(hashed_path(&self.runs_dir, run_id)) {
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ProviderFault::corrupt_journal("model run is unreadable")),
        }
        match fs::symlink_metadata(&self.recovery_dir) {
            Ok(metadata) => validate_existing_directory(&self.recovery_dir, &metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => {
                return Err(ProviderFault::corrupt_journal(
                    "recovery archive is unreadable",
                ))
            }
        }
        match fs::symlink_metadata(hashed_path(&self.recovery_dir, run_id)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ProviderFault::corrupt_journal("recovery run is unreadable")),
        }
    }

    pub(crate) fn archive_recovery_pending(&self) -> Result<bool, ProviderFault> {
        let archive = match fs::symlink_metadata(&self.recovery_dir) {
            Ok(metadata) => {
                validate_existing_directory(&self.recovery_dir, &metadata)?;
                metadata
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => {
                return Err(ProviderFault::corrupt_journal(
                    "recovery archive is unreadable",
                ))
            }
        };
        let archive_handle = open_journal_directory(&self.recovery_dir)?;
        if archive_identity(&archive)?
            != archive_identity(
                &archive_handle.metadata().map_err(|_| {
                    ProviderFault::corrupt_journal("recovery archive is unreadable")
                })?,
            )?
        {
            return Err(ProviderFault::corrupt_journal(
                "recovery archive changed before acknowledgement",
            ));
        }
        match fs::symlink_metadata(&self.recovery_ack_path) {
            Ok(metadata) => {
                validate_existing_file(&self.recovery_ack_path, &metadata)?;
                if metadata.len() > MAX_REQUEST_FLOOR_BYTES {
                    return Err(ProviderFault::corrupt_journal(
                        "recovery acknowledgement is invalid",
                    ));
                }
                let mut file = open_run_readonly(&self.recovery_ack_path)?;
                let opened = file.metadata().map_err(|_| {
                    ProviderFault::corrupt_journal("recovery acknowledgement is unreadable")
                })?;
                validate_existing_file(&self.recovery_ack_path, &opened)?;
                if !same_snapshot(&metadata, &opened) {
                    return Err(ProviderFault::corrupt_journal(
                        "recovery acknowledgement changed before read",
                    ));
                }
                let bytes = read_bounded_record(&mut file, metadata.len())?;
                let after = file.metadata().map_err(|_| {
                    ProviderFault::corrupt_journal("recovery acknowledgement is unreadable")
                })?;
                if !same_snapshot(&opened, &after) {
                    return Err(ProviderFault::corrupt_journal(
                        "recovery acknowledgement changed during read",
                    ));
                }
                verify_path_snapshot(&self.recovery_ack_path, &metadata)?;
                let ack: RecoveryAcknowledgement =
                    serde_json::from_slice(&bytes).map_err(|_| {
                        ProviderFault::corrupt_journal("recovery acknowledgement is invalid")
                    })?;
                if ack.schema != "elastos.model.recovery-complete/v1" {
                    return Err(ProviderFault::corrupt_journal(
                        "recovery acknowledgement is invalid",
                    ));
                }
                verify_journal_directory(&self.recovery_dir, &archive_handle)?;
                Ok((ack.archive_device, ack.archive_inode) != archive_identity(&archive)?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(_) => Err(ProviderFault::corrupt_journal(
                "recovery acknowledgement is unreadable",
            )),
        }
    }

    pub(crate) fn acknowledge_archive_recovery(&self) -> Result<(), ProviderFault> {
        if self.archive_recovery_pending()? {
            let archive = fs::symlink_metadata(&self.recovery_dir)
                .map_err(|_| ProviderFault::corrupt_journal("recovery archive is unreadable"))?;
            validate_existing_directory(&self.recovery_dir, &archive)?;
            let (archive_device, archive_inode) = archive_identity(&archive)?;
            let bytes = encode_bounded(
                &RecoveryAcknowledgement {
                    schema: "elastos.model.recovery-complete/v1".into(),
                    archive_device,
                    archive_inode,
                },
                MAX_REQUEST_FLOOR_BYTES,
            )?;
            let expected = match fs::symlink_metadata(&self.recovery_ack_path) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => {
                    return Err(ProviderFault::corrupt_journal(
                        "recovery acknowledgement is unreadable",
                    ))
                }
            };
            let held = open_journal_directory(&self.recovery_dir)?;
            verify_journal_directory(&self.recovery_dir, &held)?;
            atomic_write_with_temp(
                &self.recovery_ack_path,
                &bytes,
                expected.as_ref(),
                self.recovery_ack_path.with_file_name(format!(
                    ".recovery-complete.json.tmp-{}",
                    std::process::id()
                )),
                |file, bytes| {
                    file.write_all(bytes)?;
                    file.sync_all()
                },
                |from, to| fs::rename(from, to),
                sync_directory,
            )?;
            verify_journal_directory(&self.recovery_dir, &held)?;
        }
        Ok(())
    }

    fn archive_legacy_runs(&self) -> Result<(), ProviderFault> {
        let root = self.runs_dir.parent().unwrap();
        let root_handle = open_journal_directory(root)?;
        lock_journal_directory(&root_handle)?;
        let runs_handle = open_journal_directory(&self.runs_dir)?;
        lock_journal_directory(&runs_handle)?;
        let directories = [
            (root.to_path_buf(), root_handle),
            (self.runs_dir.clone(), runs_handle),
        ];
        let root_handle = &directories[0].1;
        let runs_handle = &directories[1].1;
        for path in [&self.recovery_dir, &self.recovery_ack_path] {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(ProviderFault::corrupt_journal(
                        "protected recovery archive already exists",
                    ))
                }
            }
        }
        // The floor protects clock rollback; direct archive lookup preserves
        // every retained legacy and future-issued identity before validation.
        self.record_pruned_issuance_ms_with_locks(
            now_ms().max(1),
            #[cfg(unix)]
            &directories,
        )?;
        verify_journal_directory(root, root_handle)?;
        verify_journal_directory(&self.runs_dir, runs_handle)?;
        rename_directory_no_replace(&self.runs_dir, &self.recovery_dir)
            .map_err(|_| ProviderFault::corrupt_journal("legacy journal archive failed"))?;
        verify_journal_directory(&self.recovery_dir, runs_handle)?;
        root_handle
            .sync_all()
            .map_err(|_| ProviderFault::corrupt_journal("legacy journal archive sync failed"))?;
        ensure_journal_dir_owner_only(&self.runs_dir)?;
        sync_directory(root)?;
        verify_journal_directory(root, root_handle)?;
        verify_journal_directory(&self.recovery_dir, runs_handle)
    }

    pub(crate) fn visit_runs(
        &self,
        visit: impl FnMut(&Path, &StoredRun, u64) -> Result<(), ProviderFault>,
    ) -> Result<JournalUsage, ProviderFault> {
        // Visitors collect bounded summaries. Mutation follows full success.
        self.visit_census(&self.census()?, visit)
    }

    fn visit_census(
        &self,
        census: &[JournalEntry],
        mut visit: impl FnMut(&Path, &StoredRun, u64) -> Result<(), ProviderFault>,
    ) -> Result<JournalUsage, ProviderFault> {
        let mut usage = JournalUsage {
            entries: census.len(),
            ..JournalUsage::default()
        };
        for entry in census {
            if entry.transaction {
                // Crash transactions consume capacity but cannot authorize a run.
                verify_path_snapshot(&entry.path, &entry.metadata)?;
                usage.charged_bytes += entry.metadata.len();
                continue;
            }
            let run =
                load_verified_run_snapshot(&entry.path, &self.runs_dir, None, &entry.metadata)?;
            usage.charged_bytes = usage
                .charged_bytes
                .checked_add(run_charge(&run, entry.metadata.len()))
                .ok_or_else(|| {
                    ProviderFault::corrupt_journal("model run journal charge overflow")
                })?;
            visit(&entry.path, &run, entry.metadata.len())?;
        }
        self.verify_census(census)?;
        Ok(usage)
    }
    fn verify_census(&self, census: &[JournalEntry]) -> Result<(), ProviderFault> {
        let current = self.census()?;
        if current.len() != census.len()
            || census.iter().any(|old| {
                current
                    .iter()
                    .find(|entry| entry.path == old.path)
                    .is_none_or(|entry| !same_snapshot(&old.metadata, &entry.metadata))
            })
        {
            return Err(ProviderFault::corrupt_journal(
                "model run journal census changed during scan",
            ));
        }
        Ok(())
    }
}

fn journal_budget_fault() -> ProviderFault {
    ProviderFault::policy_limit(
        "Model run history is full. Retained outcomes expire after their history window; then try again.",
        "model run journal admission or growth limit exceeded",
    )
}

struct BoundedEncoding {
    bytes: Vec<u8>,
    limit: u64,
}
impl Write for BoundedEncoding {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if (self.bytes.len() as u64)
            .checked_add(bytes.len() as u64)
            .is_none_or(|total| total > self.limit)
        {
            return Err(std::io::Error::other(
                "model run journal encoding limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode_bounded(value: &impl Serialize, limit: u64) -> Result<Vec<u8>, ProviderFault> {
    let mut writer = BoundedEncoding {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| journal_budget_fault())?;
    Ok(writer.bytes)
}

fn validate_new_run_shape(run: &StoredRun) -> Result<(), ProviderFault> {
    #[derive(Serialize)]
    struct Header<'a> {
        schema: &'a str,
        run_id: &'a str,
        runtime_binding: &'a RuntimeCreateBinding,
        offer: &'a OfferSummary,
        execution_binding_hash: &'a str,
        input_hash: &'a str,
        status: &'a RunStatus,
        next_sequence: u64,
        created_at_ms: u64,
        updated_at_ms: u64,
        deadline_ms: u64,
        terminal_at_ms: Option<u64>,
        retention_until_ms: Option<u64>,
        error: &'a Option<RunError>,
    }
    encode_bounded(
        &Header {
            schema: &run.schema,
            run_id: &run.run_id,
            runtime_binding: &run.runtime_binding,
            offer: &run.offer,
            execution_binding_hash: &run.execution_binding_hash,
            input_hash: &run.input_hash,
            status: &run.status,
            next_sequence: run.next_sequence,
            created_at_ms: run.created_at_ms,
            updated_at_ms: run.updated_at_ms,
            deadline_ms: run.deadline_ms,
            terminal_at_ms: run.terminal_at_ms,
            retention_until_ms: run.retention_until_ms,
            error: &run.error,
        },
        MAX_RUN_HEADER_BYTES,
    )?;
    if let Some(state) = &run.backend_state {
        encode_bounded(state, MAX_RUN_BACKEND_STATE_BYTES)?;
    }
    Ok(())
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
    elastos_model_contract::model_run_id(binding)
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

struct OwnedTransaction {
    path: PathBuf,
    file: fs::File,
    committed: bool,
}

impl Drop for OwnedTransaction {
    fn drop(&mut self) {
        if !self.committed {
            if let Ok(metadata) = self.file.metadata() {
                if verify_path_snapshot(&self.path, &metadata).is_ok() {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }
}

fn atomic_write_owner_only(
    path: &Path,
    bytes: &[u8],
    expected: Option<&fs::Metadata>,
) -> Result<(), ProviderFault> {
    atomic_write_with(
        path,
        bytes,
        expected,
        |file, bytes| {
            file.write_all(bytes)?;
            file.sync_all()
        },
        |from, to| fs::rename(from, to),
        sync_directory,
    )
}

fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    expected: Option<&fs::Metadata>,
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    sync_parent: impl FnOnce(&Path) -> Result<(), ProviderFault>,
) -> Result<(), ProviderFault> {
    atomic_write_with_temp(
        path,
        bytes,
        expected,
        allocate_temp_path(path)?,
        write,
        rename,
        sync_parent,
    )
}

fn atomic_write_with_temp(
    path: &Path,
    bytes: &[u8],
    expected: Option<&fs::Metadata>,
    tmp: PathBuf,
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    sync_parent: impl FnOnce(&Path) -> Result<(), ProviderFault>,
) -> Result<(), ProviderFault> {
    if let Some(parent) = path.parent() {
        ensure_journal_dir_owner_only(parent)?;
    }
    validate_run_path_target(path)?;
    #[cfg(unix)]
    let directories = lock_transaction_directories(path)?;
    atomic_write_with_temp_with_locks(
        (path, tmp),
        bytes,
        expected,
        write,
        rename,
        sync_parent,
        #[cfg(unix)]
        &directories,
    )
}

fn atomic_write_with_temp_with_locks(
    paths: (&Path, PathBuf),
    bytes: &[u8],
    expected: Option<&fs::Metadata>,
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    sync_parent: impl FnOnce(&Path) -> Result<(), ProviderFault>,
    #[cfg(unix)] directories: &[(PathBuf, fs::File)],
) -> Result<(), ProviderFault> {
    let (path, tmp) = paths;
    if let Some(parent) = path.parent() {
        ensure_journal_dir_owner_only(parent)?;
    }
    validate_run_path_target(path)?;
    #[cfg(unix)]
    verify_transaction_directories(directories)?;
    let file = open_temp_file_owner_only(&tmp)?;
    let mut transaction = OwnedTransaction {
        path: tmp,
        file,
        committed: false,
    };
    // Keep the created file handle until rename/cleanup; pathname replacement
    // never gives this transaction authority over the replacement inode.
    validate_existing_file(
        &transaction.path,
        &transaction.file.metadata().map_err(|err| {
            ProviderFault::corrupt_journal(format!("failed to inspect temporary journal: {err}"))
        })?,
    )?;
    write(&mut transaction.file, bytes).map_err(|err| {
        ProviderFault::corrupt_journal(format!("failed to write or sync temporary journal: {err}"))
    })?;
    #[cfg(unix)]
    verify_transaction_directories(directories)?;
    verify_path_snapshot(
        &transaction.path,
        &transaction.file.metadata().map_err(|err| {
            ProviderFault::corrupt_journal(format!("failed to inspect temporary journal: {err}"))
        })?,
    )?;
    match expected {
        Some(metadata) => verify_path_snapshot(path, metadata)?,
        None => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(ProviderFault::corrupt_journal(
                    "new model run journal target appeared",
                ))
            }
        },
    }
    rename(&transaction.path, path).map_err(|err| {
        ProviderFault::corrupt_journal(format!(
            "failed to replace provider journal {}: {err}",
            path.display()
        ))
    })?;
    transaction.committed = true;
    sync_parent(
        path.parent()
            .ok_or_else(|| ProviderFault::corrupt_journal("run journal file missing parent"))?,
    )?;
    #[cfg(unix)]
    verify_transaction_directories(directories)?;
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
        if err.kind() == std::io::ErrorKind::NotFound
            && expected_run_id.is_some()
            && fs::symlink_metadata(runs_dir)
                .is_ok_and(|metadata| validate_existing_directory(runs_dir, &metadata).is_ok())
        {
            return ProviderFault::unauthorized_run_access();
        }
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
    load_verified_run_snapshot(path, runs_dir, expected_run_id, &metadata)
}

fn same_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
}

fn verify_path_snapshot(path: &Path, expected: &fs::Metadata) -> Result<(), ProviderFault> {
    let current = fs::symlink_metadata(path).map_err(|err| {
        ProviderFault::corrupt_journal(format!("model run journal changed during access: {err}"))
    })?;
    validate_existing_file(path, &current)?;
    if !same_snapshot(expected, &current) {
        return Err(ProviderFault::corrupt_journal(
            "model run journal changed during access",
        ));
    }
    Ok(())
}

fn open_run_readonly(path: &Path) -> Result<fs::File, ProviderFault> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    options.open(path).map_err(|err| {
        ProviderFault::corrupt_journal(format!("failed to open model run journal: {err}"))
    })
}

fn load_verified_run_snapshot(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: Option<&str>,
    metadata: &fs::Metadata,
) -> Result<StoredRun, ProviderFault> {
    load_verified_run_snapshot_with_reader(
        path,
        runs_dir,
        expected_run_id,
        metadata,
        read_bounded_record,
    )
}

fn read_bounded_record(file: &mut fs::File, limit: u64) -> Result<Vec<u8>, ProviderFault> {
    let mut bytes = Vec::new();
    Read::by_ref(file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| {
            ProviderFault::corrupt_journal(format!("failed to read model run journal: {err}"))
        })?;
    if bytes.len() as u64 != limit {
        return Err(ProviderFault::corrupt_journal(
            "model run journal changed during read",
        ));
    }
    Ok(bytes)
}

fn load_verified_run_snapshot_with_reader(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: Option<&str>,
    metadata: &fs::Metadata,
    read: impl FnOnce(&mut fs::File, u64) -> Result<Vec<u8>, ProviderFault>,
) -> Result<StoredRun, ProviderFault> {
    let mut file = open_run_readonly(path)?;
    let opened = file.metadata().map_err(|err| {
        ProviderFault::corrupt_journal(format!("failed to inspect opened model run journal: {err}"))
    })?;
    validate_existing_file(path, &opened)?;
    if opened.len() > MAX_RUN_JOURNAL_BYTES || !same_snapshot(metadata, &opened) {
        return Err(ProviderFault::corrupt_journal(
            "model run journal changed before read",
        ));
    }
    let bytes = read(&mut file, opened.len())?;
    if bytes.len() as u64 != opened.len()
        || !same_snapshot(
            &opened,
            &file.metadata().map_err(|err| {
                ProviderFault::corrupt_journal(format!(
                    "failed to inspect opened model run journal: {err}"
                ))
            })?,
        )
    {
        return Err(ProviderFault::corrupt_journal(
            "model run journal changed during read",
        ));
    }
    verify_path_snapshot(path, &opened)?;
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
    if let Some(context) = &offer.context {
        if context.context_window_tokens < 128
            || context.context_window_tokens > crate::config::MAX_LOCAL_LLAMA_CONTEXT_SIZE
            || context.max_output_tokens == 0
            || context.max_output_tokens >= context.context_window_tokens
        {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal context limit is invalid at {}",
                path.display()
            )));
        }
    }
    if let Some(hosted) = offer.hosted.as_ref() {
        if hosted.placement != HOSTED_PLACEMENT
            || hosted.selection_mode != HOSTED_SELECTION_PINNED
            || hosted.provider_request_policy != SINGLE_DISPATCH_NO_RETRY
            || hosted.upstream_routing_fallback_assertion
                != UPSTREAM_FALLBACK_OPERATOR_ASSERTED_DISABLED
        {
            return Err(ProviderFault::corrupt_journal(format!(
                "model run journal hosted disclosure is invalid at {}",
                path.display()
            )));
        }
        for (value, label, max) in [
            (
                hosted.backend_provider_label.as_str(),
                "backend_provider_label",
                MAX_HOSTED_PROVIDER_LABEL_BYTES,
            ),
            (
                hosted.requested_selector.as_str(),
                "requested_selector",
                MAX_MODEL_BYTES,
            ),
            (
                hosted.privacy_policy_ref.as_str(),
                "privacy_policy_ref",
                MAX_HOSTED_POLICY_REF_BYTES,
            ),
            (
                hosted.terms_ref.as_str(),
                "terms_ref",
                MAX_HOSTED_POLICY_REF_BYTES,
            ),
        ] {
            validate_bounded_trimmed(value, label, max).map_err(|_| {
                ProviderFault::corrupt_journal(format!(
                    "model run journal hosted {label} is invalid at {}",
                    path.display()
                ))
            })?;
        }
    }
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
        match (
            &event.backend_report,
            event.terminal,
            run.offer.hosted.is_some(),
        ) {
            (Some(report), true, true) => validate_backend_report(report).map_err(|_| {
                ProviderFault::corrupt_journal(format!(
                    "model run journal backend report is invalid at {}",
                    path.display()
                ))
            })?,
            (None, true, true) | (Some(_), false, _) | (Some(_), true, false) => {
                return Err(ProviderFault::corrupt_journal(format!(
                    "model run journal backend report placement is invalid at {}",
                    path.display()
                )))
            }
            (None, _, _) => {}
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

pub(crate) fn validate_backend_report(report: &BackendReport) -> anyhow::Result<()> {
    if report.schema != BACKEND_REPORT_SCHEMA {
        anyhow::bail!("backend report schema is invalid");
    }
    if let BackendFact::Reported { value } = &report.resolved_model {
        validate_bounded_trimmed(value, "resolved_model", MAX_MODEL_BYTES)?;
    }
    if let BackendFact::Reported { value } = &report.usage {
        validate_backend_usage(value)?;
    }
    if let BackendFact::Reported { value } = &report.cost {
        validate_backend_cost(value)?;
    }
    Ok(())
}

fn validate_backend_usage(usage: &BackendTokenUsage) -> anyhow::Result<()> {
    if usage.input_tokens.is_none() && usage.output_tokens.is_none() && usage.total_tokens.is_none()
    {
        anyhow::bail!("reported backend usage is empty");
    }
    Ok(())
}

fn validate_backend_cost(cost: &BackendCost) -> anyhow::Result<()> {
    validate_bounded_trimmed(&cost.value, "cost", MAX_BACKEND_COST_BYTES)?;
    if cost.value.starts_with('-') {
        anyhow::bail!("reported backend cost is negative");
    }
    let parsed = serde_json::from_str::<Value>(&cost.value)?;
    let Value::Number(number) = parsed else {
        anyhow::bail!("reported backend cost is not a number");
    };
    if number.to_string() != cost.value {
        anyhow::bail!("reported backend cost is not canonical");
    }
    if let Some(unit) = cost.unit.as_deref() {
        validate_bounded_trimmed(unit, "cost unit", MAX_BACKEND_COST_UNIT_BYTES)?;
    }
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
        RUN_OUTPUT_TEXT_SCHEMA
            | RUN_OUTPUT_OBJECT_SCHEMA
            | RUN_OUTPUT_CONTENT_SCHEMA
            | elastos_model_contract::decisions::OUTPUT_SCHEMA
    ) {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal output schema is invalid at {}",
            path.display()
        )));
    }
    if (schema == elastos_model_contract::decisions::OUTPUT_SCHEMA)
        != (run.offer.operation == elastos_model_contract::decisions::OPERATION)
    {
        return Err(ProviderFault::corrupt_journal(
            "decision operation/output schema mismatch",
        ));
    }
    if schema == elastos_model_contract::decisions::OUTPUT_SCHEMA {
        let decision: elastos_model_contract::decisions::Output =
            serde_json::from_value(output.clone())
                .map_err(|_| ProviderFault::corrupt_journal("invalid stored decision output"))?;
        decision
            .validate()
            .map_err(|_| ProviderFault::corrupt_journal("invalid stored decision answers"))?;
        if run
            .offer
            .hosted
            .as_ref()
            .map(|hosted| hosted.requested_selector.as_str())
            != Some(decision.model.as_str())
        {
            return Err(ProviderFault::corrupt_journal(
                "stored decision model differs from selected model",
            ));
        }
    }
    Ok(())
}

fn is_expired_terminal_run(run: &StoredRun, now_ms: u64) -> bool {
    run.status.is_terminal()
        // Unknown remains truthful while retained. Expiry retires history,
        // never proves settlement, and commits the replay floor first.
        && run
            .retention_until_ms
            .is_some_and(|retention_until_ms| {
                let replay_deadline = elastos_model_contract::model_request_replay_deadline_ms(
                    &run.runtime_binding.request_id,
                ).unwrap_or(0);
                now_ms > retention_until_ms.max(replay_deadline)
            })
}

fn delete_verified_run_file(
    path: &Path,
    runs_dir: &Path,
    expected_run_id: &str,
    before_remove: impl FnOnce(&StoredRun) -> Result<(), ProviderFault>,
) -> Result<(), ProviderFault> {
    // Keep the verified inode open until removal; a replacement path never
    // inherits the expired record's deletion authority.
    let held = open_run_readonly(path)?;
    let snapshot = held.metadata().map_err(|err| {
        ProviderFault::corrupt_journal(format!("failed to inspect expiring run journal: {err}"))
    })?;
    validate_existing_file(path, &snapshot)?;
    let run = load_verified_run_snapshot(path, runs_dir, Some(expected_run_id), &snapshot)?;
    if !is_expired_terminal_run(&run, now_ms()) {
        return Err(ProviderFault::corrupt_journal(format!(
            "model run journal is not expired at {}",
            path.display()
        )));
    }
    // Commit the refusal floor before deleting the sole retained identity.
    before_remove(&run)?;
    let current = held.metadata().map_err(|err| {
        ProviderFault::corrupt_journal(format!("failed to inspect expiring run journal: {err}"))
    })?;
    if !same_snapshot(&snapshot, &current) {
        return Err(ProviderFault::corrupt_journal(
            "model run journal changed before expiry removal",
        ));
    }
    verify_path_snapshot(path, &snapshot)?;
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

fn transaction_creator(path: &Path, floor: bool) -> Option<i32> {
    let name = path.file_name()?.to_str()?;
    let suffix = if floor {
        name.strip_prefix(".pruned-request-floor.json.tmp-")
            .or_else(|| name.strip_prefix(".recovery-complete.json.tmp-"))?
    } else {
        let (hash, suffix) = name.strip_prefix(".sha256-")?.split_once(".json.tmp-")?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        suffix
    };
    let number = |text: &str| -> Option<u64> {
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        text.parse().ok()
    };
    let mut parts = suffix.split('-');
    let pid = i32::try_from(number(parts.next()?)?)
        .ok()
        .filter(|pid| *pid > 0)?;
    if !floor && (number(parts.next()?)? == 0 || number(parts.next()?)? >= 128) {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(pid)
}

#[cfg(test)]
fn require_absent_creator(pid: i32) -> Result<(), ProviderFault> {
    #[cfg(unix)]
    if unsafe { libc::kill(pid, 0) } == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return Ok(());
    }
    #[cfg(not(unix))]
    let _ = pid;
    Err(ProviderFault::corrupt_journal(
        "journal transaction creator is live or its exit is unproven",
    ))
}

fn same_optional_snapshot(left: Option<&fs::Metadata>, right: Option<&fs::Metadata>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => same_snapshot(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn open_journal_directory(path: &Path) -> Result<fs::File, ProviderFault> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .map_err(|_| ProviderFault::corrupt_journal("journal recovery directory is unreadable"))?;
    verify_journal_directory(path, &file)?;
    Ok(file)
}

/// How long a busy journal directory lock is retried before it counts as a
/// pending transaction. A child spawned on another thread holds a copy of this
/// close-on-exec descriptor until it execs, which keeps an flock alive after its
/// owner closed it; under load that window lasts milliseconds, never seconds.
#[cfg(unix)]
const JOURNAL_LOCK_RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);
#[cfg(unix)]
const JOURNAL_LOCK_RETRY_STEP: std::time::Duration = std::time::Duration::from_millis(5);

fn lock_journal_directory(directory: &fs::File) -> Result<(), ProviderFault> {
    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + JOURNAL_LOCK_RETRY_WINDOW;
        loop {
            match directory.try_lock() {
                Ok(()) => return Ok(()),
                Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(JOURNAL_LOCK_RETRY_STEP);
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(ProviderFault::selection_unavailable(
                        "journal transaction pending",
                    ))
                }
                Err(_) => {
                    return Err(ProviderFault::corrupt_journal(
                        "journal transaction lock is unavailable",
                    ))
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Err(ProviderFault::corrupt_journal(
            "journal transaction recovery lock is unavailable",
        ))
    }
}

#[cfg(unix)]
fn lock_transaction_directories(path: &Path) -> Result<Vec<(PathBuf, fs::File)>, ProviderFault> {
    let parent = path.parent().ok_or_else(|| {
        ProviderFault::corrupt_journal("journal transaction parent is unavailable")
    })?;
    let root = if path
        .file_name()
        .is_some_and(|name| name == "pruned-request-floor.json" || name == "recovery-complete.json")
    {
        parent
    } else {
        parent.parent().ok_or_else(|| {
            ProviderFault::corrupt_journal("journal transaction root is unavailable")
        })?
    };
    let mut held = Vec::with_capacity(2);
    for directory in [root, parent] {
        if held.iter().any(|(path, _)| path == directory) {
            continue;
        }
        let file = open_journal_directory(directory)?;
        lock_journal_directory(&file)?;
        held.push((directory.to_path_buf(), file));
    }
    Ok(held)
}

#[cfg(unix)]
fn verify_transaction_directories(
    directories: &[(PathBuf, fs::File)],
) -> Result<(), ProviderFault> {
    for (path, directory) in directories {
        verify_journal_directory(path, directory)?;
    }
    Ok(())
}

fn archive_identity(metadata: &fs::Metadata) -> Result<(u64, u64), ProviderFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(ProviderFault::corrupt_journal(
            "protected recovery archives require Unix directory identity",
        ))
    }
}

fn rename_directory_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // The release builds Linux binaries against musl, whose libc has no renameat2 wrapper;
        // the raw syscall works with glibc and musl alike.
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "protected recovery archive rename is unavailable",
        ))
    }
}

fn verify_journal_directory(path: &Path, held: &fs::File) -> Result<(), ProviderFault> {
    let opened = held
        .metadata()
        .map_err(|_| ProviderFault::corrupt_journal("opened journal directory is unreadable"))?;
    let current = fs::symlink_metadata(path)
        .map_err(|_| ProviderFault::corrupt_journal("journal directory changed during recovery"))?;
    validate_existing_directory(path, &opened)?;
    validate_existing_directory(path, &current)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() == current.dev() && opened.ino() == current.ino() {
            return Ok(());
        }
    }
    #[cfg(not(unix))]
    if same_snapshot(&opened, &current) {
        return Ok(());
    }
    Err(ProviderFault::corrupt_journal(
        "journal directory identity changed during recovery",
    ))
}

fn delete_verified_transaction(
    path: &Path,
    expected: &fs::Metadata,
    parent: &fs::File,
) -> Result<(), ProviderFault> {
    let held = open_run_readonly(path)?;
    let opened = held
        .metadata()
        .map_err(|_| ProviderFault::corrupt_journal("opened transaction is unreadable"))?;
    validate_existing_file(path, &opened)?;
    if !same_snapshot(expected, &opened) {
        return Err(ProviderFault::corrupt_journal(
            "journal transaction inode changed during recovery",
        ));
    }
    verify_path_snapshot(path, expected)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.file_name().unwrap().as_bytes())
            .map_err(|_| ProviderFault::corrupt_journal("invalid transaction filename"))?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            return Ok(());
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
    Err(ProviderFault::corrupt_journal(
        "owned journal transaction removal failed",
    ))
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(ProviderFault::corrupt_journal(
                "provider journal file has multiple links",
            ));
        }
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
                hosted: crate::config::test_hosted_disclosure(),
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
            backend_report: Some(BackendReport::unknown()),
            terminal: true,
        }];
        run
    }

    #[test]
    fn stored_decision_output_retains_operation_and_pinned_model() {
        let mut run = prepared_run("decision");
        run.offer.operation = elastos_model_contract::decisions::OPERATION.into();
        run.offer.hosted.as_mut().unwrap().requested_selector = "typesafe/jev-1.13".into();
        let mut output = serde_json::json!({"schema": elastos_model_contract::decisions::OUTPUT_SCHEMA,
            "model":"typesafe/jev-1.13", "answers":{"review":{"type":"choice","choice":"defer"}}});
        let path = Path::new("fixture-run.json");
        validate_stored_output(path, &run, &output).unwrap();
        output["model"] = serde_json::json!("different-model");
        assert!(validate_stored_output(path, &run, &output).is_err());
        assert!(validate_stored_output(
            path,
            &run,
            &serde_json::json!({"schema":RUN_OUTPUT_TEXT_SCHEMA,"text":"approve"})
        )
        .is_err());
        run.offer.operation = "text.generate".into();
        assert!(validate_stored_output(path, &run, &output).is_err());
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
            backend_report: Some(BackendReport::unknown()),
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
        path.parent().unwrap().join(format!(
            ".{file_name}.tmp-{}-1000-0",
            abandoned_fixture_pid()
        ))
    }

    fn abandoned_fixture_pid() -> u32 {
        #[cfg(unix)]
        {
            static PID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *PID.get_or_init(|| {
                let mut child = std::process::Command::new("/bin/sh")
                    .args(["-c", "exit 0"])
                    .spawn()
                    .unwrap();
                let pid = child.id();
                assert!(child.wait().unwrap().success());
                require_absent_creator(pid as i32).unwrap();
                pid
            })
        }
        #[cfg(not(unix))]
        std::process::id()
    }

    #[cfg(unix)]
    fn interrupted_transaction(root: &Path, floor: bool) -> PathBuf {
        // This owned child dies after a partial write and before rename. The
        // fixture proves absence through wait and the production PID check.
        let stem = if floor {
            "floor".into()
        } else {
            format!("sha256-{}.json", "a".repeat(64))
        };
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "umask 077; if [ \"$2\" = floor ]; then path=\"$1/.pruned-request-floor.json.tmp-$$\"; else path=\"$1/runs/.$2.tmp-$$-1000-0\"; fi; printf '{' > \"$path\"; kill -KILL $$", "fixture"])
            .arg(root)
            .arg(&stem)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!child.wait().unwrap().success());
        require_absent_creator(pid as i32).unwrap();
        let path = if floor {
            root.join(format!(".pruned-request-floor.json.tmp-{pid}"))
        } else {
            root.join("runs").join(format!(".{stem}.tmp-{pid}-1000-0"))
        };
        assert_eq!(file_bytes(&path), b"{");
        path
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
        assert_eq!(
            journal.load_run(&linked_run.run_id).unwrap_err().code(),
            "journal_corrupt"
        );
    }

    #[test]
    #[cfg(unix)]
    fn missing_requested_run_requires_an_intact_private_directory() {
        let root = temp_root("missing-requested-run");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run_id = prepared_run("request:missing").run_id;
        assert_eq!(
            journal.load_run(&run_id).unwrap_err().code(),
            "run_not_found"
        );
        let path = hashed_path(&journal.runs_dir, &run_id);
        assert_eq!(
            load_verified_run_file(&path, &journal.runs_dir, None)
                .unwrap_err()
                .code(),
            "journal_corrupt"
        );
        for mode in [0o755, 0o000] {
            set_mode(&journal.runs_dir, mode);
            assert_eq!(
                journal.load_run(&run_id).unwrap_err().code(),
                "journal_corrupt"
            );
        }
        set_mode(&journal.runs_dir, 0o700);
        fs::remove_dir(&journal.runs_dir).unwrap();
        assert_eq!(
            journal.load_run(&run_id).unwrap_err().code(),
            "journal_corrupt"
        );
        let target = root.join("replacement");
        fs::create_dir(&target).unwrap();
        set_mode(&target, 0o700);
        symlink_path(&target, &journal.runs_dir);
        assert_eq!(
            journal.load_run(&run_id).unwrap_err().code(),
            "journal_corrupt"
        );
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

        #[cfg(unix)]
        {
            let reopened = RunJournal::open(root.clone()).unwrap();
            assert_eq!(reopened.active_run_count("local-text").unwrap(), 1);
            assert_eq!(reopened.load_run(&run.run_id).unwrap().run_id, run.run_id);
            assert!(!temp_path.exists());
        }
        #[cfg(not(unix))]
        {
            assert!(RunJournal::open(root).is_err());
            assert!(temp_path.exists());
        }
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
            backend_report: None,
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
        // Retain the separate malformed binding while constructing this fixture.
        overwrite_run_file(
            &run_path(&root, &valid_policy_run.run_id),
            &valid_policy_run,
        );
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
        overwrite_run_file(&run_path(&root, &timestamp.run_id), &timestamp);
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

        let mut invalid_disclosure = valid.clone();
        invalid_disclosure
            .offer
            .hosted
            .as_mut()
            .unwrap()
            .privacy_policy_ref = "fixture:privacy\nv1".to_string();
        let disclosure_error = journal.store_run(&invalid_disclosure).unwrap_err();
        assert_eq!(disclosure_error.code(), "journal_corrupt");
        assert_eq!(file_bytes(&path), original_bytes);

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
            delete_verified_run_file(&malformed_path, &root.join("runs"), &expired.run_id, |_| {
                Ok(())
            })
            .unwrap_err();
        assert_eq!(malformed_error.code(), "journal_corrupt");
        assert!(malformed_path.exists());

        let expired_unexpired = completed_run("request:delete-unexpired", 1, 10, 1);
        let unexpired_path = run_path(&root, &expired_unexpired.run_id);
        overwrite_run_file(&unexpired_path, &expired_unexpired);
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
            |_| Ok(()),
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

    fn crash_transaction(root: &Path, index: usize, bytes: u64) -> PathBuf {
        let path = root.join("runs").join(format!(
            ".sha256-{index:064x}.json.tmp-{}-1000-0",
            abandoned_fixture_pid()
        ));
        open_temp_file_owner_only(&path)
            .unwrap()
            .set_len(bytes)
            .unwrap();
        path
    }

    fn settle_unknown(run: &StoredRun) -> StoredRun {
        let mut next = run.clone();
        let terminal = now_ms().max(run.updated_at_ms);
        next.status = RunStatus::SettlementUnknown;
        next.backend_state = None;
        next.output = None;
        next.error = Some(RunError {
            class: crate::contract::ErrorClass::SettlementUnknown,
            code: "settlement_unknown".into(),
            message: "model backend settlement is unknown".into(),
        });
        next.events.push(RunEvent {
            schema: RUN_EVENT_SCHEMA.into(),
            sequence: next.next_sequence,
            kind: "settlement_unknown".into(),
            data: serde_json::json!({"code":"settlement_unknown","class":"settlement_unknown"}),
            backend_report: next.offer.hosted.as_ref().map(|_| BackendReport::unknown()),
            terminal: true,
        });
        next.next_sequence += 1;
        next.updated_at_ms = terminal;
        next.terminal_at_ms = Some(terminal);
        next.retention_until_ms = Some(terminal + next.offer.policy.retention_secs * 1000);
        next
    }

    #[test]
    fn census_bounds_entries_and_aggregate_before_any_decode_or_prune() {
        let root = temp_root("census-bounds");
        let journal = RunJournal::open(root.clone()).unwrap();
        let expired = completed_run("expired-retained-on-bound", 1, 10, 1);
        journal.store_run(&expired).unwrap();
        let committed = run_path(&root, &expired.run_id);
        let original = file_bytes(&committed);
        for index in 0..MAX_RECOVERY_JOURNAL_ENTRIES - 1 {
            crash_transaction(&root, index, 0);
        }
        assert_eq!(
            journal.census().unwrap().len(),
            MAX_RECOVERY_JOURNAL_ENTRIES
        );
        let extra = crash_transaction(&root, MAX_RECOVERY_JOURNAL_ENTRIES, 0);
        assert_eq!(
            journal.prune_expired_terminal_runs().unwrap_err().code(),
            "journal_corrupt"
        );
        assert_eq!(file_bytes(&committed), original);
        fs::remove_file(extra).unwrap();
        for entry in journal.census().unwrap() {
            if entry.transaction {
                fs::remove_file(entry.path).unwrap();
            }
        }
        for index in 0..4 {
            crash_transaction(&root, index, MAX_RUN_JOURNAL_BYTES);
        }
        // The committed record adds bytes beyond the128MiB recovery ceiling.
        assert_eq!(
            journal.prune_expired_terminal_runs().unwrap_err().code(),
            "journal_corrupt"
        );
        assert_eq!(file_bytes(&committed), original);
        fs::remove_file(&committed).unwrap();
        assert_eq!(
            journal
                .census()
                .unwrap()
                .iter()
                .map(|entry| entry.metadata.len())
                .sum::<u64>(),
            MAX_RECOVERY_SCAN_BYTES
        );
        let extra = crash_transaction(&root, 4, 1);
        assert!(journal.census().is_err());
        assert_eq!(fs::metadata(extra).unwrap().len(), 1);
    }

    #[test]
    fn new_admission_counts_crash_transactions_and_reserves_active_and_unknown_tail() {
        let root = temp_root("admission-reservations");
        let journal = RunJournal::open(root.clone()).unwrap();
        for index in 0..64 {
            let run = prepared_run(&format!("reserved-{index}"));
            journal.store_run(&run).unwrap();
        }
        let rejected = prepared_run("reserved-overflow");
        assert_eq!(
            journal.store_run(&rejected).unwrap_err().code(),
            "policy_limit"
        );
        assert!(!run_path(&root, &rejected.run_id).exists());
        assert_eq!(
            journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes,
            MAX_JOURNAL_GROWTH_BYTES
        );
        for index in 0..2 {
            let active = journal
                .load_run(&prepared_run(&format!("reserved-{index}")).run_id)
                .unwrap();
            journal.store_run(&settle_unknown(&active)).unwrap();
            assert_eq!(
                journal.load_run(&active.run_id).unwrap().status,
                RunStatus::SettlementUnknown
            );
        }
        assert!(
            journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes < MAX_JOURNAL_GROWTH_BYTES
        );
        journal.store_run(&rejected).unwrap();
        let root = temp_root("admission-crash-count");
        let journal = RunJournal::open(root.clone()).unwrap();
        for index in 0..MAX_NEW_JOURNAL_ENTRIES {
            crash_transaction(&root, index, 0);
        }
        assert_eq!(
            journal.store_run(&rejected).unwrap_err().code(),
            "policy_limit"
        );
        assert!(!run_path(&root, &rejected.run_id).exists());
    }

    #[test]
    fn legacy_above_admission_budget_settles_original_binding_and_keeps_unknown() {
        let root = temp_root("legacy-recovery");
        let journal = RunJournal::open(root.clone()).unwrap();
        let legacy = prepared_run("legacy-protected");
        overwrite_run_file(&run_path(&root, &legacy.run_id), &legacy);
        for index in 0..3 {
            crash_transaction(&root, index, 24 * 1024 * 1024);
        }
        let before = journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes;
        assert!(before > MAX_JOURNAL_GROWTH_BYTES && before < MAX_RECOVERY_SCAN_BYTES);
        assert_eq!(
            journal
                .store_run(&prepared_run("blocked-new"))
                .unwrap_err()
                .code(),
            "policy_limit"
        );
        let recovered = settle_unknown(&legacy);
        journal.store_run(&recovered).unwrap();
        assert!(journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes <= before);
        let actual = journal.load_run(&legacy.run_id).unwrap();
        assert!(same_execution_identity(&legacy, &actual));
        assert_eq!(actual.status, RunStatus::SettlementUnknown);
        let mut expired = actual.clone();
        expired.created_at_ms = 1;
        expired.deadline_ms = 31;
        expired.updated_at_ms = 10;
        expired.terminal_at_ms = Some(10);
        expired.retention_until_ms = Some(60_010);
        // Simulate a legacy expiry without granting a new execution binding.
        overwrite_run_file(&run_path(&root, &expired.run_id), &expired);
        let bytes = file_bytes(&run_path(&root, &expired.run_id));
        let restarted = RunJournal::open(root.clone()).unwrap();
        assert!(!run_path(&root, &expired.run_id).exists());
        assert!(!bytes.is_empty());
        assert_eq!(restarted.census().unwrap().len(), 0);
    }

    #[test]
    fn replacement_at_512_entries_has_only_one_owned_extra_transaction() {
        let root = temp_root("recovery-atomic-count");
        let journal = RunJournal::open(root.clone()).unwrap();
        let mut run = prepared_run("512-entry-replacement");
        journal.store_run(&run).unwrap();
        for index in 0..MAX_RECOVERY_JOURNAL_ENTRIES - 1 {
            crash_transaction(&root, index, 0);
        }
        run.updated_at_ms += 1;
        journal.store_run(&run).unwrap();
        assert_eq!(
            journal.census().unwrap().len(),
            MAX_RECOVERY_JOURNAL_ENTRIES
        );
        let path = run_path(&root, &run.run_id);
        let metadata = fs::symlink_metadata(&path).unwrap();
        let bytes = encode_bounded(&settle_unknown(&run), MAX_NEW_RUN_JOURNAL_BYTES).unwrap();
        atomic_write_with(
            &path,
            &bytes,
            Some(&metadata),
            |file, bytes| {
                assert_eq!(
                    fs::read_dir(root.join("runs"))?.count(),
                    MAX_RECOVERY_JOURNAL_ENTRIES + 1
                );
                assert!(bytes.len() as u64 <= MAX_NEW_RUN_JOURNAL_BYTES);
                file.write_all(bytes)?;
                file.sync_all()
            },
            |from, to| fs::rename(from, to),
            sync_directory,
        )
        .unwrap();
        assert_eq!(
            journal.census().unwrap().len(),
            MAX_RECOVERY_JOURNAL_ENTRIES
        );
        assert_eq!(
            journal.load_run(&run.run_id).unwrap().status,
            RunStatus::SettlementUnknown
        );
    }

    #[test]
    fn compact_header_backend_and_record_bounds_preserve_existing_binding() {
        let root = temp_root("compact-shapes");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("backend-shape");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let original = file_bytes(&path);
        let mut active = run.clone();
        active.status = RunStatus::Running;
        active.backend_state =
            Some(serde_json::json!({"opaque":"x".repeat(MAX_RUN_BACKEND_STATE_BYTES as usize)}));
        assert_eq!(
            journal.store_run(&active).unwrap_err().code(),
            "policy_limit"
        );
        assert_eq!(file_bytes(&path), original);
        // Exercise the raw encoding cap separately from stored-string validation.
        let mut header = prepared_run("header-shape");
        let escaped = format!("{}a", "\u{0001}".repeat(MAX_HOSTED_POLICY_REF_BYTES - 1));
        header.offer.hosted.as_mut().unwrap().privacy_policy_ref = escaped.clone();
        header.offer.hosted.as_mut().unwrap().terms_ref = escaped;
        for field in [
            &mut header.runtime_binding.principal_id,
            &mut header.runtime_binding.session_id,
            &mut header.runtime_binding.capsule_id,
            &mut header.runtime_binding.grant_id,
        ] {
            *field = format!("{}a", "\u{0001}".repeat(MAX_RUNTIME_BINDING_ID_BYTES - 1));
        }
        assert!((serde_json::to_vec(&header).unwrap().len() as u64) < MAX_RUN_HEADER_BYTES);
        assert!(validate_new_run_shape(&header).is_ok());
        header.offer.title = format!("{}a", "\u{0001}".repeat(MAX_OFFER_TITLE_BYTES - 1));
        header.offer.hosted.as_mut().unwrap().backend_provider_label = format!(
            "{}a",
            "\u{0001}".repeat(MAX_HOSTED_PROVIDER_LABEL_BYTES - 1)
        );
        header.offer.hosted.as_mut().unwrap().requested_selector =
            format!("{}a", "\u{0001}".repeat(MAX_MODEL_BYTES - 1));
        assert!((serde_json::to_vec(&header).unwrap().len() as u64) > MAX_RUN_HEADER_BYTES);
        assert!(validate_new_run_shape(&header).is_err());
        assert_eq!(
            journal.store_run(&header).unwrap_err().code(),
            "journal_corrupt"
        );
        assert!(!run_path(&root, &header.run_id).exists());
        assert_eq!(file_bytes(&path), original);
        let mut valid_header = prepared_run("escaped-header-shape");
        let escaped = format!("{}a", "\"\\".repeat((MAX_HOSTED_POLICY_REF_BYTES - 1) / 2));
        valid_header
            .offer
            .hosted
            .as_mut()
            .unwrap()
            .privacy_policy_ref = escaped.clone();
        valid_header.offer.hosted.as_mut().unwrap().terms_ref = escaped;
        let valid_path = run_path(&root, &valid_header.run_id);
        validate_stored_run(
            &valid_path,
            &root.join("runs"),
            Some(&valid_header.run_id),
            &valid_header,
        )
        .unwrap();
        assert!(validate_new_run_shape(&valid_header).is_ok());
        journal.store_run(&valid_header).unwrap();
        let retained = journal.load_run(&valid_header.run_id).unwrap();
        assert_eq!(retained.runtime_binding, valid_header.runtime_binding);
        assert_eq!(retained.offer, valid_header.offer);
        assert_eq!(journal.census().unwrap().len(), 2);
        assert_eq!(file_bytes(&path), original);
        assert!(encode_bounded(
            &"x".repeat(MAX_NEW_RUN_JOURNAL_BYTES as usize),
            MAX_NEW_RUN_JOURNAL_BYTES
        )
        .is_err());
        assert!(!file_bytes(&path).contains(&b'\n'));
    }

    #[test]
    fn read_grows_past_limit_and_opened_record_replacement_fail_closed() {
        let root = temp_root("stable-open-read");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("stable-read");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let before = fs::symlink_metadata(&path).unwrap();
        let error = load_verified_run_snapshot_with_reader(
            &path,
            &root.join("runs"),
            Some(&run.run_id),
            &before,
            |file, limit| {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b" ")
                    .unwrap();
                read_bounded_record(file, limit)
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
        let before = fs::symlink_metadata(&path).unwrap();
        let displaced = root.join("displaced.json");
        let replacement = prepared_run("replacement-read");
        let error = load_verified_run_snapshot_with_reader(
            &path,
            &root.join("runs"),
            Some(&run.run_id),
            &before,
            |file, limit| {
                fs::rename(&path, &displaced).unwrap();
                overwrite_run_file(&path, &replacement);
                read_bounded_record(file, limit)
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
        assert!(displaced.exists());
        assert_eq!(
            serde_json::from_slice::<StoredRun>(&file_bytes(&path))
                .unwrap()
                .run_id,
            replacement.run_id
        );
    }

    #[cfg(unix)]
    #[test]
    fn hardlink_and_nonregular_census_entries_fail_closed() {
        let root = temp_root("census-file-types");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("hardlink");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let link = root.join("hardlink.json");
        fs::hard_link(&path, &link).unwrap();
        assert!(journal.census().is_err());
        assert!(journal.load_run(&run.run_id).is_err());
        fs::remove_file(link).unwrap();
        fs::create_dir(root.join("runs").join("unexpected-directory")).unwrap();
        assert!(journal.census().is_err());
        assert!(path.exists());
    }

    #[test]
    fn failed_atomic_write_cleans_only_fresh_owned_temp_and_allows_retry() {
        let root = temp_root("atomic-faults");
        let journal = RunJournal::open(root.clone()).unwrap();
        let mut run = prepared_run("atomic-fault");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let original = file_bytes(&path);
        let stale = crash_transaction(&root, 0, 7);
        let bytes = encode_bounded(&run, MAX_NEW_RUN_JOURNAL_BYTES).unwrap();
        for code in [libc::ENOSPC, libc::EIO] {
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(atomic_write_with(
                &path,
                &bytes,
                Some(&metadata),
                |file, bytes| {
                    file.write_all(if code == libc::ENOSPC {
                        &bytes[..8]
                    } else {
                        bytes
                    })?;
                    Err(std::io::Error::from_raw_os_error(code))
                },
                |from, to| fs::rename(from, to),
                sync_directory,
            )
            .is_err());
            assert_eq!(file_bytes(&path), original);
            assert_eq!(journal.census().unwrap().len(), 2);
        }
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(atomic_write_with(
            &path,
            &bytes,
            Some(&metadata),
            |file, bytes| {
                file.write_all(bytes)?;
                file.sync_all()
            },
            |_, _| Err(std::io::Error::from_raw_os_error(libc::EIO)),
            sync_directory,
        )
        .is_err());
        assert_eq!(file_bytes(&path), original);
        assert!(stale.exists());
        run.updated_at_ms += 1;
        journal.store_run(&run).unwrap();
        assert!(same_execution_identity(
            &run,
            &journal.load_run(&run.run_id).unwrap()
        ));
        assert_eq!(journal.census().unwrap().len(), 2);
    }

    #[test]
    fn replaced_transaction_path_is_preserved_on_failure() {
        let root = temp_root("atomic-temp-replacement");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("temp-replacement");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let original = file_bytes(&path);
        let metadata = fs::symlink_metadata(&path).unwrap();
        let bytes = encode_bounded(&run, MAX_NEW_RUN_JOURNAL_BYTES).unwrap();
        let mut replacement = None;
        assert!(atomic_write_with(
            &path,
            &bytes,
            Some(&metadata),
            |_, _| {
                let temp = fs::read_dir(root.join("runs"))?
                    .map(|entry| entry.unwrap().path())
                    .find(|path| is_transaction_temp_file(path))
                    .unwrap();
                fs::remove_file(&temp)?;
                open_temp_file_owner_only(&temp)
                    .expect("open owned fixture replacement")
                    .write_all(b"replacement")?;
                replacement = Some(temp);
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            },
            |from, to| fs::rename(from, to),
            sync_directory,
        )
        .is_err());
        assert_eq!(file_bytes(&path), original);
        assert_eq!(file_bytes(&replacement.unwrap()), b"replacement");
    }

    #[test]
    fn legacy_large_backend_settles_with_original_binding_and_nonincreasing_charge() {
        let root = temp_root("legacy-large-backend");
        let journal = RunJournal::open(root.clone()).unwrap();
        let mut legacy = prepared_run("large-legacy");
        legacy.status = RunStatus::Running;
        legacy.backend_state =
            Some(serde_json::json!({"legacy_opaque":"x".repeat(2 * 1024 * 1024)}));
        let path = run_path(&root, &legacy.run_id);
        overwrite_run_file(&path, &legacy);
        let prior_charge = journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes;
        assert!(prior_charge > MAX_NEW_RUN_JOURNAL_BYTES);
        assert_eq!(
            journal.load_run(&legacy.run_id).unwrap().status,
            RunStatus::Running
        );
        journal.store_run(&settle_unknown(&legacy)).unwrap();
        let actual = journal.load_run(&legacy.run_id).unwrap();
        assert!(same_execution_identity(&legacy, &actual));
        assert_eq!(actual.status, RunStatus::SettlementUnknown);
        assert!(journal.visit_runs(|_, _, _| Ok(())).unwrap().charged_bytes < prior_charge);
        let bytes = file_bytes(&path);
        let mut changed = actual.clone();
        changed.execution_binding_hash = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            journal.store_run(&changed).unwrap_err().code(),
            "journal_corrupt"
        );
        assert_eq!(file_bytes(&path), bytes);
    }

    #[test]
    fn committed_record_survives_parent_sync_failure() {
        let root = temp_root("parent-sync-failure");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("parent-sync");
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let metadata = fs::symlink_metadata(&path).unwrap();
        let next = settle_unknown(&run);
        let bytes = encode_bounded(&next, MAX_NEW_RUN_JOURNAL_BYTES).unwrap();
        assert!(atomic_write_with(
            &path,
            &bytes,
            Some(&metadata),
            |file, bytes| {
                file.write_all(bytes)?;
                file.sync_all()
            },
            |from, to| fs::rename(from, to),
            |_| Err(ProviderFault::corrupt_journal(
                "fixture parent sync failure"
            ))
        )
        .is_err());
        assert_eq!(file_bytes(&path), bytes);
        assert_eq!(journal.census().unwrap().len(), 1);
        let actual = journal.load_run(&run.run_id).unwrap();
        assert!(same_execution_identity(&run, &actual));
        assert_eq!(actual.status, RunStatus::SettlementUnknown);
    }

    #[test]
    fn replay_retention_uses_strict_maximum_and_keeps_unknown_and_legacy_facts() {
        let id = crate::test_support::fixture_model_request_id_at("replay-window", 1_000);
        let mut run = completed_run(&id, 1_000, 1_100, 1);
        let replay = elastos_model_contract::model_request_replay_deadline_ms(&id).unwrap();
        assert_eq!(replay, 361_000);
        assert!(!is_expired_terminal_run(&run, 2_101));
        assert!(!is_expired_terminal_run(&run, replay));
        assert!(is_expired_terminal_run(&run, replay + 1));
        run.offer.policy.retention_secs = 400;
        run.retention_until_ms = Some(401_100);
        assert!(!is_expired_terminal_run(&run, 401_100));
        assert!(is_expired_terminal_run(&run, 401_101));
        let unknown = settle_unknown(&prepared_run(&id));
        assert!(is_expired_terminal_run(&unknown, u64::MAX));
        let legacy = completed_run("legacy-retained", 1_000, 1_100, 1);
        assert!(is_expired_terminal_run(&legacy, 2_101));
        assert_eq!(legacy.runtime_binding.request_id, "legacy-retained");
    }

    #[test]
    fn pruned_floor_survives_restart_and_refuses_clock_rollback_replay() {
        let root = temp_root("request-floor-restart");
        let journal = RunJournal::open(root.clone()).unwrap();
        let issued = now_ms() - 400_000;
        let id = crate::test_support::fixture_model_request_id_at("pruned", issued);
        let run = completed_run(&id, issued, issued + 10, 1);
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        journal.prune_expired_terminal_runs().unwrap();
        assert!(!path.exists());
        assert_eq!(journal.pruned_issued_floor_ms().unwrap(), issued);
        assert!(
            fs::metadata(&journal.request_floor_path).unwrap().len() <= MAX_REQUEST_FLOOR_BYTES
        );
        assert!(journal.census().unwrap().is_empty());
        let reopened = RunJournal::open(root).unwrap();
        assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
        // This request would be fresh after a clock rollback, but was already pruned.
        assert!(
            elastos_model_contract::validate_new_model_request_id(&id, issued + 1, issued).is_err()
        );
        let next = crate::test_support::fixture_model_request_id_at("next", issued + 1);
        assert!(
            elastos_model_contract::validate_new_model_request_id(&next, issued + 1, issued)
                .is_ok()
        );
        reopened
            .record_pruned_issuance(&crate::test_support::fixture_model_request_id_at(
                "older",
                issued - 1,
            ))
            .unwrap();
        assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
    }

    #[cfg(unix)]
    #[test]
    fn held_writer_locks_block_oversized_init_archive_and_preserve_journals() {
        for hold_root in [false, true] {
            let root = temp_root("held-writer-overflow-archive");
            let journal = RunJournal::open(root.clone()).unwrap();
            let unknown = settle_unknown(&prepared_run("held-writer-archive-unknown"));
            journal.store_run(&unknown).unwrap();
            let path = run_path(&root, &unknown.run_id);
            let before = file_bytes(&path);
            // A future committed floor makes the old archive floor write a
            // no-op, so the writer lock must protect the rename itself.
            let issued = now_ms() + 60_000;
            journal.record_pruned_issuance_ms(issued).unwrap();
            let floor_before = file_bytes(&journal.request_floor_path);
            let mut temps = Vec::new();
            for index in 0..MAX_RECOVERY_JOURNAL_ENTRIES {
                temps.push(crash_transaction(&root, index, 1));
            }
            let root_writer = if hold_root {
                let held = open_journal_directory(&root).unwrap();
                held.try_lock().unwrap();
                Some(held)
            } else {
                None
            };
            let runs_writer = open_journal_directory(&root.join("runs")).unwrap();
            runs_writer.try_lock().unwrap();
            assert_eq!(
                RunJournal::open(root.clone()).err().unwrap().code(),
                "selection_unavailable"
            );
            assert_eq!(file_bytes(&path), before);
            assert_eq!(file_bytes(&journal.request_floor_path), floor_before);
            assert!(temps.iter().all(|temp| file_bytes(temp) == vec![0]));
            assert_eq!(
                fs::read_dir(root.join("runs")).unwrap().count(),
                MAX_RECOVERY_JOURNAL_ENTRIES + 1
            );
            assert!(!root.join("recovery-runs").exists());
            assert!(!root.join("recovery-complete.json").exists());
            drop(runs_writer);
            drop(root_writer);
            let reopened = RunJournal::open(root.clone()).unwrap();
            assert!(reopened.census().unwrap().is_empty());
            assert!(reopened.archive_recovery_pending().unwrap());
            assert!(!root.join("recovery-complete.json").exists());
            assert_eq!(file_bytes(&reopened.request_floor_path), floor_before);
            assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
            assert_eq!(
                file_bytes(&hashed_path(&root.join("recovery-runs"), &unknown.run_id)),
                before
            );
            assert_eq!(
                reopened.load_run(&unknown.run_id).unwrap().status,
                RunStatus::SettlementUnknown
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn legacy_entry_and_byte_overflow_archive_once_with_exact_owner_access() {
        for byte_overflow in [false, true] {
            let root = temp_root("legacy-overflow-archive");
            let journal = RunJournal::open(root.clone()).unwrap();
            let id = crate::test_support::fixture_model_request_id_at(
                "future-retained",
                now_ms() + 60_000,
            );
            let run = settle_unknown(&prepared_run(&id));
            journal.store_run(&run).unwrap();
            let original = file_bytes(&run_path(&root, &run.run_id));
            let count = if byte_overflow {
                4
            } else {
                MAX_RECOVERY_JOURNAL_ENTRIES
            };
            for index in 0..count {
                crash_transaction(
                    &root,
                    index,
                    if byte_overflow {
                        MAX_RUN_JOURNAL_BYTES
                    } else {
                        0
                    },
                );
            }
            drop(journal);
            let journal = RunJournal::open(root.clone()).unwrap();
            assert!(journal.census().unwrap().is_empty());
            assert!(journal.archive_recovery_pending().unwrap());
            let retained = journal.load_run_if_present(&run.run_id).unwrap().unwrap();
            assert!(same_execution_identity(&run, &retained));
            assert_eq!(retained.status, RunStatus::SettlementUnknown);
            assert_eq!(
                file_bytes(&hashed_path(&root.join("recovery-runs"), &run.run_id)),
                original
            );
            assert!(journal.pruned_issued_floor_ms().unwrap() > 0);
            assert_eq!(
                journal.archive_legacy_runs().unwrap_err().code(),
                "journal_corrupt"
            );
            journal.acknowledge_archive_recovery().unwrap();
            let restarted = RunJournal::open(root.clone()).unwrap();
            assert!(!restarted.archive_recovery_pending().unwrap());
            assert_eq!(
                restarted.load_run(&run.run_id).unwrap().runtime_binding,
                run.runtime_binding
            );
            fs::rename(root.join("recovery-runs"), root.join("previous-recovery")).unwrap();
            ensure_journal_dir_owner_only(&root.join("recovery-runs")).unwrap();
            assert!(
                restarted.archive_recovery_pending().unwrap(),
                "a replacement archive cannot inherit acknowledgement"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn archive_restart_after_rename_keeps_gate_and_rejects_unsafe_exact_record() {
        let root = temp_root("archive-rename-crash");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = settle_unknown(&prepared_run("legacy-archive-owner"));
        journal.store_run(&run).unwrap();
        journal.record_pruned_issuance_ms(now_ms()).unwrap();
        rename_directory_no_replace(&root.join("runs"), &root.join("recovery-runs")).unwrap();
        let abandoned_ack = root.join(format!(
            ".recovery-complete.json.tmp-{}",
            abandoned_fixture_pid()
        ));
        open_temp_file_owner_only(&abandoned_ack)
            .unwrap()
            .write_all(b"{")
            .unwrap();
        let restarted = RunJournal::open(root.clone()).unwrap();
        assert!(!abandoned_ack.exists());
        assert!(restarted.archive_recovery_pending().unwrap());
        assert_eq!(
            restarted.load_run(&run.run_id).unwrap().status,
            RunStatus::SettlementUnknown
        );
        let archived = hashed_path(&root.join("recovery-runs"), &run.run_id);
        fs::rename(&archived, root.join("protected-original.json")).unwrap();
        symlink_path(&root.join("protected-original.json"), &archived);
        assert_eq!(
            restarted.load_run(&run.run_id).unwrap_err().code(),
            "journal_corrupt"
        );
        let other = root.join("other-owner-directory");
        ensure_journal_dir_owner_only(&other).unwrap();
        assert!(rename_directory_no_replace(&root.join("runs"), &other).is_err());
        assert!(root.join("runs").exists());
    }

    #[test]
    fn failed_floor_persistence_keeps_run_and_only_one_crash_transaction() {
        let root = temp_root("request-floor-crash");
        let journal = RunJournal::open(root.clone()).unwrap();
        let issued = now_ms() - 400_000;
        let id = crate::test_support::fixture_model_request_id_at("floor-crash", issued);
        let run = completed_run(&id, issued, issued + 10, 1);
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let before = file_bytes(&path);
        let unknown = settle_unknown(&prepared_run("retained-unknown-during-floor-recovery"));
        journal.store_run(&unknown).unwrap();
        let unknown_path = run_path(&root, &unknown.run_id);
        let unknown_before = file_bytes(&unknown_path);
        #[cfg(unix)]
        {
            let temp = interrupted_transaction(&root, true);
            assert_eq!(file_bytes(&path), before);
            assert_eq!(journal.pruned_issued_floor_ms().unwrap(), 0);
            let reopened = RunJournal::open(root.clone()).unwrap();
            assert!(!temp.exists());
            assert!(!path.exists());
            assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
            assert_eq!(file_bytes(&unknown_path), unknown_before);
            assert_eq!(
                reopened.load_run(&unknown.run_id).unwrap().status,
                RunStatus::SettlementUnknown
            );
            let floor_before = file_bytes(&reopened.request_floor_path);
            let reopened = RunJournal::open(root).unwrap();
            assert_eq!(file_bytes(&reopened.request_floor_path), floor_before);
            assert!(elastos_model_contract::validate_new_model_request_id(
                &id,
                issued + 1,
                reopened.pruned_issued_floor_ms().unwrap()
            )
            .is_err());
        }
        #[cfg(not(unix))]
        {
            let temp = root.join(format!(
                ".pruned-request-floor.json.tmp-{}",
                std::process::id()
            ));
            open_temp_file_owner_only(&temp)
                .unwrap()
                .write_all(b"{")
                .unwrap();
            assert!(RunJournal::open(root).is_err());
            assert_eq!(file_bytes(&path), before);
            assert_eq!(file_bytes(&unknown_path), unknown_before);
            assert!(temp.exists());
        }
    }

    #[test]
    #[cfg(unix)]
    fn unlocked_reused_creator_pid_recovers_transactions_and_preserves_unknown_and_floor() {
        let root = temp_root("reused-transaction-creator");
        let journal = RunJournal::open(root.clone()).unwrap();
        let unknown = settle_unknown(&prepared_run("reused-pid-retained-unknown"));
        journal.store_run(&unknown).unwrap();
        let path = run_path(&root, &unknown.run_id);
        let before = file_bytes(&path);
        let issued = now_ms().saturating_sub(10_000);
        journal.record_pruned_issuance_ms(issued).unwrap();
        let floor_before = file_bytes(&journal.request_floor_path);
        let pid = std::process::id();
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);
        let transactions = [
            root.join(format!(".pruned-request-floor.json.tmp-{pid}")),
            root.join(format!(".recovery-complete.json.tmp-{pid}")),
            root.join("runs")
                .join(format!(".sha256-{}.json.tmp-{pid}-1000-0", "a".repeat(64))),
        ];
        for temp in &transactions {
            open_temp_file_owner_only(temp)
                .unwrap()
                .write_all(b"{")
                .unwrap();
        }
        let reopened = RunJournal::open(root).unwrap();
        assert!(transactions.iter().all(|path| !path.exists()));
        assert_eq!(file_bytes(&path), before);
        assert_eq!(file_bytes(&reopened.request_floor_path), floor_before);
        assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
        assert_eq!(
            reopened.load_run(&unknown.run_id).unwrap().status,
            RunStatus::SettlementUnknown
        );
        assert!(!reopened.archive_recovery_pending().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn transaction_locks_preserve_a_custom_journal_root_named_runs() {
        let root = temp_root("journal-root-named-runs").join("runs");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = prepared_run("custom-journal-root");
        journal.store_run(&run).unwrap();
        let issued = now_ms();
        journal.record_pruned_issuance_ms(issued).unwrap();
        let reopened = RunJournal::open(root).unwrap();
        assert_eq!(reopened.load_run(&run.run_id).unwrap().run_id, run.run_id);
        assert_eq!(reopened.pruned_issued_floor_ms().unwrap(), issued);
    }

    #[test]
    #[cfg(unix)]
    fn held_transaction_directory_lock_blocks_recovery_and_atomic_writes() {
        for floor in [false, true] {
            let root = temp_root(if floor {
                "held-floor-writer"
            } else {
                "held-run-writer"
            });
            let journal = RunJournal::open(root.clone()).unwrap();
            let unknown = settle_unknown(&prepared_run("held-writer-retained-unknown"));
            journal.store_run(&unknown).unwrap();
            let path = run_path(&root, &unknown.run_id);
            let before = file_bytes(&path);
            let directory = if floor {
                root.clone()
            } else {
                root.join("runs")
            };
            let temp = if floor {
                root.join(format!(
                    ".pruned-request-floor.json.tmp-{}",
                    abandoned_fixture_pid()
                ))
            } else {
                crash_transaction(&root, 0, 1)
            };
            if floor {
                open_temp_file_owner_only(&temp)
                    .unwrap()
                    .write_all(b"{")
                    .unwrap();
            }
            let temp_before = file_bytes(&temp);
            let writer = open_journal_directory(&directory).unwrap();
            // A child forked on another test thread can briefly hold a copy of an
            // earlier lock fd on this directory (#381); take the lock with the
            // provider's own bounded retry instead of one try.
            lock_journal_directory(&writer).unwrap();
            assert_eq!(
                RunJournal::open(root.clone()).err().unwrap().code(),
                "selection_unavailable"
            );
            assert_eq!(file_bytes(&temp), temp_before);
            assert_eq!(file_bytes(&path), before);
            let fresh = prepared_run("fresh-during-held-writer");
            assert_eq!(
                journal.store_run(&fresh).unwrap_err().code(),
                "selection_unavailable"
            );
            if floor {
                assert_eq!(
                    journal
                        .record_pruned_issuance_ms(now_ms())
                        .unwrap_err()
                        .code(),
                    "selection_unavailable"
                );
            }
            drop(writer);
            let reopened = RunJournal::open(root).unwrap();
            assert!(!temp.exists());
            assert_eq!(file_bytes(&path), before);
            reopened.store_run(&fresh).unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    fn briefly_held_directory_lock_is_waited_out_not_reported_pending() {
        // A lock that outlives its owner only until a concurrently spawned child
        // execs must not fail the transaction.
        let root = temp_root("briefly-held-writer");
        let journal = RunJournal::open(root.clone()).unwrap();
        let holder = open_journal_directory(&root.join("runs")).unwrap();
        holder.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(holder);
        });
        let fresh = prepared_run("after-brief-holder");
        journal.store_run(&fresh).unwrap();
        release.join().unwrap();
        assert!(run_path(&root, &fresh.run_id).exists());
    }

    #[test]
    #[cfg(unix)]
    fn interrupted_run_transaction_recovery_preserves_committed_floor_and_unknown() {
        let root = temp_root("run-transaction-interruption");
        let journal = RunJournal::open(root.clone()).unwrap();
        let issued = now_ms();
        let id = crate::test_support::fixture_model_request_id_at("committed-floor", issued);
        journal.record_pruned_issuance(&id).unwrap();
        let floor_before = file_bytes(&journal.request_floor_path);
        let run = settle_unknown(&prepared_run("unknown-before-interrupted-write"));
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let before = file_bytes(&path);
        let temp = interrupted_transaction(&root, false);
        let reopened = RunJournal::open(root).unwrap();
        assert!(!temp.exists());
        assert_eq!(file_bytes(&path), before);
        assert_eq!(file_bytes(&reopened.request_floor_path), floor_before);
        assert_eq!(
            reopened.load_run(&run.run_id).unwrap().status,
            RunStatus::SettlementUnknown
        );
        assert!(elastos_model_contract::validate_new_model_request_id(
            &id,
            issued + 1,
            reopened.pruned_issued_floor_ms().unwrap()
        )
        .is_err());
    }

    #[test]
    fn live_transaction_creator_preserves_partial_files_and_bindings() {
        for floor in [false, true] {
            let root = temp_root(if floor {
                "live-floor-creator"
            } else {
                "live-run-creator"
            });
            let journal = RunJournal::open(root.clone()).unwrap();
            let run = settle_unknown(&prepared_run("live-creator-retained"));
            journal.store_run(&run).unwrap();
            let path = run_path(&root, &run.run_id);
            let before = file_bytes(&path);
            let temp = if floor {
                root.join(format!(
                    ".pruned-request-floor.json.tmp-{}",
                    std::process::id()
                ))
            } else {
                root.join("runs").join(format!(
                    ".sha256-{}.json.tmp-{}-1000-0",
                    "a".repeat(64),
                    std::process::id()
                ))
            };
            open_temp_file_owner_only(&temp)
                .unwrap()
                .write_all(b"{")
                .unwrap();
            #[cfg(unix)]
            let _writer = {
                let writer = open_journal_directory(temp.parent().unwrap()).unwrap();
                writer.try_lock().unwrap();
                writer
            };
            assert!(RunJournal::open(root).is_err());
            assert_eq!(file_bytes(&temp), b"{");
            assert_eq!(file_bytes(&path), before);
        }
    }

    #[test]
    #[cfg(unix)]
    fn recovery_validates_all_records_before_removing_dead_creator_transactions() {
        let root = temp_root("invalid-record-preserves-crash-file");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = settle_unknown(&prepared_run("retained-invalid-neighbor"));
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let before = file_bytes(&path);
        let temp = interrupted_transaction(&root, false);
        let invalid = run_path(&root, "invalid-neighbor");
        open_temp_file_owner_only(&invalid)
            .unwrap()
            .write_all(b"{")
            .unwrap();
        assert!(RunJournal::open(root).is_err());
        assert_eq!(file_bytes(&temp), b"{");
        assert_eq!(file_bytes(&path), before);
        assert_eq!(file_bytes(&invalid), b"{");
    }

    #[test]
    #[cfg(unix)]
    fn recovery_preserves_replaced_transaction_and_parent_directory() {
        for replace_parent in [false, true] {
            let root = temp_root(if replace_parent {
                "replaced-transaction-parent"
            } else {
                "replaced-transaction-inode"
            });
            let journal = RunJournal::open(root.clone()).unwrap();
            let run = settle_unknown(&prepared_run("replacement-retained"));
            journal.store_run(&run).unwrap();
            let original = file_bytes(&run_path(&root, &run.run_id));
            let temp = interrupted_transaction(&root, false);
            let preserved = root.join("preserved-runs");
            let error = journal
                .recover_abandoned_transactions_with(|| {
                    if replace_parent {
                        fs::rename(root.join("runs"), &preserved).unwrap();
                        ensure_journal_dir_owner_only(&root.join("runs")).unwrap();
                    } else {
                        fs::rename(&temp, root.join("preserved-transaction")).unwrap();
                    }
                    open_temp_file_owner_only(&temp)
                        .unwrap()
                        .write_all(b"replacement")
                        .unwrap();
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(error.code(), "journal_corrupt");
            assert_eq!(file_bytes(&temp), b"replacement");
            let original_path = if replace_parent {
                hashed_path(&preserved, &run.run_id)
            } else {
                run_path(&root, &run.run_id)
            };
            assert_eq!(file_bytes(&original_path), original);
        }
    }

    #[test]
    #[cfg(unix)]
    fn recovery_refuses_unsafe_or_unattributed_transactions_before_cleanup() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        for case in [
            "untagged",
            "bad-name",
            "non-utf8",
            "symlink",
            "hardlink",
            "mode",
            "oversized",
            "count",
        ] {
            let root = temp_root(case);
            let journal = RunJournal::open(root.clone()).unwrap();
            let run = settle_unknown(&prepared_run("unsafe-temp-retained"));
            journal.store_run(&run).unwrap();
            let before = file_bytes(&run_path(&root, &run.run_id));
            let valid = interrupted_transaction(&root, false);
            let path = root.join(format!(
                ".pruned-request-floor.json.tmp-{}",
                abandoned_fixture_pid()
            ));
            let unsafe_path = match case {
                "untagged" => root.join(".pruned-request-floor.json.tmp"),
                "bad-name" => root.join(".pruned-request-floor.json.tmp-01"),
                "non-utf8" => {
                    let mut name = b".pruned-request-floor.json.tmp-".to_vec();
                    name.push(0xff);
                    root.join(std::ffi::OsString::from_vec(name))
                }
                _ => path,
            };
            if case == "non-utf8" {
                assert_eq!(transaction_creator(&unsafe_path, true), None);
                // APFS rejects this byte sequence before a file can exist. Keep
                // the parser refusal on Mac; Linux also checks disk recovery.
                #[cfg(target_os = "macos")]
                {
                    assert!(valid.exists());
                    assert_eq!(file_bytes(&run_path(&root, &run.run_id)), before);
                    continue;
                }
            }
            match case {
                "symlink" => symlink(&valid, &unsafe_path).unwrap(),
                "hardlink" => fs::hard_link(&valid, &unsafe_path).unwrap(),
                "count" => {
                    for index in 0..MAX_RECOVERY_JOURNAL_ENTRIES {
                        crash_transaction(&root, index, 0);
                    }
                }
                _ => {
                    let mut file = open_temp_file_owner_only(&unsafe_path).unwrap();
                    file.write_all(b"{").unwrap();
                    if case == "oversized" {
                        file.set_len(MAX_REQUEST_FLOOR_BYTES + 1).unwrap();
                    }
                    if case == "mode" {
                        set_mode(&unsafe_path, 0o644);
                    }
                }
            }
            if case == "count" {
                let recovered = RunJournal::open(root.clone()).unwrap();
                assert!(recovered.archive_recovery_pending().unwrap());
                assert_eq!(
                    recovered.load_run(&run.run_id).unwrap().status,
                    RunStatus::SettlementUnknown
                );
                assert_eq!(
                    file_bytes(&hashed_path(&root.join("recovery-runs"), &run.run_id)),
                    before
                );
                assert!(root
                    .join("recovery-runs")
                    .join(valid.file_name().unwrap())
                    .exists());
                continue;
            }
            assert!(RunJournal::open(root.clone()).is_err(), "{case}");
            assert!(valid.exists(), "{case}");
            assert_eq!(file_bytes(&run_path(&root, &run.run_id)), before, "{case}");
            if case != "count" {
                assert!(fs::symlink_metadata(unsafe_path).is_ok(), "{case}");
            }
        }
    }

    #[test]
    fn malformed_or_oversized_floor_preserves_runs_before_pruning() {
        let root = temp_root("request-floor-invalid");
        let journal = RunJournal::open(root.clone()).unwrap();
        let run = completed_run("legacy-expired-with-bad-floor", 1, 10, 1);
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let before = file_bytes(&path);
        for bytes in [
            b"invalid".to_vec(),
            vec![b'x'; MAX_REQUEST_FLOOR_BYTES as usize + 1],
        ] {
            let mut file = if journal.request_floor_path.exists() {
                fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&journal.request_floor_path)
                    .unwrap()
            } else {
                open_temp_file_owner_only(&journal.request_floor_path).unwrap()
            };
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
            assert!(journal.prune_expired_terminal_runs().is_err());
            assert!(RunJournal::open(root.clone()).is_err());
            assert_eq!(file_bytes(&path), before);
            assert_eq!(file_bytes(&journal.request_floor_path), bytes);
        }
    }

    #[test]
    fn committed_floor_precedes_run_deletion_and_preserves_replacement() {
        let root = temp_root("floor-before-exact-delete");
        let journal = RunJournal::open(root.clone()).unwrap();
        let issued = now_ms() - 400_000;
        let id = crate::test_support::fixture_model_request_id_at("replaced-prune", issued);
        let run = completed_run(&id, issued, issued + 10, 1);
        journal.store_run(&run).unwrap();
        let path = run_path(&root, &run.run_id);
        let held_original = root.join("original-run-fixture.json");
        assert!(
            delete_verified_run_file(&path, &root.join("runs"), &run.run_id, |run| {
                journal.record_pruned_issuance(&run.runtime_binding.request_id)?;
                assert_eq!(journal.pruned_issued_floor_ms()?, issued);
                fs::rename(&path, &held_original).unwrap();
                let mut replacement = open_temp_file_owner_only(&path).unwrap();
                replacement.write_all(b"replacement retained").unwrap();
                Ok(())
            })
            .is_err()
        );
        assert_eq!(file_bytes(&path), b"replacement retained");
        assert_eq!(journal.pruned_issued_floor_ms().unwrap(), issued);
        assert!(held_original.exists());
    }
}
