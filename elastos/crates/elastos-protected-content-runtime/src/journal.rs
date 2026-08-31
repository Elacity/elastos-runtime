use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, Digest32, RuntimeOperationIssuerKeyV1,
    RuntimeReleaseAuditIdV1, SignedNodeContributionV1, SignedNodeRightsDecisionV1,
    SignedRuntimeReleaseOperationV1,
};
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::geteuid;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

const STORE_MAGIC: &[u8; 8] = b"epc-ro02";
const STORE_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/runtime-release-journal/v2";
const SLOT_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/runtime-release-journal/slot/v1";
const STORE_LOCK_FILE: &str = "runtime-release-journal.lock";
const STORE_DIGEST_BYTES: usize = 32;
const STORE_HEADER_BYTES: usize = 8 + 32 + 32 + 32 + 32 + 1 + 4 + 4 + 4 + 4 + 4;
const MAX_WALLET_REQUEST_BYTES: usize = 64 * 1024;
const MAX_WALLET_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_SIGNED_OPERATION_BYTES: usize = 256 * 1024;
const MAX_TERMINAL_BYTES: usize = 256 * 1024;
const MAX_REPLAYABLE_RIGHTS_BYTES: usize = 256 * 1024;
const MAX_STORE_FILE_BYTES: usize = STORE_HEADER_BYTES
    + MAX_WALLET_REQUEST_BYTES
    + MAX_WALLET_RESPONSE_BYTES
    + MAX_SIGNED_OPERATION_BYTES
    + MAX_TERMINAL_BYTES
    + MAX_REPLAYABLE_RIGHTS_BYTES
    + STORE_DIGEST_BYTES;
const MAX_RUNTIME_RELEASE_RECORDS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeReleaseJournalError {
    #[error("runtime release journal is unavailable")]
    Unavailable,
    #[error("runtime release journal record is corrupt")]
    Corrupt,
    #[error("runtime release journal record conflicts with existing authority")]
    Conflict,
    #[error("runtime release journal capacity exceeded")]
    CapacityExceeded,
    #[error("runtime release terminal result is invalid")]
    InvalidTerminal,
    #[error("runtime release operation is not terminal")]
    NotTerminal,
    #[error("runtime release operation was not found")]
    NotFound,
}

impl From<ContractError> for RuntimeReleaseJournalError {
    fn from(_: ContractError) -> Self {
        Self::Corrupt
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeReleaseOperationDraft {
    audit_request_id: RuntimeReleaseAuditIdV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    signed_runtime_release_operation: SignedRuntimeReleaseOperationV1,
    wallet_request_bytes: Vec<u8>,
    wallet_response_bytes: Vec<u8>,
}

impl fmt::Debug for RuntimeReleaseOperationDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeReleaseOperationDraft")
            .field("audit_request_id", &self.audit_request_id)
            .field("runtime_operation_issuer", &self.runtime_operation_issuer)
            .field(
                "runtime_operation_hash",
                &self
                    .signed_runtime_release_operation
                    .statement()
                    .canonical_hash(),
            )
            .field("wallet_request_bytes", &"[redacted]")
            .field("wallet_response_bytes", &"[redacted]")
            .finish()
    }
}

impl RuntimeReleaseOperationDraft {
    pub fn new(
        wallet_request_bytes: Vec<u8>,
        wallet_response_bytes: Vec<u8>,
        signed_runtime_release_operation: SignedRuntimeReleaseOperationV1,
    ) -> Result<Self, RuntimeReleaseJournalError> {
        if wallet_request_bytes.is_empty()
            || wallet_request_bytes.len() > MAX_WALLET_REQUEST_BYTES
            || wallet_response_bytes.is_empty()
            || wallet_response_bytes.len() > MAX_WALLET_RESPONSE_BYTES
        {
            return Err(RuntimeReleaseJournalError::Corrupt);
        }
        let statement = signed_runtime_release_operation.statement();
        Ok(Self {
            audit_request_id: statement.audit_request_id(),
            runtime_operation_issuer: statement.runtime_operation_issuer(),
            signed_runtime_release_operation,
            wallet_request_bytes,
            wallet_response_bytes,
        })
    }

    pub const fn audit_request_id(&self) -> RuntimeReleaseAuditIdV1 {
        self.audit_request_id
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub fn signed_runtime_release_operation(&self) -> &SignedRuntimeReleaseOperationV1 {
        &self.signed_runtime_release_operation
    }

    pub fn wallet_request_bytes(&self) -> &[u8] {
        &self.wallet_request_bytes
    }

    pub fn wallet_response_bytes(&self) -> &[u8] {
        &self.wallet_response_bytes
    }

    pub fn operation_hash(&self) -> Result<Digest32, RuntimeReleaseJournalError> {
        Ok(self.signed_runtime_release_operation.canonical_hash()?)
    }

    fn signed_operation_bytes(&self) -> Result<Vec<u8>, RuntimeReleaseJournalError> {
        let bytes = self.signed_runtime_release_operation.canonical_bytes()?;
        if bytes.is_empty() || bytes.len() > MAX_SIGNED_OPERATION_BYTES {
            return Err(RuntimeReleaseJournalError::Corrupt);
        }
        Ok(bytes)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RuntimeReleaseTerminalResult {
    RightsDenied {
        signed_node_rights_decision: Box<SignedNodeRightsDecisionV1>,
    },
    ContributionsReady {
        signed_node_contributions: Vec<SignedNodeContributionV1>,
    },
}

impl fmt::Debug for RuntimeReleaseTerminalResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RightsDenied { .. } => formatter.write_str("RightsDenied"),
            Self::ContributionsReady {
                signed_node_contributions,
            } => formatter
                .debug_struct("ContributionsReady")
                .field("contribution_count", &signed_node_contributions.len())
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeReleaseAuditPhase {
    Unresolved { provider_effect_started: bool },
    TerminalRightsDenied,
    TerminalContributionsReady { contribution_count: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReleaseAuditRecord {
    operation_hash: Digest32,
    audit_request_id: RuntimeReleaseAuditIdV1,
    phase: RuntimeReleaseAuditPhase,
}

impl RuntimeReleaseAuditRecord {
    pub const fn operation_hash(&self) -> Digest32 {
        self.operation_hash
    }

    pub const fn audit_request_id(&self) -> RuntimeReleaseAuditIdV1 {
        self.audit_request_id
    }

    pub const fn phase(&self) -> RuntimeReleaseAuditPhase {
        self.phase
    }

    pub fn reason(&self) -> String {
        let operation_hash = hex::encode(self.operation_hash.as_bytes());
        let audit_request_id = hex::encode(self.audit_request_id.digest().as_bytes());
        match self.phase {
            RuntimeReleaseAuditPhase::Unresolved {
                provider_effect_started,
            } => format!(
                "protected-content.runtime.release operation_hash={operation_hash} audit_request_id={audit_request_id} phase=unresolved provider_effect_started={provider_effect_started}"
            ),
            RuntimeReleaseAuditPhase::TerminalRightsDenied => format!(
                "protected-content.runtime.release operation_hash={operation_hash} audit_request_id={audit_request_id} phase=terminal_rights_denied"
            ),
            RuntimeReleaseAuditPhase::TerminalContributionsReady { contribution_count } => format!(
                "protected-content.runtime.release operation_hash={operation_hash} audit_request_id={audit_request_id} phase=terminal_contributions_ready contribution_count={contribution_count}"
            ),
        }
    }
}

impl RuntimeReleaseTerminalResult {
    pub fn contribution_count(&self) -> usize {
        match self {
            Self::RightsDenied { .. } => 0,
            Self::ContributionsReady {
                signed_node_contributions,
            } => signed_node_contributions.len(),
        }
    }

    fn encode(&self) -> Result<Vec<u8>, RuntimeReleaseJournalError> {
        let mut bytes = Vec::new();
        match self {
            Self::RightsDenied {
                signed_node_rights_decision,
            } => {
                bytes.push(1);
                push_blob(&mut bytes, signed_node_rights_decision.canonical_bytes()?)?;
            }
            Self::ContributionsReady {
                signed_node_contributions,
            } => {
                if signed_node_contributions.is_empty() {
                    return Err(RuntimeReleaseJournalError::InvalidTerminal);
                }
                bytes.push(2);
                let count = u16::try_from(signed_node_contributions.len())
                    .map_err(|_| RuntimeReleaseJournalError::InvalidTerminal)?;
                bytes.extend_from_slice(&count.to_be_bytes());
                for contribution in signed_node_contributions {
                    push_blob(&mut bytes, contribution.canonical_bytes()?)?;
                }
            }
        }
        if bytes.len() > MAX_TERMINAL_BYTES {
            return Err(RuntimeReleaseJournalError::InvalidTerminal);
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, RuntimeReleaseJournalError> {
        if bytes.is_empty() || bytes.len() > MAX_TERMINAL_BYTES {
            return Err(RuntimeReleaseJournalError::Corrupt);
        }
        match bytes[0] {
            1 => {
                let mut cursor = 1usize;
                let decision_bytes = take_blob(bytes, &mut cursor)?;
                if cursor != bytes.len() {
                    return Err(RuntimeReleaseJournalError::Corrupt);
                }
                Ok(Self::RightsDenied {
                    signed_node_rights_decision: Box::new(
                        SignedNodeRightsDecisionV1::from_canonical_bytes(decision_bytes)?,
                    ),
                })
            }
            2 => {
                if bytes.len() < 3 {
                    return Err(RuntimeReleaseJournalError::Corrupt);
                }
                let count = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                if count == 0 {
                    return Err(RuntimeReleaseJournalError::Corrupt);
                }
                let mut cursor = 3usize;
                let mut signed_node_contributions = Vec::with_capacity(count);
                for _ in 0..count {
                    signed_node_contributions.push(SignedNodeContributionV1::from_canonical_bytes(
                        take_blob(bytes, &mut cursor)?,
                    )?);
                }
                if cursor != bytes.len() {
                    return Err(RuntimeReleaseJournalError::Corrupt);
                }
                Ok(Self::ContributionsReady {
                    signed_node_contributions,
                })
            }
            _ => Err(RuntimeReleaseJournalError::Corrupt),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRuntimeReleaseOperation {
    draft: RuntimeReleaseOperationDraft,
    terminal_result: Option<RuntimeReleaseTerminalResult>,
    replayable_rights_decisions: Vec<SignedNodeRightsDecisionV1>,
    provider_effect_started: bool,
}

impl PersistedRuntimeReleaseOperation {
    pub fn draft(&self) -> &RuntimeReleaseOperationDraft {
        &self.draft
    }

    pub fn terminal_result(&self) -> Option<&RuntimeReleaseTerminalResult> {
        self.terminal_result.as_ref()
    }

    pub fn replayable_rights_decisions(&self) -> &[SignedNodeRightsDecisionV1] {
        &self.replayable_rights_decisions
    }

    pub const fn provider_effect_started(&self) -> bool {
        self.provider_effect_started
    }

    pub fn audit_record(&self) -> Result<RuntimeReleaseAuditRecord, RuntimeReleaseJournalError> {
        let phase = match self.terminal_result.as_ref() {
            Some(RuntimeReleaseTerminalResult::RightsDenied { .. }) => {
                RuntimeReleaseAuditPhase::TerminalRightsDenied
            }
            Some(RuntimeReleaseTerminalResult::ContributionsReady {
                signed_node_contributions,
            }) => RuntimeReleaseAuditPhase::TerminalContributionsReady {
                contribution_count: u16::try_from(signed_node_contributions.len())
                    .map_err(|_| RuntimeReleaseJournalError::InvalidTerminal)?,
            },
            None => RuntimeReleaseAuditPhase::Unresolved {
                provider_effect_started: self.provider_effect_started,
            },
        };
        Ok(RuntimeReleaseAuditRecord {
            operation_hash: self.draft.operation_hash()?,
            audit_request_id: self.draft.audit_request_id(),
            phase,
        })
    }

    pub fn into_terminal_result(
        self,
    ) -> Result<RuntimeReleaseTerminalResult, RuntimeReleaseJournalError> {
        self.terminal_result
            .ok_or(RuntimeReleaseJournalError::NotTerminal)
    }
}

#[derive(Clone)]
pub struct RuntimeReleaseJournal {
    root_dir: PathBuf,
    lock_path: PathBuf,
}

impl fmt::Debug for RuntimeReleaseJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeReleaseJournal")
            .field("root_dir", &"[redacted]")
            .field("lock_path", &"[redacted]")
            .finish()
    }
}

impl RuntimeReleaseJournal {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            lock_path: root_dir.join(STORE_LOCK_FILE),
            root_dir,
        }
    }

    pub fn persist_before_provider_effect(
        &self,
        draft: &RuntimeReleaseOperationDraft,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let entry = StoreEntry::from_draft(draft, None, false, Vec::new())?;
        self.write_or_replay(entry)
    }

    pub fn mark_provider_effect_started(
        &self,
        draft: &RuntimeReleaseOperationDraft,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let path = self.record_path(slot_hash_for(draft.operation_hash()?));
        let existing = read_entry_from_path(
            &path,
            slot_hash_for(draft.operation_hash()?),
            draft.operation_hash()?,
        )?;
        let expected = StoreEntry::from_draft(
            draft,
            existing.terminal_bytes.clone(),
            existing.effect_started,
            existing.replayable_rights_bytes.clone(),
        )?;
        if existing != expected {
            return Err(RuntimeReleaseJournalError::Conflict);
        }
        if existing.effect_started || existing.terminal_bytes.is_some() {
            return existing.into_persisted();
        }
        let started =
            StoreEntry::from_draft(draft, None, true, existing.replayable_rights_bytes.clone())?;
        let bytes = encode_entry(&started)?;
        self.write_replace_entry(started.slot_hash, &bytes)?;
        started.into_persisted()
    }

    pub fn persist_replayable_rights_decisions(
        &self,
        draft: &RuntimeReleaseOperationDraft,
        replayable_rights_decisions: &[SignedNodeRightsDecisionV1],
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let path = self.record_path(slot_hash_for(draft.operation_hash()?));
        let existing = read_entry_from_path(
            &path,
            slot_hash_for(draft.operation_hash()?),
            draft.operation_hash()?,
        )?;
        let expected = StoreEntry::from_draft(
            draft,
            existing.terminal_bytes.clone(),
            existing.effect_started,
            existing.replayable_rights_bytes.clone(),
        )?;
        if existing != expected {
            return Err(RuntimeReleaseJournalError::Conflict);
        }
        if existing.terminal_bytes.is_some() {
            return existing.into_persisted();
        }
        if !existing.effect_started {
            return Err(RuntimeReleaseJournalError::Conflict);
        }
        let merged = merge_replayable_rights_decisions(
            &decode_replayable_rights_decisions(&existing.replayable_rights_bytes)?,
            replayable_rights_decisions,
        )?;
        let updated = StoreEntry::from_draft(
            draft,
            None,
            true,
            encode_replayable_rights_decisions(&merged)?,
        )?;
        if updated == existing {
            return existing.into_persisted();
        }
        let bytes = encode_entry(&updated)?;
        self.write_replace_entry(updated.slot_hash, &bytes)?;
        updated.into_persisted()
    }

    pub fn load(
        &self,
        operation_hash: Digest32,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let slot_hash = slot_hash_for(operation_hash);
        let entry = read_entry_from_path(&self.record_path(slot_hash), slot_hash, operation_hash)?;
        entry.into_persisted()
    }

    /// List durable operations that have no terminal result.
    ///
    /// This scan removes leftover temp files only. It does not settle, expire,
    /// replay, or infer completion from missing providers or elapsed time.
    pub fn list_unresolved(
        &self,
    ) -> Result<Vec<PersistedRuntimeReleaseOperation>, RuntimeReleaseJournalError> {
        match fs::symlink_metadata(&self.root_dir) {
            Ok(metadata) => validate_owner_only_directory_metadata(&metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(RuntimeReleaseJournalError::Unavailable),
        }
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let mut unresolved = Vec::new();
        for dir_entry in
            fs::read_dir(&self.root_dir).map_err(|_| RuntimeReleaseJournalError::Unavailable)?
        {
            let dir_entry = dir_entry.map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            let name = dir_entry
                .file_name()
                .into_string()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            if name == STORE_LOCK_FILE {
                continue;
            }
            if !is_record_name(&name) {
                return Err(RuntimeReleaseJournalError::Unavailable);
            }
            let slot_hash =
                digest_from_record_name(&name).ok_or(RuntimeReleaseJournalError::Corrupt)?;
            let entry =
                read_entry_from_path_for_slot_with_link_count(&dir_entry.path(), slot_hash, 1)?;
            if entry.terminal_bytes.is_some() {
                continue;
            }
            unresolved.push(entry);
        }
        unresolved.sort_by(|left, right| {
            left.operation_hash
                .as_bytes()
                .cmp(right.operation_hash.as_bytes())
        });
        unresolved
            .into_iter()
            .map(StoreEntry::into_persisted)
            .collect()
    }

    pub fn mark_terminal(
        &self,
        draft: &RuntimeReleaseOperationDraft,
        terminal_result: RuntimeReleaseTerminalResult,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        self.ensure_root_dir()?;
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.cleanup_stale_temps()?;
        let path = self.record_path(slot_hash_for(draft.operation_hash()?));
        let existing = read_entry_from_path(
            &path,
            slot_hash_for(draft.operation_hash()?),
            draft.operation_hash()?,
        )?;
        let expected = StoreEntry::from_draft(
            draft,
            existing.terminal_bytes.clone(),
            existing.effect_started,
            existing.replayable_rights_bytes.clone(),
        )?;
        if existing != expected {
            return Err(RuntimeReleaseJournalError::Conflict);
        }
        if existing.terminal_bytes.is_some() {
            if existing.terminal_bytes.as_deref() != Some(terminal_result.encode()?.as_slice()) {
                return Err(RuntimeReleaseJournalError::Conflict);
            }
            return existing.into_persisted();
        }
        let terminal_entry =
            StoreEntry::from_draft(draft, Some(terminal_result.encode()?), true, Vec::new())?;
        let bytes = encode_entry(&terminal_entry)?;
        self.write_replace_entry(terminal_entry.slot_hash, &bytes)?;
        terminal_entry.into_persisted()
    }

    fn write_or_replay(
        &self,
        entry: StoreEntry,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        let path = self.record_path(entry.slot_hash);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_owner_only_regular_file_metadata(&metadata)?;
                let existing = read_entry_from_path(&path, entry.slot_hash, entry.operation_hash)?;
                let expected = StoreEntry {
                    terminal_bytes: existing.terminal_bytes.clone(),
                    effect_started: existing.effect_started,
                    ..entry
                };
                if existing == expected {
                    return existing.into_persisted();
                }
                return Err(RuntimeReleaseJournalError::Conflict);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(RuntimeReleaseJournalError::Unavailable),
        }

        if self.record_count()? >= MAX_RUNTIME_RELEASE_RECORDS {
            return Err(RuntimeReleaseJournalError::CapacityExceeded);
        }

        let bytes = encode_entry(&entry)?;
        self.write_new_entry(entry.slot_hash, &bytes)?;
        entry.into_persisted()
    }

    fn ensure_root_dir(&self) -> Result<(), RuntimeReleaseJournalError> {
        let parent = self
            .root_dir
            .parent()
            .ok_or(RuntimeReleaseJournalError::Unavailable)?;
        validate_owner_only_directory(parent)?;
        match fs::symlink_metadata(&self.root_dir) {
            Ok(metadata) => validate_owner_only_directory_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_owner_only_directory(&self.root_dir)?;
                sync_directory(parent)?;
                validate_owner_only_directory(&self.root_dir)
            }
            Err(_) => Err(RuntimeReleaseJournalError::Unavailable),
        }
    }

    fn cleanup_stale_temps(&self) -> Result<(), RuntimeReleaseJournalError> {
        let mut removed = false;
        for entry in
            fs::read_dir(&self.root_dir).map_err(|_| RuntimeReleaseJournalError::Unavailable)?
        {
            let entry = entry.map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            if name == STORE_LOCK_FILE || is_record_name(&name) {
                continue;
            }
            if is_temp_name(&name) {
                self.cleanup_temp_entry(&name, &entry.path())?;
                removed = true;
                continue;
            }
            return Err(RuntimeReleaseJournalError::Unavailable);
        }
        if removed {
            sync_directory(&self.root_dir)
        } else {
            Ok(())
        }
    }

    fn cleanup_temp_entry(
        &self,
        name: &str,
        temp_path: &Path,
    ) -> Result<(), RuntimeReleaseJournalError> {
        let slot_hash =
            slot_hash_from_temp_name(name).ok_or(RuntimeReleaseJournalError::Unavailable)?;
        let temp_metadata =
            fs::symlink_metadata(temp_path).map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        let record_path = self.record_path(slot_hash);
        match fs::symlink_metadata(&record_path) {
            Ok(record_metadata) => {
                validate_owner_only_regular_file_metadata_with_link_count(&temp_metadata, 2)?;
                validate_owner_only_regular_file_metadata_with_link_count(&record_metadata, 2)?;
                require_same_file(&temp_metadata, &record_metadata)?;
                let _entry =
                    read_entry_from_path_for_slot_with_link_count(&record_path, slot_hash, 2)?;
                fs::remove_file(temp_path).map_err(|_| RuntimeReleaseJournalError::Unavailable)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                validate_owner_only_regular_file_metadata(&temp_metadata)?;
                fs::remove_file(temp_path).map_err(|_| RuntimeReleaseJournalError::Unavailable)
            }
            Err(_) => Err(RuntimeReleaseJournalError::Unavailable),
        }
    }

    fn record_count(&self) -> Result<usize, RuntimeReleaseJournalError> {
        let mut count = 0usize;
        for entry in
            fs::read_dir(&self.root_dir).map_err(|_| RuntimeReleaseJournalError::Unavailable)?
        {
            let entry = entry.map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            if name == STORE_LOCK_FILE {
                continue;
            }
            if !is_record_name(&name) {
                return Err(RuntimeReleaseJournalError::Unavailable);
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
            validate_owner_only_regular_file_metadata(&metadata)?;
            count = count
                .checked_add(1)
                .ok_or(RuntimeReleaseJournalError::CapacityExceeded)?;
        }
        Ok(count)
    }

    fn write_new_entry(
        &self,
        slot_hash: Digest32,
        bytes: &[u8],
    ) -> Result<(), RuntimeReleaseJournalError> {
        let temp_path = self.temp_path(slot_hash);
        remove_temp_file_if_present(&temp_path)?;
        let mut temp_file = open_owner_only_temp_file_for_write(&temp_path)?;
        temp_file
            .write_all(bytes)
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        temp_file
            .sync_all()
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        validate_owner_only_regular_file_metadata(
            &temp_file
                .metadata()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?,
        )?;
        fs::hard_link(&temp_path, self.record_path(slot_hash)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RuntimeReleaseJournalError::Conflict
            } else {
                RuntimeReleaseJournalError::Unavailable
            }
        })?;
        sync_directory(&self.root_dir)?;
        fs::remove_file(&temp_path).map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        sync_directory(&self.root_dir)
    }

    fn write_replace_entry(
        &self,
        slot_hash: Digest32,
        bytes: &[u8],
    ) -> Result<(), RuntimeReleaseJournalError> {
        let temp_path = self.temp_path(slot_hash);
        remove_temp_file_if_present(&temp_path)?;
        let mut temp_file = open_owner_only_temp_file_for_write(&temp_path)?;
        temp_file
            .write_all(bytes)
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        temp_file
            .sync_all()
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        validate_owner_only_regular_file_metadata(
            &temp_file
                .metadata()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?,
        )?;
        fs::rename(&temp_path, self.record_path(slot_hash))
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        sync_directory(&self.root_dir)
    }

    fn record_path(&self, slot_hash: Digest32) -> PathBuf {
        self.root_dir.join(lower_hex_digest(slot_hash))
    }

    fn temp_path(&self, slot_hash: Digest32) -> PathBuf {
        self.root_dir
            .join(format!(".{}.tmp", lower_hex_digest(slot_hash)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreEntry {
    slot_hash: Digest32,
    audit_request_id: RuntimeReleaseAuditIdV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    operation_hash: Digest32,
    wallet_request_bytes: Vec<u8>,
    wallet_response_bytes: Vec<u8>,
    signed_operation_bytes: Vec<u8>,
    terminal_bytes: Option<Vec<u8>>,
    replayable_rights_bytes: Vec<u8>,
    effect_started: bool,
}

impl StoreEntry {
    fn from_draft(
        draft: &RuntimeReleaseOperationDraft,
        terminal_bytes: Option<Vec<u8>>,
        effect_started: bool,
        replayable_rights_bytes: Vec<u8>,
    ) -> Result<Self, RuntimeReleaseJournalError> {
        let operation_hash = draft.operation_hash()?;
        Ok(Self {
            slot_hash: slot_hash_for(operation_hash),
            audit_request_id: draft.audit_request_id(),
            runtime_operation_issuer: draft.runtime_operation_issuer(),
            operation_hash,
            wallet_request_bytes: draft.wallet_request_bytes().to_vec(),
            wallet_response_bytes: draft.wallet_response_bytes().to_vec(),
            signed_operation_bytes: draft.signed_operation_bytes()?,
            terminal_bytes,
            replayable_rights_bytes,
            effect_started,
        })
    }

    fn into_persisted(
        self,
    ) -> Result<PersistedRuntimeReleaseOperation, RuntimeReleaseJournalError> {
        let signed_runtime_release_operation =
            SignedRuntimeReleaseOperationV1::from_canonical_bytes(&self.signed_operation_bytes)?;
        if signed_runtime_release_operation.canonical_hash()? != self.operation_hash {
            return Err(RuntimeReleaseJournalError::Corrupt);
        }
        if signed_runtime_release_operation
            .statement()
            .audit_request_id()
            != self.audit_request_id
            || signed_runtime_release_operation
                .statement()
                .runtime_operation_issuer()
                != self.runtime_operation_issuer
        {
            return Err(RuntimeReleaseJournalError::Corrupt);
        }
        let draft = RuntimeReleaseOperationDraft::new(
            self.wallet_request_bytes,
            self.wallet_response_bytes,
            signed_runtime_release_operation,
        )?;
        let terminal_result = self
            .terminal_bytes
            .as_deref()
            .map(RuntimeReleaseTerminalResult::decode)
            .transpose()?;
        let replayable_rights_decisions =
            decode_replayable_rights_decisions(&self.replayable_rights_bytes)?;
        Ok(PersistedRuntimeReleaseOperation {
            draft,
            terminal_result,
            replayable_rights_decisions,
            provider_effect_started: self.effect_started,
        })
    }
}

fn push_blob(target: &mut Vec<u8>, bytes: Vec<u8>) -> Result<(), RuntimeReleaseJournalError> {
    if bytes.is_empty() || bytes.len() > u32::MAX as usize {
        return Err(RuntimeReleaseJournalError::InvalidTerminal);
    }
    target.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    target.extend_from_slice(&bytes);
    Ok(())
}

fn encode_replayable_rights_decisions(
    replayable_rights_decisions: &[SignedNodeRightsDecisionV1],
) -> Result<Vec<u8>, RuntimeReleaseJournalError> {
    if replayable_rights_decisions.is_empty() {
        return Ok(Vec::new());
    }
    let count = u16::try_from(replayable_rights_decisions.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&count.to_be_bytes());
    for decision in replayable_rights_decisions {
        push_blob(&mut bytes, decision.canonical_bytes()?)?;
    }
    if bytes.len() > MAX_REPLAYABLE_RIGHTS_BYTES {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    Ok(bytes)
}

fn decode_replayable_rights_decisions(
    bytes: &[u8],
) -> Result<Vec<SignedNodeRightsDecisionV1>, RuntimeReleaseJournalError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() < 2 || bytes.len() > MAX_REPLAYABLE_RIGHTS_BYTES {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let count = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    if count == 0 {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let mut cursor = 2usize;
    let mut decisions = Vec::with_capacity(count);
    for _ in 0..count {
        decisions.push(SignedNodeRightsDecisionV1::from_canonical_bytes(
            take_blob(bytes, &mut cursor)?,
        )?);
    }
    if cursor != bytes.len() {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    Ok(decisions)
}

fn merge_replayable_rights_decisions(
    existing: &[SignedNodeRightsDecisionV1],
    additional: &[SignedNodeRightsDecisionV1],
) -> Result<Vec<SignedNodeRightsDecisionV1>, RuntimeReleaseJournalError> {
    let mut merged = existing.to_vec();
    for decision in additional {
        let node_public_key = decision.statement().node_public_key();
        if let Some(existing_decision) = merged.iter().find(|existing_decision| {
            existing_decision.statement().node_public_key() == node_public_key
        }) {
            if existing_decision != decision {
                return Err(RuntimeReleaseJournalError::Conflict);
            }
            continue;
        }
        merged.push(decision.clone());
    }
    merged.sort_by(|left, right| {
        left.statement()
            .node_public_key()
            .as_bytes()
            .cmp(right.statement().node_public_key().as_bytes())
    });
    Ok(merged)
}

fn take_blob<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], RuntimeReleaseJournalError> {
    if bytes.len().saturating_sub(*cursor) < 4 {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let len = u32::from_be_bytes(
        bytes[*cursor..*cursor + 4]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    *cursor += 4;
    if len == 0 || bytes.len().saturating_sub(*cursor) < len {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let start = *cursor;
    *cursor += len;
    Ok(&bytes[start..*cursor])
}

fn slot_hash_for(operation_hash: Digest32) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(SLOT_DIGEST_DOMAIN);
    hasher.update(operation_hash.as_bytes());
    Digest32::new(hasher.finalize().into())
}

fn encode_entry(entry: &StoreEntry) -> Result<Vec<u8>, RuntimeReleaseJournalError> {
    if entry.wallet_request_bytes.is_empty()
        || entry.wallet_request_bytes.len() > MAX_WALLET_REQUEST_BYTES
        || entry.wallet_response_bytes.is_empty()
        || entry.wallet_response_bytes.len() > MAX_WALLET_RESPONSE_BYTES
        || entry.signed_operation_bytes.is_empty()
        || entry.signed_operation_bytes.len() > MAX_SIGNED_OPERATION_BYTES
        || entry.replayable_rights_bytes.len() > MAX_REPLAYABLE_RIGHTS_BYTES
    {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    let terminal_bytes = entry.terminal_bytes.as_deref().unwrap_or_default();
    if terminal_bytes.len() > MAX_TERMINAL_BYTES {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    let wallet_len = u32::try_from(entry.wallet_request_bytes.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let wallet_response_len = u32::try_from(entry.wallet_response_bytes.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let operation_len = u32::try_from(entry.signed_operation_bytes.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let terminal_len =
        u32::try_from(terminal_bytes.len()).map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let replayable_rights_len = u32::try_from(entry.replayable_rights_bytes.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    let mut payload = Vec::with_capacity(
        STORE_HEADER_BYTES
            + entry.wallet_request_bytes.len()
            + entry.wallet_response_bytes.len()
            + entry.signed_operation_bytes.len()
            + terminal_bytes.len(),
    );
    payload.extend_from_slice(STORE_MAGIC);
    payload.extend_from_slice(entry.slot_hash.as_bytes());
    payload.extend_from_slice(entry.audit_request_id.digest().as_bytes());
    payload.extend_from_slice(entry.runtime_operation_issuer.as_bytes());
    payload.extend_from_slice(entry.operation_hash.as_bytes());
    payload.push(u8::from(entry.effect_started));
    payload.extend_from_slice(&wallet_len.to_be_bytes());
    payload.extend_from_slice(&wallet_response_len.to_be_bytes());
    payload.extend_from_slice(&operation_len.to_be_bytes());
    payload.extend_from_slice(&terminal_len.to_be_bytes());
    payload.extend_from_slice(&replayable_rights_len.to_be_bytes());
    payload.extend_from_slice(&entry.wallet_request_bytes);
    payload.extend_from_slice(&entry.wallet_response_bytes);
    payload.extend_from_slice(&entry.signed_operation_bytes);
    payload.extend_from_slice(terminal_bytes);
    payload.extend_from_slice(&entry.replayable_rights_bytes);
    if payload.len() + STORE_DIGEST_BYTES > MAX_STORE_FILE_BYTES {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    let digest = store_integrity_digest(&payload);
    let mut bytes = payload;
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

fn decode_entry_for_slot(
    bytes: &[u8],
    expected_slot_hash: Digest32,
) -> Result<StoreEntry, RuntimeReleaseJournalError> {
    if bytes.len() < STORE_HEADER_BYTES + STORE_DIGEST_BYTES || bytes.len() > MAX_STORE_FILE_BYTES {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let (payload, digest) = bytes.split_at(bytes.len() - STORE_DIGEST_BYTES);
    if payload[..8] != *STORE_MAGIC {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    if store_integrity_digest(payload).as_slice() != digest {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let slot_hash = Digest32::new(
        payload[8..40]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    );
    if slot_hash != expected_slot_hash {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let audit_request_id = RuntimeReleaseAuditIdV1::new(Digest32::new(
        payload[40..72]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ))?;
    let runtime_operation_issuer = RuntimeOperationIssuerKeyV1::new(
        payload[72..104]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    )?;
    let operation_hash = Digest32::new(
        payload[104..136]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    );
    if slot_hash_for(operation_hash) != expected_slot_hash {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let effect_started = match payload[136] {
        0 => false,
        1 => true,
        _ => return Err(RuntimeReleaseJournalError::Corrupt),
    };
    let wallet_len = u32::from_be_bytes(
        payload[137..141]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    let wallet_response_len = u32::from_be_bytes(
        payload[141..145]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    let operation_len = u32::from_be_bytes(
        payload[145..149]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    let terminal_len = u32::from_be_bytes(
        payload[149..153]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    let replayable_rights_len = u32::from_be_bytes(
        payload[153..157]
            .try_into()
            .map_err(|_| RuntimeReleaseJournalError::Corrupt)?,
    ) as usize;
    if wallet_len == 0
        || wallet_len > MAX_WALLET_REQUEST_BYTES
        || wallet_response_len == 0
        || wallet_response_len > MAX_WALLET_RESPONSE_BYTES
        || operation_len == 0
        || operation_len > MAX_SIGNED_OPERATION_BYTES
        || terminal_len > MAX_TERMINAL_BYTES
        || replayable_rights_len > MAX_REPLAYABLE_RIGHTS_BYTES
    {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let expected_len = STORE_HEADER_BYTES
        .checked_add(wallet_len)
        .and_then(|value| value.checked_add(wallet_response_len))
        .and_then(|value| value.checked_add(operation_len))
        .and_then(|value| value.checked_add(terminal_len))
        .and_then(|value| value.checked_add(replayable_rights_len))
        .ok_or(RuntimeReleaseJournalError::Corrupt)?;
    if payload.len() != expected_len {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let wallet_start = STORE_HEADER_BYTES;
    let wallet_response_start = wallet_start + wallet_len;
    let operation_start = wallet_response_start + wallet_response_len;
    let terminal_start = operation_start + operation_len;
    let replayable_rights_start = terminal_start + terminal_len;
    Ok(StoreEntry {
        slot_hash,
        audit_request_id,
        runtime_operation_issuer,
        operation_hash,
        wallet_request_bytes: payload[wallet_start..wallet_response_start].to_vec(),
        wallet_response_bytes: payload[wallet_response_start..operation_start].to_vec(),
        signed_operation_bytes: payload[operation_start..terminal_start].to_vec(),
        terminal_bytes: (terminal_len > 0)
            .then(|| payload[terminal_start..replayable_rights_start].to_vec()),
        replayable_rights_bytes: payload[replayable_rights_start..].to_vec(),
        effect_started,
    })
}

fn store_integrity_digest(payload: &[u8]) -> [u8; STORE_DIGEST_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(STORE_DIGEST_DOMAIN);
    hasher.update(payload);
    hasher.finalize().into()
}

fn read_entry_from_path(
    path: &Path,
    expected_slot_hash: Digest32,
    expected_operation_hash: Digest32,
) -> Result<StoreEntry, RuntimeReleaseJournalError> {
    let entry = read_entry_from_path_for_slot_with_link_count(path, expected_slot_hash, 1)?;
    if entry.operation_hash != expected_operation_hash {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    Ok(entry)
}

fn read_entry_from_path_for_slot_with_link_count(
    path: &Path,
    expected_slot_hash: Digest32,
    expected_link_count: u64,
) -> Result<StoreEntry, RuntimeReleaseJournalError> {
    let pre_metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => RuntimeReleaseJournalError::NotFound,
        _ => RuntimeReleaseJournalError::Unavailable,
    })?;
    validate_owner_only_regular_file_metadata_with_link_count(&pre_metadata, expected_link_count)?;
    let file = open_owner_only_file_for_read(path)?;
    let open_metadata = file
        .metadata()
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    validate_owner_only_regular_file_metadata_with_link_count(&open_metadata, expected_link_count)?;
    require_same_file(&pre_metadata, &open_metadata)?;
    let metadata_len = usize::try_from(open_metadata.len())
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    if metadata_len > MAX_STORE_FILE_BYTES {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    let mut bytes = Vec::with_capacity(metadata_len);
    file.take((MAX_STORE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    if bytes.len() > MAX_STORE_FILE_BYTES {
        return Err(RuntimeReleaseJournalError::Corrupt);
    }
    decode_entry_for_slot(&bytes, expected_slot_hash)
}

fn create_owner_only_directory(path: &Path) -> Result<(), RuntimeReleaseJournalError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(RuntimeReleaseJournalError::Unavailable),
    }
}

fn validate_owner_only_directory(path: &Path) -> Result<(), RuntimeReleaseJournalError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    validate_owner_only_directory_metadata(&metadata)
}

fn validate_owner_only_directory_metadata(
    metadata: &fs::Metadata,
) -> Result<(), RuntimeReleaseJournalError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    validate_owner_and_mode(metadata, 0o700)
}

fn validate_owner_only_regular_file_metadata(
    metadata: &fs::Metadata,
) -> Result<(), RuntimeReleaseJournalError> {
    validate_owner_only_regular_file_metadata_with_link_count(metadata, 1)
}

fn validate_owner_only_regular_file_metadata_with_link_count(
    metadata: &fs::Metadata,
    expected_link_count: u64,
) -> Result<(), RuntimeReleaseJournalError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RuntimeReleaseJournalError::Unavailable);
    }
    validate_owner_and_mode(metadata, 0o600)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != expected_link_count {
            return Err(RuntimeReleaseJournalError::Unavailable);
        }
    }
    Ok(())
}

fn validate_owner_and_mode(
    metadata: &fs::Metadata,
    exact_mode: u32,
) -> Result<(), RuntimeReleaseJournalError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o777 != exact_mode {
            return Err(RuntimeReleaseJournalError::Unavailable);
        }
    }
    Ok(())
}

fn require_same_file(
    pre_metadata: &fs::Metadata,
    open_metadata: &fs::Metadata,
) -> Result<(), RuntimeReleaseJournalError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if pre_metadata.dev() != open_metadata.dev()
            || pre_metadata.ino() != open_metadata.ino()
            || pre_metadata.len() != open_metadata.len()
        {
            return Err(RuntimeReleaseJournalError::Unavailable);
        }
    }
    Ok(())
}

fn open_owner_only_file_for_read(path: &Path) -> Result<File, RuntimeReleaseJournalError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
    }
    options
        .open(path)
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)
}

fn open_owner_only_temp_file_for_write(path: &Path) -> Result<File, RuntimeReleaseJournalError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
    }
    options
        .open(path)
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)
}

fn sync_directory(path: &Path) -> Result<(), RuntimeReleaseJournalError> {
    let dir = File::open(path).map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
    dir.sync_all()
        .map_err(|_| RuntimeReleaseJournalError::Unavailable)
}

fn remove_temp_file_if_present(path: &Path) -> Result<(), RuntimeReleaseJournalError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_owner_only_regular_file_metadata(&metadata)?;
            fs::remove_file(path).map_err(|_| RuntimeReleaseJournalError::Unavailable)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RuntimeReleaseJournalError::Unavailable),
    }
}

fn is_record_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn digest_from_record_name(name: &str) -> Option<Digest32> {
    if !is_record_name(name) {
        return None;
    }
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(name, &mut bytes).ok()?;
    Some(Digest32::new(bytes))
}

fn is_temp_name(name: &str) -> bool {
    name.len() == 69
        && name.starts_with('.')
        && name.ends_with(".tmp")
        && is_record_name(&name[1..65])
}

fn slot_hash_from_temp_name(name: &str) -> Option<Digest32> {
    if !is_temp_name(name) {
        return None;
    }
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(&name[1..65], &mut bytes).ok()?;
    Some(Digest32::new(bytes))
}

fn lower_hex_digest(digest: Digest32) -> String {
    hex::encode(digest.as_bytes())
}

struct ExclusiveFileLock {
    _lock: Flock<File>,
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> Result<Self, RuntimeReleaseJournalError> {
        if let Some(parent) = path.parent() {
            validate_owner_only_directory(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            options.custom_flags(nix::libc::O_CLOEXEC);
        }
        let file = options
            .open(path)
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        validate_owner_only_regular_file_metadata(
            &file
                .metadata()
                .map_err(|_| RuntimeReleaseJournalError::Unavailable)?,
        )?;
        let lock = Flock::lock(file, FlockArg::LockExclusive)
            .map_err(|_| RuntimeReleaseJournalError::Unavailable)?;
        Ok(Self { _lock: lock })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_auth::ethereum_signed_message_hash;
    use elastos_protected_content_contracts::{
        ContentAccessIdV1, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
        CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1,
        CustodyEpochStatementV1, CustodyNodeIdentityV1, CustodyPoolIdentityV1,
        EncryptedContentIdentityV1, EvmContractAddressV1, EvmFunctionSelectorV1,
        EvmRightsMethodAbiV1, KeyReleaseRequestV1, NodeContributionStatementV1,
        NodeCustodyPublicKeyV1, NodePublicKey, PqHybridSealedShareV1,
        RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1, RecipientPublicKeyBytesV1,
        RecipientSealedContributionV1, ReplayNonce16, RightsActionV1, RightsDecisionV1,
        RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
        RightsRequestV1, RightsSubjectSourceV1, RuntimeReleaseOperationStatementV1,
        RuntimeSessionBindingV1, ShareCoordinateV1, SignedCustodyEpochV1, SignedNodeContributionV1,
        SignedNodeRightsDecisionV1, SignedRecipientKeyAuthorizationV1, ThresholdV1, WalletAddress,
        WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES, X_WING_DRAFT06_CIPHERTEXT_BYTES,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use sha3::Keccak256;
    use tempfile::tempdir;
    use x_wing::kem::{Decapsulator as _, KeyExport as _};
    use x_wing::TryKeyInit as _;

    const NOW: u64 = 2_000_000_000;
    const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
    const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn node_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
    }

    fn wallet(seed: u8) -> WalletAddress {
        let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
        let encoded = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
        RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(seed.max(9))).unwrap()
    }

    fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
        recipient_public_key(seed)
            .key_identity(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
            .unwrap()
    }

    fn xwing_public_key_bytes(
        seed: u8,
    ) -> [u8; elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
        let secret = x_wing::DecapsulationKey::from([seed; x_wing::DECAPSULATION_KEY_SIZE]);
        secret.encapsulation_key().to_bytes().into()
    }

    fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        NodeCustodyPublicKeyV1::new(xwing_public_key_bytes(seed)).unwrap()
    }

    fn sealed_share(seed: u8) -> PqHybridSealedShareV1 {
        let public =
            x_wing::EncapsulationKey::new_from_slice(&xwing_public_key_bytes(seed)).unwrap();
        let (ciphertext, _) =
            public.encapsulate_deterministic(&[seed; x_wing::ENCAPSULATION_RANDOMNESS_SIZE].into());
        let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
        let mut envelope = Vec::with_capacity(PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES);
        envelope.extend_from_slice(&ciphertext);
        envelope.extend_from_slice(&[seed; PQ_HYBRID_AEAD_NONCE_BYTES]);
        envelope.extend_from_slice(&[seed ^ 0x5a; PQ_HYBRID_WRAPPED_SHARE_BYTES]);
        PqHybridSealedShareV1::new(envelope).unwrap()
    }

    fn policy_body() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            EncryptedContentIdentityV1::new(digest(0x21), 4096).unwrap(),
            ContentAccessIdV1::new([0x41; 16]).unwrap(),
            RightsActionV1::View,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap()
    }

    fn signed_custody_epoch() -> SignedCustodyEpochV1 {
        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        let nodes = [1, 2, 3]
            .into_iter()
            .map(|seed| {
                CustodyNodeIdentityV1::new(
                    node_public_key(seed),
                    node_custody_public_key(0x30 + seed),
                    ShareCoordinateV1::new(seed).unwrap(),
                )
                .unwrap()
            })
            .collect();
        let statement = CustodyEpochStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
            CustodyApprovedSuitesV1::new(
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            )
            .unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            nodes,
        )
        .unwrap();
        SignedCustodyEpochV1::new(
            statement.clone(),
            issuer_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn custody_envelope(seed: u8) -> CustodyEnvelopeV1 {
        let epoch = signed_custody_epoch();
        let manifest = CustodyEnvelopeManifestV1::new(
            EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap(),
            CustodyPoolIdentityV1::new(digest(seed ^ 0x34), 512).unwrap(),
            epoch.epoch_identity().unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(seed ^ 0x35), 512).unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            digest(seed ^ 0x33),
            epoch.statement().nodes().to_vec(),
        )
        .unwrap();
        let shares = [seed ^ 0x50, seed ^ 0x51, seed ^ 0x52]
            .into_iter()
            .map(sealed_share)
            .collect();
        CustodyEnvelopeV1::new(manifest, shares).unwrap()
    }

    fn signed_runtime_release_operation(seed: u8) -> SignedRuntimeReleaseOperationV1 {
        let runtime_key = SigningKey::from_bytes(&[seed; 32]);
        let envelope = custody_envelope(0x11);
        let policy = policy_body();
        let binding = elastos_protected_content_contracts::ProtectedContentBindingV1::new(
            envelope.manifest().encrypted_content().clone(),
            envelope.key_envelope_identity().unwrap(),
            policy.policy_identity().unwrap(),
            elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
                SigningKey::from_bytes(&[0x26; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            wallet(7),
            RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
        )
        .unwrap();
        let rights_request = {
            let request = RightsRequestV1::new(
                binding.clone(),
                RightsActionV1::View,
                recipient_identity(0x30),
                NOW,
                NOW + 180,
                ReplayNonce16::new([0x55; 16]),
            )
            .unwrap();
            let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
            let (signature, recovery_id) = key
                .sign_prehash_recoverable(&ethereum_signed_message_hash(
                    &request.canonical_bytes().unwrap(),
                ))
                .unwrap();
            let mut signature_bytes = signature.to_bytes().to_vec();
            signature_bytes.push(recovery_id.to_byte());
            WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
        };
        let release_request = KeyReleaseRequestV1::new(
            binding.clone(),
            rights_request.request().request_hash().unwrap(),
            RightsActionV1::View,
            rights_request.request().recipient().clone(),
            NOW + 1,
            NOW + 50,
            ReplayNonce16::new([0x66; 16]),
        )
        .unwrap();
        let profile = SigningKey::from_bytes(&[0x26; 32]);
        let recipient_public_key = recipient_public_key(0x30);
        let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_public_key,
            rights_request.request().recipient().clone(),
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            NOW,
            NOW + 90,
        )
        .unwrap();
        let authorization = SignedRecipientKeyAuthorizationV1::new(
            authorization_statement.clone(),
            profile
                .sign(&authorization_statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let evidence_request = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
        .unwrap();
        let statement = RuntimeReleaseOperationStatementV1::new(
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            rights_request,
            release_request,
            recipient_public_key,
            authorization,
            policy,
            evidence_request,
            signed_custody_epoch(),
            RuntimeReleaseAuditIdV1::new(digest(0x91 ^ seed)).unwrap(),
            NOW + 2,
            NOW + 40,
        )
        .unwrap();
        SignedRuntimeReleaseOperationV1::new(
            statement.clone(),
            runtime_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn signed_node_rights_decision(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
        decision: RightsDecisionV1,
    ) -> SignedNodeRightsDecisionV1 {
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 3)
            .unwrap();
        let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
            authenticated.release_request_hash(),
            authenticated.rights_request_hash(),
            authenticated.binding().clone(),
            authenticated.action(),
            node_public_key(node_seed),
            decision,
            digest(0x80 ^ node_seed),
            NOW + 4,
            NOW + 35,
        )
        .unwrap();
        SignedNodeRightsDecisionV1::new(
            statement.clone(),
            node_signing_key(node_seed)
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn signed_node_contribution(
        operation: &SignedRuntimeReleaseOperationV1,
        node_seed: u8,
    ) -> SignedNodeContributionV1 {
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 5)
            .unwrap();
        let decision = signed_node_rights_decision(operation, node_seed, RightsDecisionV1::Allowed);
        let sealed = RecipientSealedContributionV1::new(
            authenticated.recipient().clone(),
            vec![node_seed; 96],
        )
        .unwrap();
        let statement = NodeContributionStatementV1::new(
            authenticated.release_request_hash(),
            authenticated.binding().clone(),
            decision,
            sealed,
            NOW + 5,
            NOW + 35,
        )
        .unwrap();
        SignedNodeContributionV1::new(
            statement.clone(),
            node_signing_key(node_seed)
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn draft(seed: u8) -> RuntimeReleaseOperationDraft {
        RuntimeReleaseOperationDraft::new(
            format!(r#"{{"wallet_request":"{seed:02x}"}}"#).into_bytes(),
            format!(r#"{{"wallet_response":"{seed:02x}"}}"#).into_bytes(),
            signed_runtime_release_operation(seed),
        )
        .unwrap()
    }

    fn owner_only_journal_root(temp: &tempfile::TempDir) -> PathBuf {
        let parent = temp.path().join("owner-only-parent");
        create_owner_only_directory(&parent).unwrap();
        parent.join("runtime-release")
    }

    #[test]
    fn durable_state_persists_pre_effect_before_provider_dispatch() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);

        let persisted = journal.persist_before_provider_effect(&draft).unwrap();
        assert_eq!(persisted.draft(), &draft);
        assert!(persisted.terminal_result().is_none());
        assert!(!persisted.provider_effect_started());

        let loaded = journal.load(draft.operation_hash().unwrap()).unwrap();
        assert_eq!(loaded.draft(), &draft);
        assert!(loaded.terminal_result().is_none());
        assert!(!loaded.provider_effect_started());
    }

    #[test]
    fn durable_state_exact_replay_and_conflict_are_distinct() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);
        journal.persist_before_provider_effect(&draft).unwrap();

        let replay = journal.persist_before_provider_effect(&draft).unwrap();
        assert_eq!(replay.draft(), &draft);

        let conflicting = RuntimeReleaseOperationDraft::new(
            br#"{"wallet_request":"changed"}"#.to_vec(),
            draft.wallet_response_bytes().to_vec(),
            draft.signed_runtime_release_operation().clone(),
        )
        .unwrap();
        assert_eq!(
            journal.persist_before_provider_effect(&conflicting),
            Err(RuntimeReleaseJournalError::Conflict)
        );
    }

    #[test]
    fn durable_state_marks_provider_effect_started_without_terminal_settlement() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);
        journal.persist_before_provider_effect(&draft).unwrap();

        let started = journal.mark_provider_effect_started(&draft).unwrap();
        assert!(started.provider_effect_started());
        assert!(started.terminal_result().is_none());

        let reloaded = journal.load(draft.operation_hash().unwrap()).unwrap();
        assert!(reloaded.provider_effect_started());
        assert!(reloaded.terminal_result().is_none());

        let replayed = journal.mark_provider_effect_started(&draft).unwrap();
        assert!(replayed.provider_effect_started());
        assert!(replayed.terminal_result().is_none());
    }

    #[test]
    fn durable_state_replays_only_persisted_terminal_result() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);
        let persisted = journal.persist_before_provider_effect(&draft).unwrap();
        assert_eq!(
            persisted.into_terminal_result(),
            Err(RuntimeReleaseJournalError::NotTerminal)
        );

        let terminal = RuntimeReleaseTerminalResult::ContributionsReady {
            signed_node_contributions: vec![
                signed_node_contribution(draft.signed_runtime_release_operation(), 1),
                signed_node_contribution(draft.signed_runtime_release_operation(), 2),
            ],
        };
        let completed = journal.mark_terminal(&draft, terminal.clone()).unwrap();
        assert_eq!(completed.terminal_result(), Some(&terminal));
        assert!(completed.provider_effect_started());

        let reloaded = journal.load(draft.operation_hash().unwrap()).unwrap();
        assert_eq!(reloaded.terminal_result(), Some(&terminal));
        assert!(reloaded.provider_effect_started());
        let replayed = journal.mark_terminal(&draft, terminal.clone()).unwrap();
        assert_eq!(replayed.terminal_result(), Some(&terminal));

        let conflicting_terminal = RuntimeReleaseTerminalResult::RightsDenied {
            signed_node_rights_decision: Box::new(signed_node_rights_decision(
                draft.signed_runtime_release_operation(),
                1,
                RightsDecisionV1::Denied,
            )),
        };
        assert_eq!(
            journal.mark_terminal(&draft, conflicting_terminal),
            Err(RuntimeReleaseJournalError::Conflict)
        );
    }

    #[test]
    fn durable_state_rejects_corruption_and_unsafe_files() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let corrupt_draft = draft(0x42);
        journal
            .persist_before_provider_effect(&corrupt_draft)
            .unwrap();
        let operation_hash = corrupt_draft.operation_hash().unwrap();
        let record_path = journal.record_path(slot_hash_for(operation_hash));

        let mut bytes = fs::read(&record_path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&record_path, bytes).unwrap();
        assert_eq!(
            journal.load(operation_hash),
            Err(RuntimeReleaseJournalError::Corrupt)
        );

        let journal = RuntimeReleaseJournal::new(temp.path().join("unsafe-release"));
        let unsafe_draft = draft(0x43);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                journal.persist_before_provider_effect(&unsafe_draft),
                Err(RuntimeReleaseJournalError::Unavailable)
            );
            fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }

        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        journal
            .persist_before_provider_effect(&unsafe_draft)
            .unwrap();
        let operation_hash = unsafe_draft.operation_hash().unwrap();
        let record_path = journal.record_path(slot_hash_for(operation_hash));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&record_path, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                journal.load(operation_hash),
                Err(RuntimeReleaseJournalError::Unavailable)
            );
        }
    }

    #[test]
    fn durable_state_recovers_exact_hard_link_publish_residue() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);
        journal.persist_before_provider_effect(&draft).unwrap();
        let operation_hash = draft.operation_hash().unwrap();
        let slot_hash = slot_hash_for(operation_hash);
        let record_path = journal.record_path(slot_hash);
        let temp_path = journal.temp_path(slot_hash);

        fs::hard_link(&record_path, &temp_path).unwrap();
        let loaded = journal.load(operation_hash).unwrap();
        assert_eq!(loaded.draft(), &draft);
        assert!(!temp_path.exists());

        let replay = journal.persist_before_provider_effect(&draft).unwrap();
        assert_eq!(replay.draft(), &draft);
    }

    #[test]
    fn durable_state_lists_unresolved_without_settling_started_effects() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let first = draft(0x21);
        let second = draft(0x22);
        let terminal = draft(0x23);
        journal.persist_before_provider_effect(&first).unwrap();
        journal.persist_before_provider_effect(&second).unwrap();
        journal.persist_before_provider_effect(&terminal).unwrap();
        journal.mark_provider_effect_started(&second).unwrap();
        journal
            .mark_terminal(
                &terminal,
                RuntimeReleaseTerminalResult::RightsDenied {
                    signed_node_rights_decision: Box::new(signed_node_rights_decision(
                        terminal.signed_runtime_release_operation(),
                        1,
                        RightsDecisionV1::Denied,
                    )),
                },
            )
            .unwrap();

        let unresolved = journal.list_unresolved().unwrap();
        let unresolved_hashes: Vec<_> = unresolved
            .iter()
            .map(|operation| operation.draft().operation_hash().unwrap())
            .collect();
        let mut expected = vec![
            first.operation_hash().unwrap(),
            second.operation_hash().unwrap(),
        ];
        expected.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        assert_eq!(unresolved_hashes, expected);
        let started = unresolved
            .iter()
            .find(|operation| {
                operation.draft().operation_hash().unwrap() == second.operation_hash().unwrap()
            })
            .unwrap();
        let pending = unresolved
            .iter()
            .find(|operation| {
                operation.draft().operation_hash().unwrap() == first.operation_hash().unwrap()
            })
            .unwrap();
        assert!(!pending.provider_effect_started());
        assert!(started.provider_effect_started());
        assert!(started.terminal_result().is_none());

        let reloaded = journal.list_unresolved().unwrap();
        assert_eq!(reloaded.len(), 2);
        let reloaded_started = reloaded
            .iter()
            .find(|operation| {
                operation.draft().operation_hash().unwrap() == second.operation_hash().unwrap()
            })
            .unwrap();
        assert!(reloaded_started.provider_effect_started());
        assert!(reloaded_started.terminal_result().is_none());
        assert!(journal
            .load(terminal.operation_hash().unwrap())
            .unwrap()
            .terminal_result()
            .is_some());
    }

    #[test]
    fn durable_state_lists_unresolved_empty_when_journal_is_absent() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        assert!(journal.list_unresolved().unwrap().is_empty());
    }

    #[test]
    fn durable_state_rejects_foreign_hard_link_publish_residue() {
        let temp = tempdir().unwrap();
        let journal = RuntimeReleaseJournal::new(owner_only_journal_root(&temp));
        let draft = draft(0x42);
        journal.persist_before_provider_effect(&draft).unwrap();
        let operation_hash = draft.operation_hash().unwrap();
        let temp_path = journal.temp_path(slot_hash_for(operation_hash));
        let foreign_path = temp.path().join("owner-only-parent").join("foreign");
        {
            let mut file = open_owner_only_temp_file_for_write(&foreign_path).unwrap();
            file.write_all(b"foreign").unwrap();
            file.sync_all().unwrap();
        }
        fs::hard_link(&foreign_path, &temp_path).unwrap();

        assert_eq!(
            journal.load(operation_hash),
            Err(RuntimeReleaseJournalError::Unavailable)
        );
    }

    #[test]
    fn durable_state_debug_redacts_paths_and_bytes() {
        let journal = RuntimeReleaseJournal::new("/tmp/secret-runtime-path");
        let rendered = format!("{journal:?}");
        assert!(rendered.contains("RuntimeReleaseJournal"));
        assert!(!rendered.contains("/tmp/secret-runtime-path"));

        let draft = draft(0x42);
        let rendered = format!("{draft:?}");
        assert!(rendered.contains("RuntimeReleaseOperationDraft"));
        assert!(!rendered.contains("425c"));
        assert!(!rendered.contains("7b2277616c6c65745f72657175657374"));
        assert!(!rendered.contains("7b2277616c6c65745f726573706f6e7365"));
    }
}
