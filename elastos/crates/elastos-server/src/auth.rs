//! Runtime-owned authentication state for proof-bound sessions.

use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[cfg(test)]
use std::collections::HashMap;

#[cfg(unix)]
use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::{anyhow, Context};
use base64::Engine as _;
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_runtime::auth::{
    validate_principal_root_protection, AuthChallengeV1, AuthSessionGrantV1,
    PrincipalRootProtectionV1, PrincipalRootProtectorKind, PrincipalRootRecoveryArchiveV1,
    ProofBinding, RecoveryKitV1, RuntimeAuditEventV1, DEFAULT_PRINCIPAL_ROOT_CIPHER,
};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const AUTH_STATE_SCHEMA: &str = "elastos.auth.state/v1";
const AUTH_STATE_ROOT: &str = "ElastOS/System/Auth";
const AUTH_STATE_FILE: &str = "auth-state.json";
const AUDIT_CHAIN_REQUIRED_FILE: &str = "audit-chain-required.json";
const RECOVERY_ARCHIVE_KEY_FILE: &str = "recovery-archive.key";
const AUDIT_EVENT_DOMAIN: &str = "elastos.audit.event.v1";
const AUDIT_CHAIN_SCHEMA: &str = "elastos.audit.chain-link/v1";
const AUDIT_CHAIN_DOMAIN: &str = "elastos.audit.chain-link.v1";
const AUDIT_CHAIN_GENESIS: &str = "sha256:genesis";
const AUDIT_CHAIN_STATE_SCHEMA: &str = "elastos.audit.chain-state/v1";
const AUDIT_CHAIN_STATE_DOMAIN: &str = "elastos.audit.chain-state.v1";
const AUDIT_CHAIN_ANCHOR_SCHEMA: &str = "elastos.audit.chain-anchor/v1";
const AUDIT_CHAIN_ANCHOR_DOMAIN: &str = "elastos.audit.chain-anchor.v1";
const AUDIT_CHAIN_ACTIVATION_SCHEMA: &str = "elastos.audit.chain-activation/v2";
const AUDIT_CHAIN_ACTIVATION_DOMAIN: &str = "elastos.audit.chain-activation.v2";
const AUDIT_RETENTION_LIMIT: usize = 512;
const RECOVERY_DESCRIPTOR_SCHEMA: &str = "elastos.principal.root-descriptor/v1";
const PRINCIPAL_ROOT_OBJECT_SCHEMA: &str = "elastos.principal-root.object/v1";
pub const PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED: &str =
    "protected principal-root object is not encrypted";
const PRINCIPAL_ROOT_OBJECT_AAD_DOMAIN: &str = "elastos.principal-root.object.v1";
pub const PRINCIPAL_ROOT_MIGRATION_REQUIRED_SCHEMA: &str =
    "elastos.principal-root.migration-required/v1";
pub const PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA: &str = "elastos.principal-root.migration-plan/v1";
pub const PRINCIPAL_ROOT_MIGRATION_RECEIPT_SCHEMA: &str =
    "elastos.principal-root.migration-receipt/v1";
pub const PRINCIPAL_ROOT_UPGRADE_RECEIPT_SCHEMA: &str = "elastos.principal-root.upgrade-receipt/v1";
const PRINCIPAL_ROOT_MIGRATION_JOURNAL_SCHEMA: &str = "elastos.principal-root.migration-journal/v1";
const PRINCIPAL_ROOT_MIGRATION_JOURNAL_FILE: &str = "principal-root-migration-journal.json";
const PRINCIPAL_ROOT_MIGRATION_RECEIPT_FILE: &str = "receipt.json";
const PRINCIPAL_ROOT_UPGRADE_RECEIPT_FILE: &str = "upgrade-receipt.json";
const MAX_PRINCIPAL_ROOT_INVENTORY_DECLARATIONS: usize = 128;
const MAX_PRINCIPAL_ROOT_INVENTORY_OBJECTS: usize = 1024;
const MAX_PRINCIPAL_ROOT_INVENTORY_DEPTH: usize = 16;
const MAX_MIGRATION_OBJECTS: usize = 64;
const MAX_MIGRATION_OBJECT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MIGRATION_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

static AUTH_STATE_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static AUDIT_CHAIN_ACTIVATION_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PRINCIPAL_ROOT_OBJECT_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrincipalRootMigrationTestFault {
    Backup(usize),
    Stage(usize),
    JournalPrepared,
    JournalCommitting,
    Commit(usize),
    JournalCommitted,
    CrashAfterCommit(usize),
}

#[cfg(test)]
static PRINCIPAL_ROOT_MIGRATION_TEST_FAULTS: OnceLock<
    Mutex<HashMap<PathBuf, PrincipalRootMigrationTestFault>>,
> = OnceLock::new();

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryReassignmentTestFault {
    TokenPreparation,
    AuditChainRejection,
    AuthStateSave,
    PostCommitOutcomeAudit,
}

#[cfg(test)]
static RECOVERY_REASSIGNMENT_TEST_FAULTS: OnceLock<
    Mutex<HashMap<PathBuf, RecoveryReassignmentTestFault>>,
> = OnceLock::new();

#[cfg(test)]
pub(crate) fn inject_recovery_reassignment_test_fault(
    data_dir: &Path,
    fault: RecoveryReassignmentTestFault,
) {
    RECOVERY_REASSIGNMENT_TEST_FAULTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("recovery reassignment test fault lock")
        .insert(data_dir.to_path_buf(), fault);
}

#[cfg(test)]
pub(crate) fn consume_recovery_reassignment_test_fault(
    data_dir: &Path,
    fault: RecoveryReassignmentTestFault,
) -> anyhow::Result<()> {
    let faults = RECOVERY_REASSIGNMENT_TEST_FAULTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut faults = faults
        .lock()
        .map_err(|_| anyhow!("recovery reassignment test fault lock poisoned"))?;
    if faults.get(data_dir) == Some(&fault) {
        faults.remove(data_dir);
        anyhow::bail!("injected recovery reassignment {fault:?} failure");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn inject_principal_root_migration_test_fault(
    data_dir: &Path,
    fault: PrincipalRootMigrationTestFault,
) {
    PRINCIPAL_ROOT_MIGRATION_TEST_FAULTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("principal-root migration test fault lock")
        .insert(data_dir.to_path_buf(), fault);
}

#[cfg(test)]
fn consume_principal_root_migration_test_fault(
    data_dir: &Path,
    fault: PrincipalRootMigrationTestFault,
) -> anyhow::Result<()> {
    let faults = PRINCIPAL_ROOT_MIGRATION_TEST_FAULTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut faults = faults
        .lock()
        .map_err(|_| anyhow!("principal-root migration test fault lock poisoned"))?;
    if faults.get(data_dir) == Some(&fault) {
        faults.remove(data_dir);
        if matches!(fault, PrincipalRootMigrationTestFault::CrashAfterCommit(_)) {
            return Err(SimulatedPrincipalRootMigrationCrash.into());
        }
        anyhow::bail!("injected principal-root migration {fault:?} failure");
    }
    Ok(())
}

fn maybe_inject_principal_root_migration_indexed_fault(
    data_dir: &Path,
    stage: PrincipalRootMigrationIndexedFaultStage,
    index: usize,
) -> anyhow::Result<()> {
    #[cfg(test)]
    {
        let fault = match stage {
            PrincipalRootMigrationIndexedFaultStage::Backup => {
                PrincipalRootMigrationTestFault::Backup(index)
            }
            PrincipalRootMigrationIndexedFaultStage::Stage => {
                PrincipalRootMigrationTestFault::Stage(index)
            }
            PrincipalRootMigrationIndexedFaultStage::Commit => {
                PrincipalRootMigrationTestFault::Commit(index)
            }
        };
        consume_principal_root_migration_test_fault(data_dir, fault)?;
        if matches!(stage, PrincipalRootMigrationIndexedFaultStage::Commit) {
            consume_principal_root_migration_test_fault(
                data_dir,
                PrincipalRootMigrationTestFault::CrashAfterCommit(index),
            )?;
        }
    }
    #[cfg(not(test))]
    {
        let _ = (data_dir, stage, index);
    }
    Ok(())
}

fn auth_state_mutation_lock() -> &'static Mutex<()> {
    AUTH_STATE_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
}

fn audit_chain_activation_mutation_lock() -> &'static Mutex<()> {
    AUDIT_CHAIN_ACTIVATION_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
}

fn principal_root_object_mutation_lock() -> &'static Mutex<()> {
    PRINCIPAL_ROOT_OBJECT_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalRootObjectEnvelopeV1 {
    schema: String,
    principal_id: String,
    localhost_root: String,
    data_key_id: String,
    object_uri: String,
    cipher: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrincipalRootProtectedObjectDeclarationV1 {
    Exact { object_uri: String },
    Root { root_uri: String },
}

impl PrincipalRootProtectedObjectDeclarationV1 {
    pub fn exact(object_uri: impl Into<String>) -> Self {
        Self::Exact {
            object_uri: object_uri.into(),
        }
    }

    pub fn root(root_uri: impl Into<String>) -> Self {
        Self::Root {
            root_uri: root_uri.into(),
        }
    }

    pub fn uri(&self) -> &str {
        match self {
            Self::Exact { object_uri } => object_uri,
            Self::Root { root_uri } => root_uri,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrincipalRootMigrationRequiredV1 {
    pub schema: String,
    pub status: String,
    pub code: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub plaintext_object_count: usize,
}

impl std::fmt::Display for PrincipalRootMigrationRequiredV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "principal-root migration required for {} declared plaintext object(s)",
            self.plaintext_object_count
        )
    }
}

impl std::error::Error for PrincipalRootMigrationRequiredV1 {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootMigrationPlanV1 {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub objects: Vec<PrincipalRootMigrationSelectionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootMigrationSelectionV1 {
    pub object_uri: String,
    pub plaintext_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrincipalRootMigrationReceiptV1 {
    pub schema: String,
    pub status: String,
    pub plan_sha256: String,
    pub principal_binding_sha256: String,
    pub object_count: usize,
    pub objects: Vec<PrincipalRootMigrationObjectReceiptV1>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrincipalRootMigrationObjectReceiptV1 {
    pub object_uri_sha256: String,
    pub plaintext_sha256: String,
    pub backup_sha256: String,
    pub encrypted_sha256: String,
    pub round_trip_verified: bool,
}

#[derive(Debug, Clone)]
pub struct PrincipalRootUpgradeDeclarationV1 {
    pub principal_id: String,
    pub localhost_root: String,
    pub inventory: Vec<PrincipalRootProtectedObjectDeclarationV1>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrincipalRootUpgradeReceiptV1 {
    pub schema: String,
    pub status: String,
    pub root_count: usize,
    pub object_count: usize,
    pub roots: Vec<PrincipalRootMigrationReceiptV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PrincipalRootMigrationJournalPhaseV1 {
    Prepared,
    Committing,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalRootMigrationJournalV1 {
    schema: String,
    phase: PrincipalRootMigrationJournalPhaseV1,
    plan_sha256: String,
    principal_id: String,
    localhost_root: String,
    data_key_id: String,
    backup_dir: PathBuf,
    stage_dir: PathBuf,
    objects: Vec<PrincipalRootMigrationJournalObjectV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalRootMigrationJournalObjectV1 {
    object_uri: String,
    plaintext_sha256: String,
    backup_path: PathBuf,
    backup_sha256: String,
    stage_path: PathBuf,
    encrypted_sha256: String,
    original_mode: u32,
}

#[derive(Debug)]
struct PreparedPrincipalRootMigrationObject {
    journal: PrincipalRootMigrationJournalObjectV1,
    path: PathBuf,
    plaintext: Vec<u8>,
    encrypted: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum PrincipalRootMigrationIndexedFaultStage {
    Backup,
    Stage,
    Commit,
}

#[cfg(test)]
#[derive(Debug)]
struct SimulatedPrincipalRootMigrationCrash;

#[cfg(test)]
impl std::fmt::Display for SimulatedPrincipalRootMigrationCrash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("simulated principal-root migration crash")
    }
}

#[cfg(test)]
impl std::error::Error for SimulatedPrincipalRootMigrationCrash {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuthChallenge {
    pub challenge: AuthChallengeV1,
    #[serde(default)]
    pub consumed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalRecord {
    pub principal_id: String,
    pub proof_binding_id: String,
    pub proof_binding: ProofBinding,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default)]
    pub role: RuntimePrincipalRole,
    #[serde(default)]
    pub localhost_root: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePrincipalRole {
    // Existing local runtimes with a single pre-role passkey remain recoverable.
    #[default]
    Admin,
    Guest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuthSession {
    pub grant: AuthSessionGrantV1,
    #[serde(default)]
    pub revoked_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditChainLinkV1 {
    pub schema: String,
    pub sequence: u64,
    pub event_id: String,
    pub previous_hash: String,
    pub event_hash: String,
    pub chain_hash: String,
    pub signer_did: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditChainStateV1 {
    pub schema: String,
    pub activated_at: u64,
    pub head_sequence: u64,
    pub head_hash: String,
    pub signer_did: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditChainAnchorV1 {
    pub schema: String,
    pub sequence: u64,
    pub chain_hash: String,
    pub signer_did: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AuditChainCheckpointV1 {
    head_sequence: u64,
    head_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditChainActivationV2 {
    schema: String,
    activated_at: u64,
    checkpoint: AuditChainCheckpointV1,
    signer_did: String,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthState {
    pub schema: String,
    #[serde(default)]
    pub challenges: Vec<StoredAuthChallenge>,
    #[serde(default)]
    pub principals: Vec<PrincipalRecord>,
    #[serde(default)]
    pub sessions: Vec<StoredAuthSession>,
    #[serde(default)]
    pub principal_root_protections: Vec<PrincipalRootProtectionV1>,
    #[serde(default)]
    pub audit: Vec<RuntimeAuditEventV1>,
    #[serde(default)]
    pub audit_chain: Vec<AuditChainLinkV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_chain_state: Option<AuditChainStateV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_chain_anchor: Option<AuditChainAnchorV1>,
    #[serde(default)]
    pub guest_registration_enabled: bool,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            schema: AUTH_STATE_SCHEMA.to_string(),
            challenges: Vec::new(),
            principals: Vec::new(),
            sessions: Vec::new(),
            principal_root_protections: Vec::new(),
            audit: Vec::new(),
            audit_chain: Vec::new(),
            audit_chain_state: None,
            audit_chain_anchor: None,
            guest_registration_enabled: false,
        }
    }
}

pub fn auth_state_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, AUTH_STATE_ROOT)
        .ok_or_else(|| anyhow!("invalid auth state root"))
        .map(|root| root.join(AUTH_STATE_FILE))
}

fn audit_chain_activation_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, AUTH_STATE_ROOT)
        .ok_or_else(|| anyhow!("invalid auth state root"))
        .map(|root| root.join(AUDIT_CHAIN_REQUIRED_FILE))
}

fn auth_state_lock_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    auth_state_path(data_dir).map(|path| path.with_file_name(format!("{AUTH_STATE_FILE}.lock")))
}

fn audit_chain_activation_lock_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    audit_chain_activation_path(data_dir)
        .map(|path| path.with_file_name(format!("{AUDIT_CHAIN_REQUIRED_FILE}.lock")))
}

fn open_new_secret_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .with_context(|| format!("failed to create secret state file {path:?}"))
}

fn ensure_regular_auth_parent(data_dir: &Path, parent: &Path) -> anyhow::Result<()> {
    let relative = parent
        .strip_prefix(data_dir)
        .map_err(|_| anyhow!("auth state path escaped its data root"))?;
    let mut current = data_dir.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .with_context(|| format!("failed to create auth state path {current:?}"))?;
                std::fs::symlink_metadata(&current).with_context(|| {
                    format!("failed to inspect created auth state path {current:?}")
                })?
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to inspect auth state path {current:?}"));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!("auth state path must use regular non-symlink directories");
        }
    }
    Ok(())
}

fn open_regular_lock_file(path: &Path, label: &str) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {label} lock {path:?}"))?;
    if !file.metadata()?.is_file() {
        anyhow::bail!("{label} lock must be a regular non-symlink file");
    }
    set_secret_file_permissions(path)?;
    Ok(file)
}

fn open_auth_state_lock(data_dir: &Path) -> anyhow::Result<File> {
    let path = auth_state_lock_path(data_dir)?;
    if let Some(parent) = path.parent() {
        ensure_regular_auth_parent(data_dir, parent)?;
    }
    open_regular_lock_file(&path, "auth state")
}

fn open_audit_chain_activation_lock(data_dir: &Path) -> anyhow::Result<File> {
    let path = audit_chain_activation_lock_path(data_dir)?;
    if let Some(parent) = path.parent() {
        ensure_regular_auth_parent(data_dir, parent)?;
    }
    open_regular_lock_file(&path, "audit activation")
}

fn lock_auth_state_file(file: &File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error()).context("failed to lock auth state");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

fn unlock_auth_state_file(file: &File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            return Err(std::io::Error::last_os_error()).context("failed to unlock auth state");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

fn mutate_auth_state<T>(
    data_dir: &Path,
    mutation: impl FnOnce(&mut AuthState) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _guard = auth_state_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("auth state mutation lock poisoned"))?;
    let lock_file = open_auth_state_lock(data_dir)?;
    lock_auth_state_file(&lock_file)?;
    let result = (|| {
        let mut state = load_auth_state(data_dir)?;
        ensure_audit_chain_state(data_dir, &mut state)?;
        let value = mutation(&mut state)?;
        save_auth_state(data_dir, &state)?;
        Ok(value)
    })();
    let unlock = unlock_auth_state_file(&lock_file);
    match (result, unlock) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

pub fn load_or_create_recovery_archive_key(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = rooted_localhost_fs_path(data_dir, AUTH_STATE_ROOT)
        .ok_or_else(|| anyhow!("invalid auth state root"))?
        .join(RECOVERY_ARCHIVE_KEY_FILE);
    if path.is_file() {
        let bytes = std::fs::read(&path).with_context(|| format!("failed to read {path:?}"))?;
        if bytes.len() != 32 {
            anyhow::bail!("invalid recovery archive key");
        }
        set_secret_file_permissions(&path)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    std::fs::write(&path, key)?;
    set_secret_file_permissions(&path)?;
    Ok(key)
}

fn set_secret_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("failed to restrict secret file permissions for {path:?}"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn audit_chain_checkpoint(state: &AuthState) -> anyhow::Result<Option<AuditChainCheckpointV1>> {
    let Some(chain_state) = state.audit_chain_state.as_ref() else {
        return Ok(None);
    };
    let (anchor_sequence, anchor_hash) = match state.audit_chain_anchor.as_ref() {
        Some(anchor) => (Some(anchor.sequence), Some(anchor.chain_hash.clone())),
        None => (None, None),
    };
    Ok(Some(AuditChainCheckpointV1 {
        head_sequence: chain_state.head_sequence,
        head_hash: chain_state.head_hash.clone(),
        anchor_sequence,
        anchor_hash,
    }))
}

fn audit_chain_activation_payload(
    activated_at: u64,
    checkpoint: &AuditChainCheckpointV1,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "schema": AUDIT_CHAIN_ACTIVATION_SCHEMA,
        "activated_at": activated_at,
        "checkpoint": checkpoint,
    }))?)
}

fn load_audit_chain_activation_unlocked(
    data_dir: &Path,
) -> anyhow::Result<Option<AuditChainActivationV2>> {
    let path = audit_chain_activation_path(data_dir)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to inspect audit chain activation record"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("audit chain activation record must be a regular non-symlink file");
    }
    let activation: AuditChainActivationV2 = serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| format!("failed to parse audit chain activation record {path:?}"))?;
    if activation.schema != AUDIT_CHAIN_ACTIVATION_SCHEMA {
        anyhow::bail!("unsupported audit chain activation schema");
    }
    let (_, expected_did) = elastos_identity::load_or_create_did(data_dir)?;
    if activation.signer_did != expected_did {
        anyhow::bail!("audit chain activation signer is not the Runtime identity");
    }
    crate::crypto::verify_domain_separated_signature(
        &activation.signer_did,
        AUDIT_CHAIN_ACTIVATION_DOMAIN,
        &audit_chain_activation_payload(activation.activated_at, &activation.checkpoint)?,
        &activation.signature,
    )
    .context("audit chain activation signature is invalid")?;
    Ok(Some(activation))
}

fn with_audit_chain_activation_lock<T>(
    data_dir: &Path,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _guard = audit_chain_activation_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("audit activation lock poisoned"))?;
    let lock_file = open_audit_chain_activation_lock(data_dir)?;
    lock_auth_state_file(&lock_file)?;
    let result = operation();
    let unlock = unlock_auth_state_file(&lock_file);
    match (result, unlock) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

#[cfg(test)]
fn load_audit_chain_activation(data_dir: &Path) -> anyhow::Result<Option<AuditChainActivationV2>> {
    with_audit_chain_activation_lock(data_dir, || load_audit_chain_activation_unlocked(data_dir))
}

fn persist_audit_chain_activation_unlocked(
    data_dir: &Path,
    activated_at: u64,
    checkpoint: &AuditChainCheckpointV1,
) -> anyhow::Result<()> {
    let path = audit_chain_activation_path(data_dir)?;
    if let Some(parent) = path.parent() {
        ensure_regular_auth_parent(data_dir, parent)?;
    }
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    let (signature, _) = crate::crypto::domain_separated_sign(
        &signing_key,
        AUDIT_CHAIN_ACTIVATION_DOMAIN,
        &audit_chain_activation_payload(activated_at, checkpoint)?,
    );
    let activation = AuditChainActivationV2 {
        schema: AUDIT_CHAIN_ACTIVATION_SCHEMA.to_string(),
        activated_at,
        checkpoint: checkpoint.clone(),
        signer_did,
        signature,
    };
    write_secret_json_atomic(&path, &activation)
}

fn validate_checkpoint_progress(
    current: &AuditChainCheckpointV1,
    next: &AuditChainCheckpointV1,
) -> anyhow::Result<bool> {
    if next.head_sequence < current.head_sequence {
        anyhow::bail!("audit chain rollback detected by the activation checkpoint");
    }
    if next.head_sequence == current.head_sequence {
        if next != current {
            anyhow::bail!(
                "audit chain truncation or substitution detected by the activation checkpoint"
            );
        }
        return Ok(false);
    }
    match (current.anchor_sequence, next.anchor_sequence) {
        (Some(_), None) => anyhow::bail!("retained audit chain anchor rollback detected"),
        (Some(current_sequence), Some(next_sequence)) if next_sequence < current_sequence => {
            anyhow::bail!("retained audit chain anchor rollback detected")
        }
        (Some(current_sequence), Some(next_sequence))
            if next_sequence == current_sequence && current.anchor_hash != next.anchor_hash =>
        {
            anyhow::bail!("retained audit chain anchor substitution detected")
        }
        _ => {}
    }
    Ok(true)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn write_secret_json_atomic(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("secret state path has no file name"))?;
    let temp = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        unique
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut file = open_new_secret_file(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        set_secret_file_permissions(path)?;
        sync_parent_directory(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn load_auth_state(data_dir: &Path) -> anyhow::Result<AuthState> {
    with_audit_chain_activation_lock(data_dir, || {
        let stored_state = load_auth_state_unverified(data_dir)?;
        let state_was_present = stored_state.is_some();
        let mut state = stored_state.unwrap_or_default();
        let activation = load_audit_chain_activation_unlocked(data_dir)?;
        match (state.audit_chain_state.as_ref(), activation.as_ref()) {
            (Some(chain_state), Some(activation)) => {
                verify_audit_chain(data_dir, &state)?;
                if chain_state.activated_at != activation.activated_at {
                    anyhow::bail!("audit chain state does not match its activation record");
                }
                let checkpoint = audit_chain_checkpoint(&state)?
                    .expect("audit chain state must produce a checkpoint");
                if validate_checkpoint_progress(&activation.checkpoint, &checkpoint)? {
                    persist_audit_chain_activation_unlocked(
                        data_dir,
                        chain_state.activated_at,
                        &checkpoint,
                    )?;
                }
            }
            (Some(chain_state), None) => {
                verify_audit_chain(data_dir, &state)
                    .context("cannot recover audit activation from invalid signed chain state")?;
                let checkpoint = audit_chain_checkpoint(&state)?
                    .expect("audit chain state must produce a checkpoint");
                persist_audit_chain_activation_unlocked(
                    data_dir,
                    chain_state.activated_at,
                    &checkpoint,
                )?;
            }
            (None, Some(_)) => {
                anyhow::bail!("audit chain state is required after activation");
            }
            (None, None) if state_was_present && auth_state_requires_audit_chain(&state) => {
                anyhow::bail!(
                    "unchained auth state is unsupported; preserve and back up the existing data root, then use a fresh data root; no automatic migration or offline migration script is provided"
                );
            }
            (None, None) => {}
        }
        verify_audit_chain(data_dir, &state)?;
        prune_auth_state(&mut state, now_ts());
        Ok(state)
    })
}

fn auth_state_requires_audit_chain(state: &AuthState) -> bool {
    !state.challenges.is_empty()
        || !state.principals.is_empty()
        || !state.sessions.is_empty()
        || !state.principal_root_protections.is_empty()
        || !state.audit.is_empty()
        || !state.audit_chain.is_empty()
        || state.audit_chain_anchor.is_some()
        || state.guest_registration_enabled
}

fn load_auth_state_unverified(data_dir: &Path) -> anyhow::Result<Option<AuthState>> {
    let path = auth_state_path(data_dir)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to inspect auth state"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("auth state must be a regular non-symlink file");
    }
    let bytes = std::fs::read(&path).with_context(|| format!("failed to read {path:?}"))?;
    let mut state: AuthState = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse auth state {path:?}"))?;
    if state.schema != AUTH_STATE_SCHEMA {
        anyhow::bail!("unsupported auth state schema");
    }
    normalize_principal_records(&mut state);
    Ok(Some(state))
}

pub fn verify_auth_audit_chain_ready(data_dir: &Path) -> anyhow::Result<()> {
    load_auth_state(data_dir).map(|_| ())
}

pub(crate) fn save_auth_state(data_dir: &Path, state: &AuthState) -> anyhow::Result<()> {
    #[cfg(test)]
    consume_recovery_reassignment_test_fault(
        data_dir,
        RecoveryReassignmentTestFault::AuthStateSave,
    )?;
    with_audit_chain_activation_lock(data_dir, || {
        verify_audit_chain(data_dir, state)?;
        let path = auth_state_path(data_dir)?;
        if let Some(parent) = path.parent() {
            ensure_regular_auth_parent(data_dir, parent)?;
        }
        let activation = load_audit_chain_activation_unlocked(data_dir)?;
        let checkpoint = audit_chain_checkpoint(state)?;
        match (
            activation.as_ref(),
            state.audit_chain_state.as_ref(),
            checkpoint.as_ref(),
        ) {
            (Some(_), None, None) => {
                anyhow::bail!(
                    "audit chain rollback detected: chain state was removed after activation"
                )
            }
            (Some(activation), Some(chain_state), Some(checkpoint)) => {
                if activation.activated_at != chain_state.activated_at {
                    anyhow::bail!("audit chain activation changed unexpectedly");
                }
                validate_checkpoint_progress(&activation.checkpoint, checkpoint)?;
            }
            _ => {}
        }
        write_secret_json_atomic(&path, state)?;
        if let (Some(chain_state), Some(checkpoint)) =
            (state.audit_chain_state.as_ref(), checkpoint.as_ref())
        {
            let should_persist = activation
                .as_ref()
                .is_none_or(|activation| activation.checkpoint != *checkpoint);
            if should_persist {
                persist_audit_chain_activation_unlocked(
                    data_dir,
                    chain_state.activated_at,
                    checkpoint,
                )?;
            }
        }
        Ok(())
    })
}

pub fn store_challenge(data_dir: &Path, challenge: AuthChallengeV1) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        let now = now_ts();
        prune_auth_state(state, now);
        state
            .challenges
            .retain(|stored| stored.challenge.challenge_id != challenge.challenge_id);
        state.challenges.push(StoredAuthChallenge {
            challenge,
            consumed_at: None,
        });
        Ok(())
    })
}

pub fn load_challenge(data_dir: &Path, challenge_id: &str) -> anyhow::Result<AuthChallengeV1> {
    let state = load_auth_state(data_dir)?;
    let stored = state
        .challenges
        .iter()
        .find(|stored| stored.challenge.challenge_id == challenge_id)
        .ok_or_else(|| anyhow!("auth challenge not found"))?;
    if stored.consumed_at.is_some() {
        anyhow::bail!("auth challenge already consumed");
    }
    Ok(stored.challenge.clone())
}

pub fn consume_challenge(
    data_dir: &Path,
    challenge_id: &str,
    consumed_at: u64,
) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        let stored = state
            .challenges
            .iter_mut()
            .find(|stored| stored.challenge.challenge_id == challenge_id)
            .ok_or_else(|| anyhow!("auth challenge not found"))?;
        if stored.consumed_at.is_some() {
            anyhow::bail!("auth challenge already consumed");
        }
        stored.consumed_at = Some(consumed_at);
        Ok(())
    })
}

pub fn upsert_principal_for_binding(
    data_dir: &Path,
    binding: ProofBinding,
    now: u64,
) -> anyhow::Result<PrincipalRecord> {
    let proof_binding_id = binding.id();
    let principal_id = local_person_principal_id(&proof_binding_id);
    upsert_principal_for_binding_as(data_dir, binding, principal_id, now)
}

pub fn upsert_principal_for_binding_as(
    data_dir: &Path,
    binding: ProofBinding,
    principal_id: String,
    now: u64,
) -> anyhow::Result<PrincipalRecord> {
    upsert_principal_for_binding_as_role(
        data_dir,
        binding,
        principal_id,
        RuntimePrincipalRole::Guest,
        now,
    )
}

pub fn upsert_principal_for_binding_as_role(
    data_dir: &Path,
    binding: ProofBinding,
    principal_id: String,
    role: RuntimePrincipalRole,
    now: u64,
) -> anyhow::Result<PrincipalRecord> {
    upsert_principal_for_binding_as_role_named(data_dir, binding, principal_id, role, None, now)
}

pub fn upsert_principal_for_binding_as_role_named(
    data_dir: &Path,
    mut binding: ProofBinding,
    principal_id: String,
    role: RuntimePrincipalRole,
    display_name: Option<&str>,
    now: u64,
) -> anyhow::Result<PrincipalRecord> {
    if principal_id.trim().is_empty()
        || principal_id
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid principal id");
    }
    let display_name = clean_principal_display_name(display_name)?;
    mutate_auth_state(data_dir, |state| {
        normalize_principal_records(state);
        let proof_binding_id = binding.id();
        let localhost_root = principal_localhost_root(&principal_id);
        if let Some(existing) = state
            .principals
            .iter_mut()
            .find(|principal| principal.proof_binding_id == proof_binding_id)
        {
            preserve_passkey_binding_metadata(&mut binding, &existing.proof_binding);
            existing.principal_id = principal_id;
            existing.proof_binding = binding;
            if let Some(display_name) = display_name {
                existing.display_name = display_name;
            }
            existing.localhost_root = localhost_root;
            existing.updated_at = now;
            return Ok(existing.clone());
        }

        let record = PrincipalRecord {
            principal_id,
            proof_binding_id,
            proof_binding: binding,
            display_name: display_name.unwrap_or_default(),
            role,
            localhost_root,
            created_at: now,
            updated_at: now,
        };
        state.principals.push(record.clone());
        Ok(record)
    })
}

fn normalize_principal_records(state: &mut AuthState) {
    for principal in &mut state.principals {
        if principal.localhost_root.trim().is_empty() {
            principal.localhost_root = principal_localhost_root(&principal.principal_id);
        }
    }
}

fn preserve_passkey_binding_metadata(binding: &mut ProofBinding, existing: &ProofBinding) {
    let (Some(next), Some(previous)) = (binding.passkey.as_mut(), existing.passkey.as_ref()) else {
        return;
    };
    if next.credential_id == previous.credential_id && next.rp_id == previous.rp_id {
        next.created_at = previous.created_at;
        next.revoked_at = previous.revoked_at;
    }
}

pub fn clean_principal_display_name(input: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(input) = input else {
        return Ok(None);
    };
    let value = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 64
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch == '/' || ch == '\\')
    {
        anyhow::bail!("invalid principal display name");
    }
    Ok(Some(value))
}

pub fn set_principal_display_name(
    data_dir: &Path,
    proof_binding_id: &str,
    display_name: &str,
    updated_at: u64,
) -> anyhow::Result<PrincipalRecord> {
    let display_name = clean_principal_display_name(Some(display_name))?
        .ok_or_else(|| anyhow!("principal display name must not be empty"))?;
    mutate_auth_state(data_dir, |state| {
        let principal = state
            .principals
            .iter_mut()
            .find(|principal| principal.proof_binding_id == proof_binding_id)
            .ok_or_else(|| anyhow!("proof binding not found"))?;
        principal.display_name = display_name;
        principal.updated_at = updated_at;
        Ok(principal.clone())
    })
}

pub fn store_session_grant(data_dir: &Path, grant: AuthSessionGrantV1) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        state
            .sessions
            .retain(|stored| stored.grant.session_id != grant.session_id);
        state.sessions.push(StoredAuthSession {
            grant,
            revoked_at: None,
        });
        Ok(())
    })
}

pub fn renew_session_grant(data_dir: &Path, grant: AuthSessionGrantV1) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        let stored = state
            .sessions
            .iter_mut()
            .find(|stored| stored.grant.session_id == grant.session_id)
            .ok_or_else(|| anyhow!("auth session not found"))?;
        if stored.revoked_at.is_some() {
            anyhow::bail!("auth session is not active");
        }
        if stored.grant.grant_id != grant.grant_id
            || stored.grant.principal_id != grant.principal_id
            || stored.grant.proof_binding_id != grant.proof_binding_id
        {
            anyhow::bail!("auth session authority context mismatch");
        }
        stored.grant = grant;
        Ok(())
    })
}

pub fn rotate_session_grant(
    data_dir: &Path,
    previous_session_id: &str,
    grant: AuthSessionGrantV1,
    revoked_at: u64,
) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        if let Some(previous) = state
            .sessions
            .iter_mut()
            .find(|stored| stored.grant.session_id == previous_session_id)
        {
            previous.revoked_at = Some(revoked_at);
        }
        state
            .sessions
            .retain(|stored| stored.grant.session_id != grant.session_id);
        state.sessions.push(StoredAuthSession {
            grant,
            revoked_at: None,
        });
        Ok(())
    })
}

pub fn revoke_session_grant(
    data_dir: &Path,
    session_id: &str,
    revoked_at: u64,
) -> anyhow::Result<()> {
    mutate_auth_state(data_dir, |state| {
        let stored = state
            .sessions
            .iter_mut()
            .find(|stored| stored.grant.session_id == session_id)
            .ok_or_else(|| anyhow!("auth session not found"))?;
        stored.revoked_at = Some(revoked_at);
        Ok(())
    })
}

pub fn load_active_session_grant(
    data_dir: &Path,
    session_id: &str,
    now: u64,
) -> anyhow::Result<AuthSessionGrantV1> {
    let state = load_auth_state(data_dir)?;
    let stored = state
        .sessions
        .iter()
        .find(|stored| stored.grant.session_id == session_id)
        .ok_or_else(|| anyhow!("auth session not found"))?;
    if stored.revoked_at.is_some() || stored.grant.expires_at <= now {
        anyhow::bail!("auth session is not active");
    }
    Ok(stored.grant.clone())
}

pub fn is_auth_session_active(data_dir: &Path, session_id: &str, now: u64) -> anyhow::Result<bool> {
    let state = load_auth_state(data_dir)?;
    Ok(state.sessions.iter().any(|stored| {
        stored.grant.session_id == session_id
            && stored.revoked_at.is_none()
            && stored.grant.expires_at > now
    }))
}

pub fn store_principal_root_protection(
    data_dir: &Path,
    protection: PrincipalRootProtectionV1,
) -> anyhow::Result<()> {
    validate_principal_root_protection(&protection).map_err(anyhow::Error::msg)?;
    mutate_auth_state(data_dir, |state| {
        state.principal_root_protections.retain(|stored| {
            stored.principal_id != protection.principal_id
                || stored.localhost_root != protection.localhost_root
        });
        state.principal_root_protections.push(protection);
        Ok(())
    })
}

pub fn load_principal_root_protection(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<PrincipalRootProtectionV1>> {
    let state = load_auth_state(data_dir)?;
    let Some(protection) = state.principal_root_protections.into_iter().find(|stored| {
        stored.principal_id == principal_id && stored.localhost_root == localhost_root
    }) else {
        return Ok(None);
    };
    validate_principal_root_protection(&protection).map_err(anyhow::Error::msg)?;
    Ok(Some(protection))
}

#[derive(Debug)]
pub struct PrincipalRootProtectionActivationGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[derive(Debug, Clone)]
pub(crate) struct PrincipalRootProtectionInspection {
    pub protection: Option<PrincipalRootProtectionV1>,
    pub declared_object_count: usize,
    pub encrypted_object_count: usize,
    pub plaintext_object_count: usize,
}

pub fn begin_declarative_principal_root_protection_activation(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<PrincipalRootProtectionActivationGuard> {
    validate_principal_root_object_binding(principal_id, localhost_root, localhost_root)?;
    let guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let protection = load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    let classified = classify_declared_principal_root_objects(
        data_dir,
        principal_id,
        localhost_root,
        inventory,
    )?;
    verify_classified_principal_root_objects_with_stored_protection(
        data_dir,
        protection.as_ref(),
        &classified,
    )?;
    require_no_declared_principal_root_plaintext(
        principal_id,
        localhost_root,
        classified.plaintext_object_count,
    )?;
    Ok(PrincipalRootProtectionActivationGuard { _guard: guard })
}

pub(crate) struct PrincipalRootProtectionActivationWithPlaintext {
    pub(crate) guard: PrincipalRootProtectionActivationGuard,
    pub(crate) plaintext_objects: Vec<PrincipalRootMigrationSelectionV1>,
}

/// First-time activation cannot demand an already-clean root: the shell
/// writes declared objects (Home browser state, for one) before a person can
/// reach Recovery, so the export flow captures the surviving plaintext here
/// and migrates it under this same guard once the root's protection exists.
/// Declared encrypted envelopes are still verified against the stored
/// protection exactly as in the strict entry above.
pub(crate) fn begin_declarative_principal_root_protection_activation_migrating_plaintext(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<PrincipalRootProtectionActivationWithPlaintext> {
    validate_principal_root_object_binding(principal_id, localhost_root, localhost_root)?;
    let guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let protection = load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    let classified = classify_declared_principal_root_objects(
        data_dir,
        principal_id,
        localhost_root,
        inventory,
    )?;
    verify_classified_principal_root_objects_with_stored_protection(
        data_dir,
        protection.as_ref(),
        &classified,
    )?;
    Ok(PrincipalRootProtectionActivationWithPlaintext {
        guard: PrincipalRootProtectionActivationGuard { _guard: guard },
        plaintext_objects: classified.plaintext_objects,
    })
}

/// Can the online path finish what protecting this root would start?
///
/// Protecting a root stores the protection before it can encrypt anything
/// under it, so a migration that fails afterwards leaves exactly the pair
/// the boot check refuses to start on: a protected root with plaintext
/// still beside it. The plan validator caps one migration far below the
/// inventory ceiling, so an ordinary Documents folder can exceed it. Ask
/// while refusing still costs nothing, and leave the larger migration to
/// the offline command, which can hold the Home still while it works.
pub(crate) fn ensure_online_plaintext_migration_is_possible(
    plaintext_objects: &[PrincipalRootMigrationSelectionV1],
) -> anyhow::Result<()> {
    if plaintext_objects.len() > MAX_MIGRATION_OBJECTS {
        anyhow::bail!(
            "migration_required: {} unprotected objects exceed the {} this can move while the \
             Home is running; run the offline principal-root upgrade first",
            plaintext_objects.len(),
            MAX_MIGRATION_OBJECTS,
        );
    }
    Ok(())
}

/// Migrate the plaintext selections captured at activation begin, under the
/// held activation guard, once the root's protection is stored. Runs the same
/// journaled plan machinery as the offline command; the backup lands under
/// the Home's own backups directory.
pub(crate) fn migrate_principal_root_plaintext_objects_under_activation(
    _activation: &PrincipalRootProtectionActivationGuard,
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    selections: Vec<PrincipalRootMigrationSelectionV1>,
) -> anyhow::Result<PrincipalRootMigrationReceiptV1> {
    let plan = PrincipalRootMigrationPlanV1 {
        schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
        principal_id: principal_id.to_string(),
        localhost_root: localhost_root.to_string(),
        objects: selections,
    };
    validate_principal_root_migration_plan(&plan)?;
    let plan_bytes = serde_json::to_vec(&plan)?;
    let plan_sha256 = sha256_label(&plan_bytes);
    let backups_parent = data_dir.join("backups");
    ensure_safe_backup_parent(&backups_parent)?;
    let backup_dir = backups_parent.join(format!(
        "principal-root-migration-{}-{}",
        &plan_sha256["sha256:".len()..][..16],
        now_ts(),
    ));
    recover_principal_root_migration_journal(data_dir)?;
    migrate_principal_root_objects_plan_locked(data_dir, &plan, &plan_sha256, &backup_dir)
}

pub(crate) fn begin_declarative_principal_root_protection_activation_with_candidate(
    data_dir: &Path,
    candidate: &PrincipalRootProtectionV1,
    data_key: &[u8; 32],
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<PrincipalRootProtectionActivationGuard> {
    validate_principal_root_protection(candidate).map_err(anyhow::Error::msg)?;
    validate_principal_root_object_binding(
        &candidate.principal_id,
        &candidate.localhost_root,
        &candidate.localhost_root,
    )?;
    if principal_data_key_id(data_key) != candidate.data_key_id {
        anyhow::bail!("candidate principal-root protection data key binding mismatch");
    }
    let guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let classified = classify_declared_principal_root_objects(
        data_dir,
        &candidate.principal_id,
        &candidate.localhost_root,
        inventory,
    )?;
    require_no_declared_principal_root_plaintext(
        &candidate.principal_id,
        &candidate.localhost_root,
        classified.plaintext_object_count,
    )?;
    verify_classified_principal_root_object_envelopes(
        &classified,
        &candidate.principal_id,
        &candidate.localhost_root,
        &candidate.data_key_id,
        data_key,
    )?;
    Ok(PrincipalRootProtectionActivationGuard { _guard: guard })
}

pub(crate) fn inspect_declarative_principal_root_protection(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<PrincipalRootProtectionInspection> {
    validate_principal_root_object_binding(principal_id, localhost_root, localhost_root)?;
    let _guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let protection = load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    let classified = classify_declared_principal_root_objects(
        data_dir,
        principal_id,
        localhost_root,
        inventory,
    )?;
    verify_classified_principal_root_objects_with_stored_protection(
        data_dir,
        protection.as_ref(),
        &classified,
    )?;
    Ok(PrincipalRootProtectionInspection {
        protection,
        declared_object_count: classified.declared_object_count,
        encrypted_object_count: classified.envelopes.len(),
        plaintext_object_count: classified.plaintext_object_count,
    })
}

pub fn read_principal_root_object(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
) -> anyhow::Result<Vec<u8>> {
    validate_principal_root_object_binding(principal_id, localhost_root, object_uri)?;
    let _guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let bytes = read_principal_root_object_bytes(path)?;
    let Some(protection) = load_principal_root_protection(data_dir, principal_id, localhost_root)?
    else {
        return Ok(bytes);
    };
    let data_key = principal_root_data_key_from_protection(data_dir, &protection)?;
    let envelope: PrincipalRootObjectEnvelopeV1 =
        serde_json::from_slice(&bytes).with_context(|| {
            format!("{PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED}: {object_uri}")
        })?;
    validate_principal_root_object_envelope(
        &envelope,
        principal_id,
        localhost_root,
        &protection.data_key_id,
        object_uri,
    )?;
    let nonce = b64_url_decode(&envelope.nonce)?;
    if nonce.len() != 12 {
        anyhow::bail!("principal-root object nonce must be 12 bytes");
    }
    let ciphertext = b64_url_decode(&envelope.ciphertext)?;
    decrypt_aes256_gcm_bytes_with_aad(
        &data_key,
        &nonce,
        &ciphertext,
        principal_root_object_aad(&envelope).as_bytes(),
    )
    .with_context(|| format!("failed to decrypt protected principal-root object: {object_uri}"))
}

fn read_principal_root_object_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(err).with_context(|| format!("failed to read {path:?}"));
        }
        Err(err) => return Err(err).with_context(|| format!("failed to read {path:?}")),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect {path:?}"))?;
    if !metadata.is_file() {
        anyhow::bail!("principal-root object must be a regular file");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {path:?}"))?;
    Ok(bytes)
}

pub fn write_principal_root_object(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
    plaintext: &[u8],
) -> anyhow::Result<()> {
    write_principal_root_object_inner(
        data_dir,
        principal_id,
        localhost_root,
        object_uri,
        path,
        plaintext,
        false,
    )
}

pub(crate) fn write_protected_principal_root_object(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
    plaintext: &[u8],
) -> anyhow::Result<()> {
    write_principal_root_object_inner(
        data_dir,
        principal_id,
        localhost_root,
        object_uri,
        path,
        plaintext,
        true,
    )
}

fn write_principal_root_object_inner(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
    plaintext: &[u8],
    require_protection: bool,
) -> anyhow::Result<()> {
    validate_principal_root_object_binding(principal_id, localhost_root, object_uri)?;
    validate_principal_root_object_path(data_dir, object_uri, path)?;
    let _guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let protection = load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    if require_protection && protection.is_none() {
        anyhow::bail!("protected principal-root object requires active principal-root protection");
    }
    let bytes = if let Some(protection) = protection {
        let data_key = principal_root_data_key_from_protection(data_dir, &protection)?;
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let mut envelope = PrincipalRootObjectEnvelopeV1 {
            schema: PRINCIPAL_ROOT_OBJECT_SCHEMA.to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            data_key_id: protection.data_key_id,
            object_uri: object_uri.to_string(),
            cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
            nonce: b64_url(&nonce),
            ciphertext: String::new(),
        };
        envelope.ciphertext = encrypt_aes256_gcm_bytes_with_aad(
            &data_key,
            &nonce,
            plaintext,
            principal_root_object_aad(&envelope).as_bytes(),
        )?;
        serde_json::to_vec_pretty(&envelope)?
    } else {
        plaintext.to_vec()
    };
    if require_protection {
        ensure_protected_principal_root_object_parent(data_dir, path)?;
    }
    atomic_write(path, &bytes)
}

pub(crate) fn recovery_archive_from_kit(
    data_dir: &Path,
    kit: &RecoveryKitV1,
) -> anyhow::Result<PrincipalRootRecoveryArchiveV1> {
    let archive_key = load_or_create_recovery_archive_key(data_dir)?;
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let bytes = serde_json::to_vec(kit)?;
    Ok(PrincipalRootRecoveryArchiveV1 {
        cipher: kit.crypto.cipher.clone(),
        nonce: b64_url(&nonce),
        encrypted_recovery_kit: encrypt_aes256_gcm_bytes(&archive_key, &nonce, &bytes)?,
        created_at: kit.created_at,
    })
}

pub(crate) fn recovery_kit_from_archive(
    data_dir: &Path,
    archive: &PrincipalRootRecoveryArchiveV1,
) -> anyhow::Result<RecoveryKitV1> {
    if archive.cipher != DEFAULT_PRINCIPAL_ROOT_CIPHER {
        anyhow::bail!("unsupported recovery kit archive cipher");
    }
    let archive_key = load_or_create_recovery_archive_key(data_dir)?;
    let nonce = b64_url_decode(&archive.nonce)?;
    if nonce.len() != 12 {
        anyhow::bail!("recovery kit archive nonce must be 12 bytes");
    }
    let ciphertext = b64_url_decode(&archive.encrypted_recovery_kit)?;
    let plaintext = decrypt_aes256_gcm_bytes(&archive_key, &nonce, &ciphertext)?;
    serde_json::from_slice(&plaintext).map_err(Into::into)
}

pub(crate) fn verify_recovery_kit_material(kit: &RecoveryKitV1) -> anyhow::Result<()> {
    elastos_runtime::auth::validate_recovery_kit(kit).map_err(anyhow::Error::msg)?;
    let data_key = recovery_kit_data_key(kit)?;
    let descriptor = decrypt_root_descriptor(kit, &data_key)?;
    if descriptor.get("schema").and_then(Value::as_str) != Some(RECOVERY_DESCRIPTOR_SCHEMA)
        || descriptor.get("principal_id").and_then(Value::as_str) != Some(kit.principal_id.as_str())
        || descriptor.get("localhost_root").and_then(Value::as_str)
            != Some(kit.localhost_root.as_str())
        || descriptor.get("data_key_id").and_then(Value::as_str) != Some(kit.data_key_id.as_str())
    {
        anyhow::bail!("recovery kit root descriptor binding mismatch");
    }
    Ok(())
}

pub(crate) fn recovery_kit_data_key(kit: &RecoveryKitV1) -> anyhow::Result<[u8; 32]> {
    let salt = b64_url_decode(&kit.salt)?;
    let nonce = b64_url_decode(&kit.nonce)?;
    if salt.len() != 32 {
        anyhow::bail!("recovery kit salt must be 32 bytes");
    }
    if nonce.len() != 12 {
        anyhow::bail!("recovery kit nonce must be 12 bytes");
    }
    let wrapping_key = derive_recovery_wrapping_key(
        &kit.recovery_phrase,
        &salt,
        &kit.principal_id,
        &kit.localhost_root,
    )?;
    let wrapped_data_key = b64_url_decode(&kit.wrapped_data_key)?;
    let data_key = decrypt_aes256_gcm_bytes(&wrapping_key, &nonce, &wrapped_data_key)?;
    if data_key.len() != 32 {
        anyhow::bail!("recovered principal data key must be 32 bytes");
    }
    if principal_data_key_id(&data_key) != kit.data_key_id {
        anyhow::bail!("recovery kit data key binding mismatch");
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&data_key);
    Ok(key)
}

pub(crate) fn derive_recovery_wrapping_key(
    recovery_phrase: &str,
    salt: &[u8],
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), recovery_phrase.as_bytes());
    let mut key = [0u8; 32];
    let info = format!("elastos:root-recovery:v1:{principal_id}:{localhost_root}");
    hk.expand(info.as_bytes(), &mut key)
        .map_err(|_| anyhow!("recovery key derivation failed"))?;
    Ok(key)
}

pub(crate) fn encrypt_aes256_gcm_bytes(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
) -> anyhow::Result<String> {
    encrypt_aes256_gcm_bytes_with_aad(key, nonce, plaintext, &[])
}

pub(crate) fn decrypt_aes256_gcm_bytes(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    decrypt_aes256_gcm_bytes_with_aad(key, nonce, ciphertext, &[])
}

pub(crate) fn principal_data_key_id(data_key: &[u8]) -> String {
    format!("pdek:{}", hex::encode(&Sha256::digest(data_key)[..16]))
}

pub(crate) fn b64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn b64_url_decode(value: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(Into::into)
}

pub fn list_passkey_principals(data_dir: &Path) -> anyhow::Result<Vec<PrincipalRecord>> {
    let state = load_auth_state(data_dir)?;
    Ok(state
        .principals
        .into_iter()
        .filter(|principal| principal.proof_binding.passkey.is_some())
        .collect())
}

pub fn active_passkey_principal_count(data_dir: &Path) -> anyhow::Result<usize> {
    Ok(active_passkey_principals(data_dir)?.len())
}

pub fn active_admin_passkey_principal_count(data_dir: &Path) -> anyhow::Result<usize> {
    Ok(active_passkey_principals(data_dir)?
        .into_iter()
        .filter(is_admin)
        .count())
}

pub fn active_passkey_principals(data_dir: &Path) -> anyhow::Result<Vec<PrincipalRecord>> {
    let state = load_auth_state(data_dir)?;
    Ok(state
        .principals
        .into_iter()
        .filter(|principal| {
            principal
                .proof_binding
                .passkey
                .as_ref()
                .is_some_and(|passkey| passkey.revoked_at.is_none())
        })
        .collect())
}

pub fn guest_registration_enabled(data_dir: &Path) -> anyhow::Result<bool> {
    Ok(load_auth_state(data_dir)?.guest_registration_enabled)
}

pub fn set_guest_registration_enabled(
    data_dir: &Path,
    enabled: bool,
    updated_at: u64,
) -> anyhow::Result<bool> {
    let reason = if enabled {
        "guest passkey registration enabled"
    } else {
        "guest passkey registration disabled"
    };
    let event = sign_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!(
                "audit:guest-registration:{updated_at}:{}",
                if enabled { "enabled" } else { "disabled" }
            ),
            event_type: "auth.guest_registration.updated".to_string(),
            principal_id: None,
            proof_binding_id: None,
            session_id: None,
            challenge_id: None,
            capsule_id: None,
            result: "ok".to_string(),
            reason: reason.to_string(),
            occurred_at: updated_at,
            signer_did: None,
            signature: None,
        },
    )?;
    mutate_auth_state(data_dir, |state| {
        state.guest_registration_enabled = enabled;
        push_audit_event(data_dir, state, event)?;
        Ok(enabled)
    })
}

pub fn is_admin(record: &PrincipalRecord) -> bool {
    record.role == RuntimePrincipalRole::Admin
}

pub fn load_principal_for_proof_binding(
    data_dir: &Path,
    proof_binding_id: &str,
) -> anyhow::Result<PrincipalRecord> {
    let state = load_auth_state(data_dir)?;
    state
        .principals
        .into_iter()
        .find(|principal| principal.proof_binding_id == proof_binding_id)
        .ok_or_else(|| anyhow!("proof binding not found"))
}

pub fn ensure_proof_binding_not_revoked(record: &PrincipalRecord) -> anyhow::Result<()> {
    if record
        .proof_binding
        .passkey
        .as_ref()
        .and_then(|passkey| passkey.revoked_at)
        .is_some()
    {
        anyhow::bail!("passkey proof binding revoked");
    }
    Ok(())
}

pub fn revoke_passkey_binding(
    data_dir: &Path,
    proof_binding_id: &str,
    revoked_at: u64,
) -> anyhow::Result<PrincipalRecord> {
    mutate_auth_state(data_dir, |state| {
        let principal = state
            .principals
            .iter_mut()
            .find(|principal| principal.proof_binding_id == proof_binding_id)
            .ok_or_else(|| anyhow!("passkey proof binding not found"))?;
        let Some(passkey) = principal.proof_binding.passkey.as_mut() else {
            anyhow::bail!("proof binding is not a passkey");
        };
        passkey.revoked_at = Some(revoked_at);
        principal.updated_at = revoked_at;
        let record = principal.clone();
        for stored in &mut state.sessions {
            if stored.grant.proof_binding_id == proof_binding_id {
                stored.revoked_at = Some(revoked_at);
            }
        }
        Ok(record)
    })
}

pub fn promote_passkey_to_admin(
    data_dir: &Path,
    proof_binding_id: &str,
    updated_at: u64,
) -> anyhow::Result<PrincipalRecord> {
    mutate_auth_state(data_dir, |state| {
        let principal = state
            .principals
            .iter_mut()
            .find(|principal| principal.proof_binding_id == proof_binding_id)
            .ok_or_else(|| anyhow!("passkey proof binding not found"))?;
        ensure_proof_binding_not_revoked(principal)?;
        if principal.proof_binding.passkey.is_none() {
            anyhow::bail!("proof binding is not a passkey");
        }
        if principal.role == RuntimePrincipalRole::Admin {
            anyhow::bail!("passkey is already admin");
        }
        principal.role = RuntimePrincipalRole::Admin;
        principal.updated_at = updated_at;
        Ok(principal.clone())
    })
}

pub fn demote_passkey_to_guest(
    data_dir: &Path,
    proof_binding_id: &str,
    updated_at: u64,
) -> anyhow::Result<PrincipalRecord> {
    mutate_auth_state(data_dir, |state| {
        let index = state
            .principals
            .iter()
            .position(|principal| principal.proof_binding_id == proof_binding_id)
            .ok_or_else(|| anyhow!("passkey proof binding not found"))?;
        let target = &state.principals[index];
        ensure_proof_binding_not_revoked(target)?;
        if target.proof_binding.passkey.is_none() {
            anyhow::bail!("proof binding is not a passkey");
        }
        if target.role != RuntimePrincipalRole::Admin {
            anyhow::bail!("passkey is already guest");
        }
        let active_admin_count = state
            .principals
            .iter()
            .filter(|principal| {
                principal.role == RuntimePrincipalRole::Admin
                    && principal
                        .proof_binding
                        .passkey
                        .as_ref()
                        .is_some_and(|passkey| passkey.revoked_at.is_none())
            })
            .count();
        if active_admin_count <= 1 {
            anyhow::bail!("last admin passkey cannot be demoted");
        }
        let principal = &mut state.principals[index];
        principal.role = RuntimePrincipalRole::Guest;
        principal.updated_at = updated_at;
        Ok(principal.clone())
    })
}

pub fn ensure_recovered_root_reassignable(
    data_dir: &Path,
    proof_binding_id: &str,
    recovered_principal_id: &str,
    recovered_localhost_root: &str,
) -> anyhow::Result<()> {
    let mut state = load_auth_state(data_dir)?;
    normalize_principal_records(&mut state);
    ensure_recovered_root_reassignable_in_state(
        &state,
        proof_binding_id,
        recovered_principal_id,
        recovered_localhost_root,
    )?;
    Ok(())
}

pub struct RecoveredRootReassignment {
    pub proof_binding_id: String,
    pub recovered_principal_id: String,
    pub recovered_localhost_root: String,
    pub protection: PrincipalRootProtectionV1,
    pub replacement_grant: AuthSessionGrantV1,
    pub signed_audit_event: RuntimeAuditEventV1,
    pub updated_at: u64,
}

pub fn commit_recovered_root_reassignment(
    data_dir: &Path,
    reassignment: RecoveredRootReassignment,
) -> anyhow::Result<PrincipalRecord> {
    let RecoveredRootReassignment {
        proof_binding_id,
        recovered_principal_id,
        recovered_localhost_root,
        protection,
        replacement_grant,
        signed_audit_event,
        updated_at,
    } = reassignment;
    validate_principal_root_protection(&protection).map_err(anyhow::Error::msg)?;
    if protection.principal_id != recovered_principal_id
        || protection.localhost_root != recovered_localhost_root
    {
        anyhow::bail!("recovery protection does not match the recovered root");
    }
    validate_recovery_replacement_grant(
        &replacement_grant,
        &proof_binding_id,
        &recovered_principal_id,
        updated_at,
    )?;
    validate_recovery_reassignment_audit(
        &signed_audit_event,
        &proof_binding_id,
        &recovered_principal_id,
        &replacement_grant.session_id,
        updated_at,
    )?;
    let (_, runtime_did) = elastos_identity::load_or_create_did(data_dir)?;
    verify_audit_event_signature(&signed_audit_event, &runtime_did)?;

    mutate_auth_state(data_dir, |state| {
        normalize_principal_records(state);
        ensure_recovered_root_reassignable_in_state(
            state,
            &proof_binding_id,
            &recovered_principal_id,
            &recovered_localhost_root,
        )?;
        if state.sessions.iter().any(|stored| {
            stored.grant.session_id == replacement_grant.session_id
                || stored.grant.grant_id == replacement_grant.grant_id
        }) {
            anyhow::bail!("replacement recovery session collides with existing auth state");
        }

        let removed_proof_binding_ids = state
            .principals
            .iter()
            .filter(|principal| {
                principal.proof_binding_id != proof_binding_id
                    && (principal.principal_id == recovered_principal_id
                        || principal.localhost_root == recovered_localhost_root)
            })
            .map(|principal| principal.proof_binding_id.clone())
            .collect::<Vec<_>>();
        state.principals.retain(|principal| {
            principal.proof_binding_id == proof_binding_id
                || (principal.principal_id != recovered_principal_id
                    && principal.localhost_root != recovered_localhost_root)
        });
        let principal = state
            .principals
            .iter_mut()
            .find(|principal| principal.proof_binding_id == proof_binding_id)
            .ok_or_else(|| anyhow!("passkey proof binding not found after reassignment cleanup"))?;
        principal.principal_id = recovered_principal_id;
        principal.localhost_root = recovered_localhost_root;
        principal.updated_at = updated_at;
        let record = principal.clone();
        for stored in &mut state.sessions {
            if stored.grant.proof_binding_id == proof_binding_id
                || removed_proof_binding_ids
                    .iter()
                    .any(|removed| removed == &stored.grant.proof_binding_id)
            {
                stored.revoked_at = Some(updated_at);
            }
        }
        state.principal_root_protections.retain(|stored| {
            stored.principal_id != protection.principal_id
                || stored.localhost_root != protection.localhost_root
        });
        state.principal_root_protections.push(protection);
        state.sessions.push(StoredAuthSession {
            grant: replacement_grant,
            revoked_at: None,
        });
        #[cfg(test)]
        consume_recovery_reassignment_test_fault(
            data_dir,
            RecoveryReassignmentTestFault::AuditChainRejection,
        )?;
        push_audit_event(data_dir, state, signed_audit_event)?;
        Ok(record)
    })
}

fn validate_recovery_replacement_grant(
    grant: &AuthSessionGrantV1,
    proof_binding_id: &str,
    recovered_principal_id: &str,
    updated_at: u64,
) -> anyhow::Result<()> {
    if grant.schema != AuthSessionGrantV1::SCHEMA
        || grant.principal_id != recovered_principal_id
        || grant.proof_binding_id != proof_binding_id
        || grant.issued_at != updated_at
        || grant.expires_at <= grant.issued_at
    {
        anyhow::bail!("invalid replacement recovery session grant");
    }
    for (label, value) in [
        ("grant_id", grant.grant_id.as_str()),
        ("session_id", grant.session_id.as_str()),
        ("principal_id", grant.principal_id.as_str()),
        ("proof_binding_id", grant.proof_binding_id.as_str()),
    ] {
        if value.is_empty()
            || value.len() > 256
            || value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            anyhow::bail!("invalid replacement recovery {label}");
        }
    }
    if grant.apps.is_empty()
        || grant.apps.len() > 8
        || grant.apps.iter().any(|app| {
            app.is_empty()
                || app.len() > 128
                || app.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        })
    {
        anyhow::bail!("invalid replacement recovery session app scope");
    }
    let apps = grant.apps.iter().collect::<std::collections::BTreeSet<_>>();
    if apps.len() != grant.apps.len() {
        anyhow::bail!("replacement recovery session app scope contains duplicates");
    }
    Ok(())
}

fn validate_recovery_reassignment_audit(
    event: &RuntimeAuditEventV1,
    proof_binding_id: &str,
    recovered_principal_id: &str,
    replacement_session_id: &str,
    updated_at: u64,
) -> anyhow::Result<()> {
    if event.schema != RuntimeAuditEventV1::SCHEMA
        || event.event_type != "auth.recovery_kit.reassigned"
        || event.principal_id.as_deref() != Some(recovered_principal_id)
        || event.proof_binding_id.as_deref() != Some(proof_binding_id)
        || event.session_id.as_deref() != Some(replacement_session_id)
        || event.result != "ok"
        || event.occurred_at != updated_at
        || event.signer_did.as_deref().is_none_or(str::is_empty)
        || event.signature.as_deref().is_none_or(str::is_empty)
    {
        anyhow::bail!("invalid signed recovery reassignment audit event");
    }
    Ok(())
}

fn ensure_recovered_root_reassignable_in_state(
    state: &AuthState,
    proof_binding_id: &str,
    _recovered_principal_id: &str,
    _recovered_localhost_root: &str,
) -> anyhow::Result<()> {
    let current = state
        .principals
        .iter()
        .find(|principal| principal.proof_binding_id == proof_binding_id)
        .ok_or_else(|| anyhow!("passkey proof binding not found"))?;
    ensure_proof_binding_not_revoked(current)?;
    if current.proof_binding.passkey.is_none() {
        anyhow::bail!("proof binding is not a passkey");
    }
    Ok(())
}

pub fn append_audit_event(data_dir: &Path, event: RuntimeAuditEventV1) -> anyhow::Result<()> {
    let event = sign_audit_event(data_dir, event)?;
    mutate_auth_state(data_dir, |state| {
        push_audit_event(data_dir, state, event)?;
        Ok(())
    })
}

pub fn append_signed_full_recovery_outcome_audit_event(
    data_dir: &Path,
    event: RuntimeAuditEventV1,
) -> anyhow::Result<()> {
    verify_signed_full_recovery_outcome_audit_event(data_dir, &event)?;
    mutate_auth_state(data_dir, |state| {
        if let Some(existing) = state
            .audit
            .iter()
            .find(|existing| existing.event_id == event.event_id)
        {
            let same_outcome = existing.schema == event.schema
                && existing.event_type == event.event_type
                && existing.principal_id == event.principal_id
                && existing.proof_binding_id == event.proof_binding_id
                && existing.session_id == event.session_id
                && existing.challenge_id == event.challenge_id
                && existing.capsule_id == event.capsule_id
                && existing.result == event.result
                && existing.reason == event.reason
                && existing.signer_did == event.signer_did;
            if same_outcome {
                return Ok(());
            }
            anyhow::bail!("full recovery outcome audit id collision");
        }
        push_audit_event(data_dir, state, event)?;
        Ok(())
    })
}

pub fn verify_signed_full_recovery_outcome_audit_event(
    data_dir: &Path,
    event: &RuntimeAuditEventV1,
) -> anyhow::Result<()> {
    let (_, runtime_did) = elastos_identity::load_or_create_did(data_dir)?;
    verify_audit_event_signature(event, &runtime_did)
}

pub fn load_signed_full_recovery_outcome_audit_event(
    data_dir: &Path,
    event_id: &str,
) -> anyhow::Result<Option<RuntimeAuditEventV1>> {
    let event = load_auth_state(data_dir)?
        .audit
        .iter()
        .find(|event| event.event_id == event_id)
        .cloned();
    if let Some(event) = event.as_ref() {
        verify_signed_full_recovery_outcome_audit_event(data_dir, event)?;
    }
    Ok(event)
}

pub fn sign_audit_event(
    data_dir: &Path,
    mut event: RuntimeAuditEventV1,
) -> anyhow::Result<RuntimeAuditEventV1> {
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    event.signer_did = Some(signer_did);
    event.signature = None;
    let bytes = serde_json::to_vec(&event)?;
    let (signature, _) =
        crate::crypto::domain_separated_sign(&signing_key, AUDIT_EVENT_DOMAIN, &bytes);
    event.signature = Some(signature);
    Ok(event)
}

fn push_audit_event(
    data_dir: &Path,
    state: &mut AuthState,
    event: RuntimeAuditEventV1,
) -> anyhow::Result<()> {
    ensure_audit_chain_state(data_dir, state)?;
    let link = sign_audit_chain_link(data_dir, state.audit_chain.last(), &event)?;
    state.audit.push(event);
    state.audit_chain.push(link);
    retain_audit_tail(data_dir, state, AUDIT_RETENTION_LIMIT)?;
    state.audit_chain_state = Some(sign_audit_chain_state(
        data_dir,
        state
            .audit_chain_state
            .as_ref()
            .map_or_else(now_ts, |chain_state| chain_state.activated_at),
        state.audit_chain.last(),
    )?);
    Ok(())
}

fn ensure_audit_chain_state(data_dir: &Path, state: &mut AuthState) -> anyhow::Result<()> {
    if state.audit_chain_state.is_some() {
        verify_audit_chain(data_dir, state)?;
        return Ok(());
    }
    if !state.audit.is_empty()
        || !state.audit_chain.is_empty()
        || state.audit_chain_anchor.is_some()
    {
        anyhow::bail!(
            "unchained audit history is unsupported; preserve and back up the existing data root, then use a fresh data root or restore a valid anchored audit chain"
        );
    }
    state.audit_chain_state = Some(sign_audit_chain_state(
        data_dir,
        now_ts(),
        state.audit_chain.last(),
    )?);
    Ok(())
}

fn retain_audit_tail(data_dir: &Path, state: &mut AuthState, limit: usize) -> anyhow::Result<()> {
    if state.audit.len() <= limit {
        return Ok(());
    }
    let keep_from = state.audit.len() - limit;
    let predecessor = &state.audit_chain[keep_from - 1];
    state.audit_chain_anchor = Some(sign_audit_chain_anchor(
        data_dir,
        predecessor.sequence,
        &predecessor.chain_hash,
    )?);
    state.audit.drain(0..keep_from);
    state.audit_chain.drain(0..keep_from);
    Ok(())
}

fn sign_audit_chain_link(
    data_dir: &Path,
    previous: Option<&AuditChainLinkV1>,
    event: &RuntimeAuditEventV1,
) -> anyhow::Result<AuditChainLinkV1> {
    let sequence = previous.map_or(1, |link| link.sequence.saturating_add(1));
    let previous_hash = previous
        .map(|link| link.chain_hash.clone())
        .unwrap_or_else(|| AUDIT_CHAIN_GENESIS.to_string());
    let event_hash = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(event)?))
    );
    let chain_hash = audit_chain_hash(sequence, &event.event_id, &previous_hash, &event_hash)?;
    let payload = audit_chain_signature_payload(
        sequence,
        &event.event_id,
        &previous_hash,
        &event_hash,
        &chain_hash,
    )?;
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    let (signature, _) =
        crate::crypto::domain_separated_sign(&signing_key, AUDIT_CHAIN_DOMAIN, &payload);
    Ok(AuditChainLinkV1 {
        schema: AUDIT_CHAIN_SCHEMA.to_string(),
        sequence,
        event_id: event.event_id.clone(),
        previous_hash,
        event_hash,
        chain_hash,
        signer_did,
        signature,
    })
}

fn audit_chain_hash(
    sequence: u64,
    event_id: &str,
    previous_hash: &str,
    event_hash: &str,
) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema": AUDIT_CHAIN_SCHEMA,
        "sequence": sequence,
        "event_id": event_id,
        "previous_hash": previous_hash,
        "event_hash": event_hash,
    }))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn audit_chain_signature_payload(
    sequence: u64,
    event_id: &str,
    previous_hash: &str,
    event_hash: &str,
    chain_hash: &str,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "schema": AUDIT_CHAIN_SCHEMA,
        "sequence": sequence,
        "event_id": event_id,
        "previous_hash": previous_hash,
        "event_hash": event_hash,
        "chain_hash": chain_hash,
    }))?)
}

fn sign_audit_chain_state(
    data_dir: &Path,
    activated_at: u64,
    head: Option<&AuditChainLinkV1>,
) -> anyhow::Result<AuditChainStateV1> {
    let head_sequence = head.map_or(0, |link| link.sequence);
    let head_hash = head
        .map(|link| link.chain_hash.clone())
        .unwrap_or_else(|| AUDIT_CHAIN_GENESIS.to_string());
    let payload = audit_chain_state_payload(activated_at, head_sequence, &head_hash)?;
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    let (signature, _) =
        crate::crypto::domain_separated_sign(&signing_key, AUDIT_CHAIN_STATE_DOMAIN, &payload);
    Ok(AuditChainStateV1 {
        schema: AUDIT_CHAIN_STATE_SCHEMA.to_string(),
        activated_at,
        head_sequence,
        head_hash,
        signer_did,
        signature,
    })
}

fn audit_chain_state_payload(
    activated_at: u64,
    head_sequence: u64,
    head_hash: &str,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "schema": AUDIT_CHAIN_STATE_SCHEMA,
        "activated_at": activated_at,
        "head_sequence": head_sequence,
        "head_hash": head_hash,
    }))?)
}

fn sign_audit_chain_anchor(
    data_dir: &Path,
    sequence: u64,
    chain_hash: &str,
) -> anyhow::Result<AuditChainAnchorV1> {
    let payload = audit_chain_anchor_payload(sequence, chain_hash)?;
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    let (signature, _) =
        crate::crypto::domain_separated_sign(&signing_key, AUDIT_CHAIN_ANCHOR_DOMAIN, &payload);
    Ok(AuditChainAnchorV1 {
        schema: AUDIT_CHAIN_ANCHOR_SCHEMA.to_string(),
        sequence,
        chain_hash: chain_hash.to_string(),
        signer_did,
        signature,
    })
}

fn audit_chain_anchor_payload(sequence: u64, chain_hash: &str) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "schema": AUDIT_CHAIN_ANCHOR_SCHEMA,
        "sequence": sequence,
        "chain_hash": chain_hash,
    }))?)
}

fn verify_audit_event_signature(
    event: &RuntimeAuditEventV1,
    expected_runtime_did: &str,
) -> anyhow::Result<()> {
    if event.schema != RuntimeAuditEventV1::SCHEMA {
        anyhow::bail!("unsupported audit event schema");
    }
    let signer_did = event
        .signer_did
        .as_deref()
        .ok_or_else(|| anyhow!("audit event is missing its signer DID"))?;
    if signer_did != expected_runtime_did {
        anyhow::bail!("audit event signer is not the Runtime identity");
    }
    let signature = event
        .signature
        .as_deref()
        .ok_or_else(|| anyhow!("audit event is missing its signature"))?;
    let mut unsigned = event.clone();
    unsigned.signature = None;
    crate::crypto::verify_domain_separated_signature(
        expected_runtime_did,
        AUDIT_EVENT_DOMAIN,
        &serde_json::to_vec(&unsigned)?,
        signature,
    )
    .context("audit event signature is invalid")
}

fn require_runtime_audit_signer(
    actual: &str,
    expected_runtime_did: &str,
    artifact: &str,
) -> anyhow::Result<()> {
    if actual != expected_runtime_did {
        anyhow::bail!("{artifact} signer is not the Runtime identity");
    }
    Ok(())
}

fn verify_audit_chain(data_dir: &Path, state: &AuthState) -> anyhow::Result<()> {
    if state.audit.is_empty()
        && state.audit_chain.is_empty()
        && state.audit_chain_state.is_none()
        && state.audit_chain_anchor.is_none()
    {
        return Ok(());
    }
    let (_, expected_runtime_did) = elastos_identity::load_or_create_did(data_dir)?;
    if let Some(chain_state) = state.audit_chain_state.as_ref() {
        if chain_state.schema != AUDIT_CHAIN_STATE_SCHEMA {
            anyhow::bail!("unsupported audit chain state schema");
        }
        require_runtime_audit_signer(
            &chain_state.signer_did,
            &expected_runtime_did,
            "audit chain state",
        )?;
        let payload = audit_chain_state_payload(
            chain_state.activated_at,
            chain_state.head_sequence,
            &chain_state.head_hash,
        )?;
        crate::crypto::verify_domain_separated_signature(
            &expected_runtime_did,
            AUDIT_CHAIN_STATE_DOMAIN,
            &payload,
            &chain_state.signature,
        )
        .context("audit chain state signature is invalid")?;
        let (head_sequence, head_hash) = state
            .audit_chain
            .last()
            .map(|link| (link.sequence, link.chain_hash.as_str()))
            .unwrap_or((0, AUDIT_CHAIN_GENESIS));
        if chain_state.head_sequence != head_sequence || chain_state.head_hash != head_hash {
            anyhow::bail!("audit chain state does not match the persisted chain head");
        }
    } else if state.audit_chain_anchor.is_some() {
        anyhow::bail!("audit chain anchor requires explicit chain state");
    }
    if state.audit_chain.is_empty() {
        if !state.audit.is_empty() && state.audit_chain_state.is_some() {
            anyhow::bail!("audit chain is required after activation");
        }
        return Ok(());
    }
    if state.audit_chain.len() != state.audit.len() {
        anyhow::bail!("audit chain length does not match persisted audit events");
    }
    let protected_chain = state.audit_chain_state.is_some();
    let anchor = state.audit_chain_anchor.as_ref();
    if let Some(anchor) = anchor {
        if anchor.schema != AUDIT_CHAIN_ANCHOR_SCHEMA {
            anyhow::bail!("unsupported audit chain anchor schema");
        }
        require_runtime_audit_signer(
            &anchor.signer_did,
            &expected_runtime_did,
            "audit chain anchor",
        )?;
        let payload = audit_chain_anchor_payload(anchor.sequence, &anchor.chain_hash)?;
        crate::crypto::verify_domain_separated_signature(
            &expected_runtime_did,
            AUDIT_CHAIN_ANCHOR_DOMAIN,
            &payload,
            &anchor.signature,
        )
        .context("audit chain anchor signature is invalid")?;
    }
    let mut previous: Option<&AuditChainLinkV1> = None;
    for (event, link) in state.audit.iter().zip(&state.audit_chain) {
        verify_audit_event_signature(event, &expected_runtime_did)?;
        if link.schema != AUDIT_CHAIN_SCHEMA || link.event_id != event.event_id {
            anyhow::bail!("audit chain event binding is invalid");
        }
        require_runtime_audit_signer(&link.signer_did, &expected_runtime_did, "audit chain link")?;
        if let Some(previous) = previous {
            if link.sequence != previous.sequence.saturating_add(1)
                || link.previous_hash != previous.chain_hash
            {
                anyhow::bail!("audit chain sequence is invalid");
            }
        } else if let Some(anchor) = anchor {
            if link.sequence != anchor.sequence.saturating_add(1)
                || link.previous_hash != anchor.chain_hash
            {
                anyhow::bail!("audit chain does not continue from its retained anchor");
            }
        } else if protected_chain
            && (link.sequence != 1 || link.previous_hash != AUDIT_CHAIN_GENESIS)
        {
            anyhow::bail!("protected audit chain is missing its retained anchor");
        } else if link.sequence == 1 && link.previous_hash != AUDIT_CHAIN_GENESIS {
            anyhow::bail!("audit chain genesis is invalid");
        }
        let event_hash = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(serde_json::to_vec(event)?))
        );
        if link.event_hash != event_hash
            || link.chain_hash
                != audit_chain_hash(
                    link.sequence,
                    &link.event_id,
                    &link.previous_hash,
                    &link.event_hash,
                )?
        {
            anyhow::bail!("audit chain hash is invalid");
        }
        let payload = audit_chain_signature_payload(
            link.sequence,
            &link.event_id,
            &link.previous_hash,
            &link.event_hash,
            &link.chain_hash,
        )?;
        crate::crypto::verify_domain_separated_signature(
            &expected_runtime_did,
            AUDIT_CHAIN_DOMAIN,
            &payload,
            &link.signature,
        )
        .context("audit chain signature is invalid")?;
        previous = Some(link);
    }
    Ok(())
}

pub fn local_person_principal_id(proof_binding_id: &str) -> String {
    let digest = Sha256::digest(proof_binding_id.as_bytes());
    format!("person:local:{}", hex::encode(&digest[..16]))
}

pub fn passkey_credential_principal_id(rp_id: &str, credential_id: &str) -> anyhow::Result<String> {
    if rp_id.trim().is_empty()
        || credential_id.trim().is_empty()
        || rp_id
            .chars()
            .chain(credential_id.chars())
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("invalid passkey credential principal input");
    }
    let digest = Sha256::digest(format!("passkey-credential:{rp_id}:{credential_id}").as_bytes());
    Ok(format!("person:local:{}", hex::encode(&digest[..16])))
}

pub fn principal_localhost_root(principal_id: &str) -> String {
    let digest = Sha256::digest(principal_id.as_bytes());
    format!("localhost://Users/{}", hex::encode(&digest[..12]))
}

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn principal_root_data_key_from_protection(
    data_dir: &Path,
    protection: &PrincipalRootProtectionV1,
) -> anyhow::Result<[u8; 32]> {
    if protection.crypto.cipher != DEFAULT_PRINCIPAL_ROOT_CIPHER {
        anyhow::bail!("unsupported principal-root cipher");
    }
    let archive = protection
        .protectors
        .iter()
        .find(|protector| protector.kind == PrincipalRootProtectorKind::RecoveryKit)
        .and_then(|protector| protector.archive.as_ref())
        .ok_or_else(|| anyhow!("principal-root protection has no recoverable data key archive"))?;
    let kit = recovery_kit_from_archive(data_dir, archive)?;
    verify_recovery_kit_material(&kit)?;
    if kit.principal_id != protection.principal_id
        || kit.localhost_root != protection.localhost_root
        || kit.data_key_id != protection.data_key_id
    {
        anyhow::bail!("recovery kit archive is not bound to this principal root");
    }
    recovery_kit_data_key(&kit)
}

fn validate_principal_root_object_binding(
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
) -> anyhow::Result<()> {
    if principal_id.trim().is_empty() {
        anyhow::bail!("principal id must not be empty");
    }
    if !localhost_root.starts_with("localhost://Users/") {
        anyhow::bail!("principal localhost root must be under localhost://Users/");
    }
    let under_root = object_uri == localhost_root
        || object_uri
            .strip_prefix(localhost_root)
            .is_some_and(|rest| rest.starts_with('/'));
    if !under_root {
        anyhow::bail!("principal-root object URI is outside the principal root");
    }
    Ok(())
}

fn validate_principal_root_object_path(
    data_dir: &Path,
    object_uri: &str,
    path: &Path,
) -> anyhow::Result<()> {
    let expected = rooted_localhost_fs_path(data_dir, object_uri)
        .ok_or_else(|| anyhow!("principal-root object URI has no canonical filesystem path"))?;
    if path != expected {
        anyhow::bail!("principal-root object path does not match its canonical URI path")
    }
    Ok(())
}

#[derive(Debug)]
struct DeclaredPrincipalRootObject {
    object_uri: String,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct ClassifiedDeclaredPrincipalRootObjects {
    declared_object_count: usize,
    plaintext_object_count: usize,
    plaintext_objects: Vec<PrincipalRootMigrationSelectionV1>,
    envelopes: Vec<(String, PrincipalRootObjectEnvelopeV1)>,
}

fn classify_declared_principal_root_objects(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<ClassifiedDeclaredPrincipalRootObjects> {
    let declared =
        collect_declared_principal_root_objects(data_dir, principal_id, localhost_root, inventory)?;
    let declared_object_count = declared.len();
    let mut plaintext_object_count = 0usize;
    let mut plaintext_objects = Vec::new();
    let mut envelopes = Vec::new();
    for object in declared {
        match serde_json::from_slice::<PrincipalRootObjectEnvelopeV1>(&object.bytes) {
            Ok(envelope) => envelopes.push((object.object_uri, envelope)),
            Err(_) if looks_like_principal_root_object_envelope(&object.bytes) => {
                anyhow::bail!("declared principal-root object envelope is malformed");
            }
            Err(_) => {
                plaintext_object_count = plaintext_object_count.saturating_add(1);
                plaintext_objects.push(PrincipalRootMigrationSelectionV1 {
                    object_uri: object.object_uri,
                    plaintext_sha256: sha256_label(&object.bytes),
                });
            }
        }
    }
    Ok(ClassifiedDeclaredPrincipalRootObjects {
        declared_object_count,
        plaintext_object_count,
        plaintext_objects,
        envelopes,
    })
}

fn require_no_declared_principal_root_plaintext(
    principal_id: &str,
    localhost_root: &str,
    plaintext_object_count: usize,
) -> anyhow::Result<()> {
    if plaintext_object_count > 0 {
        return Err(PrincipalRootMigrationRequiredV1 {
            schema: PRINCIPAL_ROOT_MIGRATION_REQUIRED_SCHEMA.to_string(),
            status: "migration_required".to_string(),
            code: "declared_plaintext_objects".to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            plaintext_object_count,
        }
        .into());
    }
    Ok(())
}

fn verify_classified_principal_root_objects_with_stored_protection(
    data_dir: &Path,
    protection: Option<&PrincipalRootProtectionV1>,
    classified: &ClassifiedDeclaredPrincipalRootObjects,
) -> anyhow::Result<()> {
    if classified.envelopes.is_empty() {
        return Ok(());
    }
    let protection = protection.ok_or_else(|| {
        anyhow!(
            "an exact verified protection binding is required for declared encrypted principal-root objects"
        )
    })?;
    let data_key = principal_root_data_key_from_protection(data_dir, protection)?;
    verify_classified_principal_root_object_envelopes(
        classified,
        &protection.principal_id,
        &protection.localhost_root,
        &protection.data_key_id,
        &data_key,
    )
}

fn verify_classified_principal_root_object_envelopes(
    classified: &ClassifiedDeclaredPrincipalRootObjects,
    principal_id: &str,
    localhost_root: &str,
    data_key_id: &str,
    data_key: &[u8; 32],
) -> anyhow::Result<()> {
    for (object_uri, envelope) in &classified.envelopes {
        validate_principal_root_object_envelope(
            envelope,
            principal_id,
            localhost_root,
            data_key_id,
            object_uri,
        )
        .context("declared principal-root object envelope binding is invalid")?;
        let nonce = b64_url_decode(&envelope.nonce)
            .context("declared principal-root object envelope nonce is invalid")?;
        if nonce.len() != 12 {
            anyhow::bail!("declared principal-root object envelope nonce must be 12 bytes");
        }
        let ciphertext = b64_url_decode(&envelope.ciphertext)
            .context("declared principal-root object envelope ciphertext is invalid")?;
        if ciphertext.is_empty() {
            anyhow::bail!("declared principal-root object envelope ciphertext is empty");
        }
        decrypt_aes256_gcm_bytes_with_aad(
            data_key,
            &nonce,
            &ciphertext,
            principal_root_object_aad(envelope).as_bytes(),
        )
        .context("declared principal-root object envelope authentication failed")?;
    }
    Ok(())
}

fn looks_like_principal_root_object_envelope(bytes: &[u8]) -> bool {
    if let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(bytes) {
        return object
            .get("schema")
            .and_then(Value::as_str)
            .is_some_and(|schema| schema.starts_with("elastos.principal-root.object/"))
            || ["data_key_id", "nonce", "ciphertext"]
                .iter()
                .all(|field| object.contains_key(*field));
    }
    std::str::from_utf8(bytes).is_ok_and(|text| {
        (text.contains("\"schema\"") && text.contains("elastos.principal-root.object/"))
            || ["\"data_key_id\"", "\"nonce\"", "\"ciphertext\""]
                .iter()
                .all(|field| text.contains(field))
    })
}

fn collect_declared_principal_root_objects(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    inventory: &[PrincipalRootProtectedObjectDeclarationV1],
) -> anyhow::Result<Vec<DeclaredPrincipalRootObject>> {
    if inventory.len() > MAX_PRINCIPAL_ROOT_INVENTORY_DECLARATIONS {
        anyhow::bail!("principal-root protected-object inventory exceeds its declaration bound");
    }
    let mut declared = std::collections::BTreeMap::<String, PathBuf>::new();
    for declaration in inventory {
        let uri = declaration.uri();
        validate_principal_root_object_binding(principal_id, localhost_root, uri)?;
        let path = rooted_localhost_fs_path(data_dir, uri)
            .ok_or_else(|| anyhow!("declared principal-root object path is invalid"))?;
        ensure_no_symlink_components(data_dir, &path)?;
        match declaration {
            PrincipalRootProtectedObjectDeclarationV1::Exact { object_uri } => {
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(err) if err.kind() == ErrorKind::NotFound => continue,
                    Err(err) => {
                        return Err(err).context("failed to inspect declared principal-root object")
                    }
                };
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    anyhow::bail!(
                        "declared principal-root object must be a regular non-symlink file"
                    );
                }
                declared.insert(object_uri.clone(), path);
            }
            PrincipalRootProtectedObjectDeclarationV1::Root { root_uri } => {
                collect_declared_principal_root_files(&path, root_uri, 0, &mut declared)?;
            }
        }
        if declared.len() > MAX_PRINCIPAL_ROOT_INVENTORY_OBJECTS {
            anyhow::bail!("principal-root protected-object inventory exceeds its object bound");
        }
    }

    let mut objects = Vec::with_capacity(declared.len());
    let mut total_bytes = 0u64;
    for (object_uri, path) in declared {
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect declared object {path:?}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            anyhow::bail!("declared principal-root object must remain a regular non-symlink file");
        }
        if metadata.len() > MAX_MIGRATION_OBJECT_BYTES {
            anyhow::bail!("declared principal-root object exceeds 16 MiB");
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| anyhow!("declared principal-root object byte total overflow"))?;
        if total_bytes > MAX_MIGRATION_TOTAL_BYTES {
            anyhow::bail!("declared principal-root objects exceed 64 MiB");
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read declared object {path:?}"))?;
        objects.push(DeclaredPrincipalRootObject { object_uri, bytes });
    }
    Ok(objects)
}

fn collect_declared_principal_root_files(
    path: &Path,
    root_uri: &str,
    depth: usize,
    declared: &mut std::collections::BTreeMap<String, PathBuf>,
) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context("failed to inspect declared principal-root root"),
    };
    if metadata.file_type().is_symlink() {
        anyhow::bail!("declared principal-root inventory rejects symlinks");
    }
    if metadata.is_file() {
        declared.insert(root_uri.to_string(), path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        anyhow::bail!("declared principal-root root must be a regular file or directory");
    }
    if depth >= MAX_PRINCIPAL_ROOT_INVENTORY_DEPTH {
        anyhow::bail!("principal-root protected-object inventory exceeds its depth bound");
    }

    let mut entries = std::fs::read_dir(path)?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to enumerate declared principal-root root")?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("declared principal-root object name must be UTF-8"))?;
        let child_uri = format!("{root_uri}/{name}");
        let child_path = entry.path();
        let child_metadata = std::fs::symlink_metadata(&child_path)?;
        if child_metadata.file_type().is_symlink() {
            anyhow::bail!("declared principal-root inventory rejects symlinks");
        }
        if child_metadata.is_dir() {
            collect_declared_principal_root_files(&child_path, &child_uri, depth + 1, declared)?;
        } else if child_metadata.is_file() {
            declared.insert(child_uri, child_path);
        } else {
            anyhow::bail!("declared principal-root inventory rejects special files");
        }
        if declared.len() > MAX_PRINCIPAL_ROOT_INVENTORY_OBJECTS {
            anyhow::bail!("principal-root protected-object inventory exceeds its object bound");
        }
    }
    Ok(())
}

fn is_bound_principal_root_object_envelope(
    bytes: &[u8],
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
) -> bool {
    let Ok(envelope) = serde_json::from_slice::<PrincipalRootObjectEnvelopeV1>(bytes) else {
        return false;
    };
    if validate_principal_root_object_envelope(
        &envelope,
        principal_id,
        localhost_root,
        &envelope.data_key_id,
        object_uri,
    )
    .is_err()
    {
        return false;
    }
    b64_url_decode(&envelope.nonce).is_ok_and(|nonce| nonce.len() == 12)
        && b64_url_decode(&envelope.ciphertext).is_ok_and(|ciphertext| !ciphertext.is_empty())
}

fn validate_principal_root_object_envelope(
    envelope: &PrincipalRootObjectEnvelopeV1,
    principal_id: &str,
    localhost_root: &str,
    data_key_id: &str,
    object_uri: &str,
) -> anyhow::Result<()> {
    if envelope.schema != PRINCIPAL_ROOT_OBJECT_SCHEMA {
        anyhow::bail!("unsupported principal-root object schema");
    }
    if envelope.cipher != DEFAULT_PRINCIPAL_ROOT_CIPHER {
        anyhow::bail!("unsupported principal-root object cipher");
    }
    if envelope.principal_id != principal_id
        || envelope.localhost_root != localhost_root
        || envelope.data_key_id != data_key_id
        || envelope.object_uri != object_uri
    {
        anyhow::bail!("principal-root object binding mismatch");
    }
    Ok(())
}

fn principal_root_object_aad(envelope: &PrincipalRootObjectEnvelopeV1) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        PRINCIPAL_ROOT_OBJECT_AAD_DOMAIN,
        envelope.schema,
        envelope.principal_id,
        envelope.localhost_root,
        envelope.data_key_id,
        envelope.object_uri
    )
}

fn decrypt_root_descriptor(kit: &RecoveryKitV1, data_key: &[u8]) -> anyhow::Result<Value> {
    let Some(rest) = kit
        .encrypted_root_descriptor
        .strip_prefix("aes-256-gcm:v1:")
    else {
        anyhow::bail!("unsupported encrypted root descriptor envelope");
    };
    let mut parts = rest.splitn(2, ':');
    let nonce = parts
        .next()
        .ok_or_else(|| anyhow!("encrypted root descriptor nonce missing"))
        .and_then(b64_url_decode)?;
    let ciphertext = parts
        .next()
        .ok_or_else(|| anyhow!("encrypted root descriptor ciphertext missing"))
        .and_then(b64_url_decode)?;
    if nonce.len() != 12 {
        anyhow::bail!("encrypted root descriptor nonce must be 12 bytes");
    }
    let plaintext = decrypt_aes256_gcm_bytes(data_key, &nonce, &ciphertext)?;
    serde_json::from_slice(&plaintext).map_err(Into::into)
}

fn encrypt_aes256_gcm_bytes_with_aad(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> anyhow::Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow!("principal-root encryption failed"))?;
    Ok(b64_url(&ciphertext))
}

fn decrypt_aes256_gcm_bytes_with_aad(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("principal-root decrypt failed"))
}

pub fn migrate_principal_root_objects_offline(
    data_dir: &Path,
    plan_path: &Path,
    backup_dir: &Path,
) -> anyhow::Result<PrincipalRootMigrationReceiptV1> {
    if !data_dir.is_absolute() || !plan_path.is_absolute() || !backup_dir.is_absolute() {
        anyhow::bail!("principal-root migration data, plan, and backup paths must all be absolute");
    }
    let _host_guard = crate::host_lock::acquire_host_process_lock(
        data_dir,
        "principal-root-migration",
        "offline",
    )?;
    let _object_guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    recover_principal_root_migration_journal(data_dir)?;

    match migrate_principal_root_objects_offline_locked(data_dir, plan_path, backup_dir) {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            #[cfg(test)]
            if error
                .downcast_ref::<SimulatedPrincipalRootMigrationCrash>()
                .is_some()
            {
                return Err(error);
            }
            if let Err(recovery_error) = recover_principal_root_migration_journal(data_dir) {
                return Err(anyhow!(
                    "{error}; principal-root migration recovery also failed: {recovery_error}"
                ));
            }
            Err(error)
        }
    }
}

fn migrate_principal_root_objects_offline_locked(
    data_dir: &Path,
    plan_path: &Path,
    backup_dir: &Path,
) -> anyhow::Result<PrincipalRootMigrationReceiptV1> {
    let plan_metadata = std::fs::symlink_metadata(plan_path)
        .with_context(|| format!("failed to inspect migration plan {plan_path:?}"))?;
    if plan_metadata.file_type().is_symlink() || !plan_metadata.is_file() {
        anyhow::bail!("principal-root migration plan must be a regular non-symlink file");
    }
    if plan_metadata.len() > 256 * 1024 {
        anyhow::bail!("principal-root migration plan exceeds 256 KiB");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if plan_metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("principal-root migration plan must be owner-only");
        }
    }
    let plan_bytes = std::fs::read(plan_path)?;
    let plan_sha256 = sha256_label(&plan_bytes);
    let plan: PrincipalRootMigrationPlanV1 = serde_json::from_slice(&plan_bytes)
        .context("failed to decode principal-root migration plan")?;
    validate_principal_root_migration_plan(&plan)?;
    migrate_principal_root_objects_plan_locked(data_dir, &plan, &plan_sha256, backup_dir)
}

fn migrate_principal_root_objects_plan_locked(
    data_dir: &Path,
    plan: &PrincipalRootMigrationPlanV1,
    plan_sha256: &str,
    backup_dir: &Path,
) -> anyhow::Result<PrincipalRootMigrationReceiptV1> {
    let protection =
        load_principal_root_protection(data_dir, &plan.principal_id, &plan.localhost_root)?
            .ok_or_else(|| {
                anyhow!("principal-root migration requires existing verified Runtime protection")
            })?;
    let data_key = principal_root_data_key_from_protection(data_dir, &protection)?;

    if backup_dir.exists() {
        anyhow::bail!("principal-root migration backup directory must not already exist");
    }
    let journal_path = principal_root_migration_journal_path(data_dir)?;
    if journal_path.exists() {
        anyhow::bail!("principal-root migration journal already exists");
    }
    let stage_dir = journal_path.with_file_name(format!(
        ".principal-root-migration-{}.stage",
        &plan_sha256["sha256:".len()..][..24]
    ));
    if stage_dir.exists() {
        anyhow::bail!("principal-root migration stage directory already exists");
    }

    let mut prepared = Vec::with_capacity(plan.objects.len());
    let mut total_bytes = 0u64;
    for (index, selection) in plan.objects.iter().enumerate() {
        let path = rooted_localhost_fs_path(data_dir, &selection.object_uri)
            .ok_or_else(|| anyhow!("migration object path is invalid"))?;
        ensure_no_symlink_components(data_dir, &path)?;
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect selected migration object {path:?}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            anyhow::bail!("selected migration object must be a regular non-symlink file");
        }
        if metadata.len() > MAX_MIGRATION_OBJECT_BYTES {
            anyhow::bail!("selected migration object exceeds 16 MiB");
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| anyhow!("migration object byte total overflow"))?;
        if total_bytes > MAX_MIGRATION_TOTAL_BYTES {
            anyhow::bail!("selected migration objects exceed 64 MiB");
        }
        let plaintext = std::fs::read(&path)?;
        let plaintext_sha256 = sha256_label(&plaintext);
        if plaintext_sha256 != selection.plaintext_sha256 {
            anyhow::bail!("selected migration object plaintext hash mismatch");
        }
        if is_bound_principal_root_object_envelope(
            &plaintext,
            &plan.principal_id,
            &plan.localhost_root,
            &selection.object_uri,
        ) {
            anyhow::bail!("selected migration object is already encrypted");
        }
        let backup_name = format!(
            "{index:03}-{}.plaintext.backup",
            &hex::encode(Sha256::digest(selection.object_uri.as_bytes()))[..24]
        );
        let backup_path = backup_dir.join(backup_name);
        let stage_path = stage_dir.join(format!("{index:03}.encrypted"));
        let encrypted = encrypt_principal_root_object_bytes(
            &plan.principal_id,
            &plan.localhost_root,
            &selection.object_uri,
            &protection,
            &data_key,
            &plaintext,
        )?;
        verify_migrated_principal_root_object(
            &encrypted,
            &plan.principal_id,
            &plan.localhost_root,
            &selection.object_uri,
            &protection,
            &data_key,
            &selection.plaintext_sha256,
        )?;
        prepared.push(PreparedPrincipalRootMigrationObject {
            journal: PrincipalRootMigrationJournalObjectV1 {
                object_uri: selection.object_uri.clone(),
                plaintext_sha256: selection.plaintext_sha256.clone(),
                backup_path,
                backup_sha256: plaintext_sha256,
                stage_path,
                encrypted_sha256: sha256_label(&encrypted),
                original_mode: principal_root_file_mode(&metadata),
            },
            path,
            plaintext,
            encrypted,
        });
    }

    create_owner_only_dir(backup_dir)?;
    create_owner_only_dir(&stage_dir)?;
    let mut journal = PrincipalRootMigrationJournalV1 {
        schema: PRINCIPAL_ROOT_MIGRATION_JOURNAL_SCHEMA.to_string(),
        phase: PrincipalRootMigrationJournalPhaseV1::Prepared,
        plan_sha256: plan_sha256.to_string(),
        principal_id: plan.principal_id.clone(),
        localhost_root: plan.localhost_root.clone(),
        data_key_id: protection.data_key_id.clone(),
        backup_dir: backup_dir.to_path_buf(),
        stage_dir,
        objects: prepared
            .iter()
            .map(|object| object.journal.clone())
            .collect(),
    };
    write_principal_root_migration_journal(data_dir, &journal)?;
    #[cfg(test)]
    consume_principal_root_migration_test_fault(
        data_dir,
        PrincipalRootMigrationTestFault::JournalPrepared,
    )?;

    for (index, object) in prepared.iter().enumerate() {
        write_owner_only_new_file(&object.journal.backup_path, &object.plaintext)?;
        if sha256_label(&std::fs::read(&object.journal.backup_path)?)
            != object.journal.backup_sha256
        {
            anyhow::bail!("principal-root migration backup verification failed");
        }
        maybe_inject_principal_root_migration_indexed_fault(
            data_dir,
            PrincipalRootMigrationIndexedFaultStage::Backup,
            index,
        )?;
    }
    for (index, object) in prepared.iter().enumerate() {
        write_owner_only_new_file(&object.journal.stage_path, &object.encrypted)?;
        let staged = std::fs::read(&object.journal.stage_path)?;
        if sha256_label(&staged) != object.journal.encrypted_sha256 {
            anyhow::bail!("principal-root migration staged ciphertext hash mismatch");
        }
        verify_migrated_principal_root_object(
            &staged,
            &plan.principal_id,
            &plan.localhost_root,
            &object.journal.object_uri,
            &protection,
            &data_key,
            &object.journal.plaintext_sha256,
        )?;
        maybe_inject_principal_root_migration_indexed_fault(
            data_dir,
            PrincipalRootMigrationIndexedFaultStage::Stage,
            index,
        )?;
    }

    journal.phase = PrincipalRootMigrationJournalPhaseV1::Committing;
    write_principal_root_migration_journal(data_dir, &journal)?;
    #[cfg(test)]
    consume_principal_root_migration_test_fault(
        data_dir,
        PrincipalRootMigrationTestFault::JournalCommitting,
    )?;

    for (index, object) in prepared.iter().enumerate() {
        verify_plaintext_migration_target(data_dir, object)?;
        let staged = std::fs::read(&object.journal.stage_path)?;
        if sha256_label(&staged) != object.journal.encrypted_sha256 {
            anyhow::bail!("principal-root migration stage changed before commit");
        }
        verify_migrated_principal_root_object(
            &staged,
            &journal.principal_id,
            &journal.localhost_root,
            &object.journal.object_uri,
            &protection,
            &data_key,
            &object.journal.plaintext_sha256,
        )?;
        tempfile::TempPath::try_from_path(object.journal.stage_path.clone())?
            .persist(&object.path)
            .map_err(|error| error.error)
            .context("failed to atomically commit staged principal-root object")?;
        sync_parent_directory(&object.path)?;
        maybe_inject_principal_root_migration_indexed_fault(
            data_dir,
            PrincipalRootMigrationIndexedFaultStage::Commit,
            index,
        )?;
    }

    for object in &journal.objects {
        verify_journal_encrypted_object(data_dir, &journal, object, &protection, &data_key)?;
    }
    journal.phase = PrincipalRootMigrationJournalPhaseV1::Committed;
    write_principal_root_migration_journal(data_dir, &journal)?;
    #[cfg(test)]
    consume_principal_root_migration_test_fault(
        data_dir,
        PrincipalRootMigrationTestFault::JournalCommitted,
    )?;

    let receipt = principal_root_migration_receipt_from_journal(&journal);
    write_principal_root_migration_receipt(&journal, &receipt)?;
    cleanup_principal_root_migration_journal(data_dir, &journal)?;
    Ok(receipt)
}

pub fn verify_declared_principal_roots_ready(
    data_dir: &Path,
    declarations: &[PrincipalRootUpgradeDeclarationV1],
) -> anyhow::Result<()> {
    let _object_guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    require_no_principal_root_migration_journal(data_dir)?;
    let plans = preflight_declared_principal_root_upgrade(data_dir, declarations)?;
    if let Some(plan) = plans.first() {
        require_no_declared_principal_root_plaintext(
            &plan.principal_id,
            &plan.localhost_root,
            plan.objects.len(),
        )?;
    }
    Ok(())
}

pub fn migrate_declared_principal_roots_offline<F>(
    data_dir: &Path,
    backup_dir: &Path,
    declarations: F,
) -> anyhow::Result<PrincipalRootUpgradeReceiptV1>
where
    F: FnOnce() -> anyhow::Result<Vec<PrincipalRootUpgradeDeclarationV1>>,
{
    if !data_dir.is_absolute() || !backup_dir.is_absolute() {
        anyhow::bail!("principal-root upgrade data and backup paths must be absolute");
    }
    let expected_backup_parent = data_dir.join("backups");
    if backup_dir.parent() != Some(expected_backup_parent.as_path()) {
        anyhow::bail!("principal-root upgrade backup must be a direct child of data-dir/backups");
    }
    let _host_guard =
        crate::host_lock::acquire_host_process_lock(data_dir, "principal-root-upgrade", "offline")?;
    let _object_guard = principal_root_object_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("principal-root object mutation lock poisoned"))?;
    let declarations = declarations()?;
    if recover_principal_root_migration_journal(data_dir)? {
        anyhow::bail!(
            "recovered an incomplete principal-root migration; rerun the upgrade after reviewing the retained owner-only backup"
        );
    }

    let plans = preflight_declared_principal_root_upgrade(data_dir, &declarations)?;
    if plans.is_empty() {
        return Ok(PrincipalRootUpgradeReceiptV1 {
            schema: PRINCIPAL_ROOT_UPGRADE_RECEIPT_SCHEMA.to_string(),
            status: "already_ready".to_string(),
            root_count: 0,
            object_count: 0,
            roots: Vec::new(),
        });
    }
    if backup_dir.exists() {
        anyhow::bail!("principal-root upgrade backup directory must not already exist");
    }
    ensure_no_symlink_components(data_dir, backup_dir)?;
    ensure_safe_backup_parent(&expected_backup_parent)?;
    create_owner_only_dir(backup_dir)?;

    let mut roots = Vec::with_capacity(plans.len());
    for (index, plan) in plans.iter().enumerate() {
        let plan_bytes = serde_json::to_vec_pretty(plan)?;
        let plan_sha256 = sha256_label(&plan_bytes);
        let root_backup_dir = backup_dir.join(format!("root-{index:03}"));
        match migrate_principal_root_objects_plan_locked(
            data_dir,
            plan,
            &plan_sha256,
            &root_backup_dir,
        ) {
            Ok(receipt) => roots.push(receipt),
            Err(error) => {
                if let Err(recovery_error) = recover_principal_root_migration_journal(data_dir) {
                    return Err(anyhow!(
                        "{error}; principal-root upgrade recovery also failed: {recovery_error}"
                    ));
                }
                return Err(error);
            }
        }
    }

    let remaining = preflight_declared_principal_root_upgrade(data_dir, &declarations)?;
    if !remaining.is_empty() {
        anyhow::bail!("principal-root upgrade left declared plaintext objects");
    }
    let receipt = PrincipalRootUpgradeReceiptV1 {
        schema: PRINCIPAL_ROOT_UPGRADE_RECEIPT_SCHEMA.to_string(),
        status: "migrated".to_string(),
        root_count: roots.len(),
        object_count: roots.iter().map(|root| root.object_count).sum(),
        roots,
    };
    let receipt_path = backup_dir.join(PRINCIPAL_ROOT_UPGRADE_RECEIPT_FILE);
    write_owner_only_new_file(&receipt_path, &serde_json::to_vec_pretty(&receipt)?)?;
    sync_parent_directory(&receipt_path)?;
    Ok(receipt)
}

fn preflight_declared_principal_root_upgrade(
    data_dir: &Path,
    declarations: &[PrincipalRootUpgradeDeclarationV1],
) -> anyhow::Result<Vec<PrincipalRootMigrationPlanV1>> {
    let state = load_auth_state(data_dir)?;
    let mut protections = std::collections::BTreeMap::new();
    for protection in state.principal_root_protections {
        validate_principal_root_protection(&protection).map_err(anyhow::Error::msg)?;
        let key = (
            protection.principal_id.clone(),
            protection.localhost_root.clone(),
        );
        if protections.insert(key, protection).is_some() {
            anyhow::bail!("configured principal-root protections contain a duplicate binding");
        }
    }
    let mut declared = std::collections::BTreeMap::new();
    for declaration in declarations {
        let key = (
            declaration.principal_id.clone(),
            declaration.localhost_root.clone(),
        );
        if declared.insert(key, declaration).is_some() {
            anyhow::bail!("principal-root upgrade declarations contain a duplicate binding");
        }
    }
    if protections.keys().ne(declared.keys()) {
        anyhow::bail!(
            "principal-root upgrade declarations must exactly match configured protections"
        );
    }

    let mut plans = Vec::new();
    for (key, protection) in protections {
        let declaration = declared
            .get(&key)
            .ok_or_else(|| anyhow!("principal-root upgrade declaration disappeared"))?;
        let classified = classify_declared_principal_root_objects(
            data_dir,
            &declaration.principal_id,
            &declaration.localhost_root,
            &declaration.inventory,
        )?;
        verify_classified_principal_root_objects_with_stored_protection(
            data_dir,
            Some(&protection),
            &classified,
        )?;
        if !classified.plaintext_objects.is_empty() {
            let plan = PrincipalRootMigrationPlanV1 {
                schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
                principal_id: declaration.principal_id.clone(),
                localhost_root: declaration.localhost_root.clone(),
                objects: classified.plaintext_objects,
            };
            validate_principal_root_migration_plan(&plan)?;
            plans.push(plan);
        }
    }
    Ok(plans)
}

fn require_no_principal_root_migration_journal(data_dir: &Path) -> anyhow::Result<()> {
    let journal_path = principal_root_migration_journal_path(data_dir)?;
    match std::fs::symlink_metadata(&journal_path) {
        Ok(_) => {
            anyhow::bail!(
                "principal-root migration is incomplete; Home readiness is blocked until offline recovery completes"
            )
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).context("failed to inspect the principal-root migration readiness journal")
        }
    }
}

fn ensure_safe_backup_parent(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                anyhow::bail!(
                    "principal-root upgrade backup parent must be a non-symlink directory"
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => create_owner_only_dir(path),
        Err(error) => Err(error).context("failed to inspect principal-root upgrade backup parent"),
    }
}

fn validate_principal_root_migration_plan(
    plan: &PrincipalRootMigrationPlanV1,
) -> anyhow::Result<()> {
    if plan.schema != PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA {
        anyhow::bail!("unsupported principal-root migration plan schema");
    }
    validate_principal_root_object_binding(
        &plan.principal_id,
        &plan.localhost_root,
        &plan.localhost_root,
    )?;
    if plan.objects.is_empty() || plan.objects.len() > MAX_MIGRATION_OBJECTS {
        anyhow::bail!("migration plan must select 1-64 objects");
    }
    let mut uris = std::collections::BTreeSet::new();
    for selection in &plan.objects {
        validate_principal_root_object_binding(
            &plan.principal_id,
            &plan.localhost_root,
            &selection.object_uri,
        )?;
        if !is_sha256_label(&selection.plaintext_sha256) {
            anyhow::bail!("migration plaintext hash must be sha256:<64 lowercase hex>");
        }
        if !uris.insert(selection.object_uri.as_str()) {
            anyhow::bail!("migration plan contains duplicate object URIs");
        }
    }
    Ok(())
}

fn encrypt_principal_root_object_bytes(
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    protection: &PrincipalRootProtectionV1,
    data_key: &[u8; 32],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut envelope = PrincipalRootObjectEnvelopeV1 {
        schema: PRINCIPAL_ROOT_OBJECT_SCHEMA.to_string(),
        principal_id: principal_id.to_string(),
        localhost_root: localhost_root.to_string(),
        data_key_id: protection.data_key_id.clone(),
        object_uri: object_uri.to_string(),
        cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
        nonce: b64_url(&nonce),
        ciphertext: String::new(),
    };
    envelope.ciphertext = encrypt_aes256_gcm_bytes_with_aad(
        data_key,
        &nonce,
        plaintext,
        principal_root_object_aad(&envelope).as_bytes(),
    )?;
    serde_json::to_vec_pretty(&envelope).map_err(Into::into)
}

fn verify_migrated_principal_root_object(
    encrypted: &[u8],
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    protection: &PrincipalRootProtectionV1,
    data_key: &[u8; 32],
    plaintext_sha256: &str,
) -> anyhow::Result<()> {
    let envelope: PrincipalRootObjectEnvelopeV1 = serde_json::from_slice(encrypted)?;
    validate_principal_root_object_envelope(
        &envelope,
        principal_id,
        localhost_root,
        &protection.data_key_id,
        object_uri,
    )?;
    let nonce = b64_url_decode(&envelope.nonce)?;
    if nonce.len() != 12 {
        anyhow::bail!("principal-root object nonce must be 12 bytes");
    }
    let ciphertext = b64_url_decode(&envelope.ciphertext)?;
    let plaintext = decrypt_aes256_gcm_bytes_with_aad(
        data_key,
        &nonce,
        &ciphertext,
        principal_root_object_aad(&envelope).as_bytes(),
    )?;
    if sha256_label(&plaintext) != plaintext_sha256 {
        anyhow::bail!("principal-root migration round-trip verification failed");
    }
    Ok(())
}

fn verify_plaintext_migration_target(
    data_dir: &Path,
    object: &PreparedPrincipalRootMigrationObject,
) -> anyhow::Result<()> {
    ensure_no_symlink_components(data_dir, &object.path)?;
    let metadata = std::fs::symlink_metadata(&object.path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("selected migration object changed type before commit");
    }
    if sha256_label(&std::fs::read(&object.path)?) != object.journal.plaintext_sha256 {
        anyhow::bail!("selected migration object changed before commit");
    }
    Ok(())
}

fn principal_root_migration_journal_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let auth_state = auth_state_path(data_dir)?;
    Ok(auth_state.with_file_name(PRINCIPAL_ROOT_MIGRATION_JOURNAL_FILE))
}

fn write_principal_root_migration_journal(
    data_dir: &Path,
    journal: &PrincipalRootMigrationJournalV1,
) -> anyhow::Result<()> {
    validate_principal_root_migration_journal(journal)?;
    let bytes = serde_json::to_vec_pretty(journal)?;
    if bytes.len() > 256 * 1024 {
        anyhow::bail!("principal-root migration journal exceeds 256 KiB");
    }
    durable_atomic_replace(
        &principal_root_migration_journal_path(data_dir)?,
        &bytes,
        0o600,
    )
}

fn validate_principal_root_migration_journal(
    journal: &PrincipalRootMigrationJournalV1,
) -> anyhow::Result<()> {
    if journal.schema != PRINCIPAL_ROOT_MIGRATION_JOURNAL_SCHEMA {
        anyhow::bail!("unsupported principal-root migration journal schema");
    }
    if !is_sha256_label(&journal.plan_sha256) {
        anyhow::bail!("principal-root migration journal plan hash is invalid");
    }
    validate_principal_root_object_binding(
        &journal.principal_id,
        &journal.localhost_root,
        &journal.localhost_root,
    )?;
    if journal.objects.is_empty() || journal.objects.len() > MAX_MIGRATION_OBJECTS {
        anyhow::bail!("principal-root migration journal object count is invalid");
    }
    let mut uris = std::collections::BTreeSet::new();
    for object in &journal.objects {
        validate_principal_root_object_binding(
            &journal.principal_id,
            &journal.localhost_root,
            &object.object_uri,
        )?;
        if !uris.insert(object.object_uri.as_str())
            || !is_sha256_label(&object.plaintext_sha256)
            || !is_sha256_label(&object.backup_sha256)
            || !is_sha256_label(&object.encrypted_sha256)
            || object.backup_path.parent() != Some(journal.backup_dir.as_path())
            || object.stage_path.parent() != Some(journal.stage_dir.as_path())
        {
            anyhow::bail!("principal-root migration journal object is invalid");
        }
    }
    Ok(())
}

fn recover_principal_root_migration_journal(data_dir: &Path) -> anyhow::Result<bool> {
    let path = principal_root_migration_journal_path(data_dir)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).context("failed to inspect principal-root migration journal"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("principal-root migration journal must be a regular non-symlink file");
    }
    if metadata.len() > 256 * 1024 {
        anyhow::bail!("principal-root migration journal exceeds 256 KiB");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("principal-root migration journal must be owner-only");
        }
    }
    let journal: PrincipalRootMigrationJournalV1 =
        serde_json::from_slice(&std::fs::read(&path)?)
            .context("failed to decode principal-root migration journal")?;
    validate_principal_root_migration_journal(&journal)?;
    let protection =
        load_principal_root_protection(data_dir, &journal.principal_id, &journal.localhost_root)?
            .ok_or_else(|| anyhow!("principal-root migration journal has no existing protection"))?;
    if protection.data_key_id != journal.data_key_id {
        anyhow::bail!("principal-root migration journal protection binding mismatch");
    }
    let data_key = principal_root_data_key_from_protection(data_dir, &protection)?;

    match journal.phase {
        PrincipalRootMigrationJournalPhaseV1::Prepared => {
            for object in &journal.objects {
                verify_journal_plaintext_object(data_dir, &journal, object)?;
            }
        }
        PrincipalRootMigrationJournalPhaseV1::Committing => {
            for object in &journal.objects {
                restore_journal_plaintext_object(data_dir, &journal, object)?;
            }
        }
        PrincipalRootMigrationJournalPhaseV1::Committed => {
            let retain_encrypted = journal.objects.iter().all(|object| {
                verify_journal_encrypted_object(data_dir, &journal, object, &protection, &data_key)
                    .is_ok()
            });
            if retain_encrypted {
                let receipt = principal_root_migration_receipt_from_journal(&journal);
                write_principal_root_migration_receipt(&journal, &receipt)?;
            } else {
                for object in &journal.objects {
                    restore_journal_plaintext_object(data_dir, &journal, object)?;
                }
                remove_principal_root_migration_receipt(&journal)?;
            }
        }
    }
    cleanup_principal_root_migration_journal(data_dir, &journal)?;
    Ok(true)
}

fn principal_root_migration_receipt_from_journal(
    journal: &PrincipalRootMigrationJournalV1,
) -> PrincipalRootMigrationReceiptV1 {
    let objects = journal
        .objects
        .iter()
        .map(|object| PrincipalRootMigrationObjectReceiptV1 {
            object_uri_sha256: sha256_label(object.object_uri.as_bytes()),
            plaintext_sha256: object.plaintext_sha256.clone(),
            backup_sha256: object.backup_sha256.clone(),
            encrypted_sha256: object.encrypted_sha256.clone(),
            round_trip_verified: true,
        })
        .collect::<Vec<_>>();
    PrincipalRootMigrationReceiptV1 {
        schema: PRINCIPAL_ROOT_MIGRATION_RECEIPT_SCHEMA.to_string(),
        status: "migrated".to_string(),
        plan_sha256: journal.plan_sha256.clone(),
        principal_binding_sha256: sha256_label(
            format!("{}\n{}", journal.principal_id, journal.localhost_root).as_bytes(),
        ),
        object_count: objects.len(),
        objects,
    }
}

fn write_principal_root_migration_receipt(
    journal: &PrincipalRootMigrationJournalV1,
    receipt: &PrincipalRootMigrationReceiptV1,
) -> anyhow::Result<()> {
    let path = journal
        .backup_dir
        .join(PRINCIPAL_ROOT_MIGRATION_RECEIPT_FILE);
    let bytes = serde_json::to_vec_pretty(receipt)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                anyhow::bail!(
                    "principal-root migration receipt must be a regular non-symlink file"
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    anyhow::bail!("principal-root migration receipt must be owner-only");
                }
            }
            if std::fs::read(&path)? != bytes {
                anyhow::bail!("principal-root migration receipt changed unexpectedly");
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            write_owner_only_new_file(&path, &bytes)?;
            sync_parent_directory(&path)?;
        }
        Err(error) => {
            return Err(error).context("failed to inspect principal-root migration receipt")
        }
    }
    Ok(())
}

fn remove_principal_root_migration_receipt(
    journal: &PrincipalRootMigrationJournalV1,
) -> anyhow::Result<()> {
    let path = journal
        .backup_dir
        .join(PRINCIPAL_ROOT_MIGRATION_RECEIPT_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                anyhow::bail!(
                    "principal-root migration receipt must be a regular non-symlink file"
                );
            }
            std::fs::remove_file(&path)?;
            sync_parent_directory(&path)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context("failed to inspect principal-root migration receipt")
        }
    }
    Ok(())
}

fn verify_journal_plaintext_object(
    data_dir: &Path,
    journal: &PrincipalRootMigrationJournalV1,
    object: &PrincipalRootMigrationJournalObjectV1,
) -> anyhow::Result<()> {
    let path = rooted_localhost_fs_path(data_dir, &object.object_uri)
        .ok_or_else(|| anyhow!("principal-root migration journal object path is invalid"))?;
    ensure_no_symlink_components(data_dir, &path)?;
    validate_principal_root_object_binding(
        &journal.principal_id,
        &journal.localhost_root,
        &object.object_uri,
    )?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("selected migration object changed type before recovery");
    }
    if sha256_label(&std::fs::read(path)?) != object.plaintext_sha256 {
        anyhow::bail!("selected migration object changed before recovery");
    }
    Ok(())
}

fn verify_journal_encrypted_object(
    data_dir: &Path,
    journal: &PrincipalRootMigrationJournalV1,
    object: &PrincipalRootMigrationJournalObjectV1,
    protection: &PrincipalRootProtectionV1,
    data_key: &[u8; 32],
) -> anyhow::Result<()> {
    let path = rooted_localhost_fs_path(data_dir, &object.object_uri)
        .ok_or_else(|| anyhow!("principal-root migration journal object path is invalid"))?;
    ensure_no_symlink_components(data_dir, &path)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("migrated principal-root object must be a regular non-symlink file");
    }
    let encrypted = std::fs::read(&path)?;
    if sha256_label(&encrypted) != object.encrypted_sha256 {
        anyhow::bail!("migrated principal-root object ciphertext hash mismatch");
    }
    verify_migrated_principal_root_object(
        &encrypted,
        &journal.principal_id,
        &journal.localhost_root,
        &object.object_uri,
        protection,
        data_key,
        &object.plaintext_sha256,
    )
}

fn restore_journal_plaintext_object(
    data_dir: &Path,
    journal: &PrincipalRootMigrationJournalV1,
    object: &PrincipalRootMigrationJournalObjectV1,
) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(&object.backup_path)
        .context("failed to inspect principal-root migration backup")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("principal-root migration backup must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("principal-root migration backup must be owner-only");
        }
    }
    let plaintext = std::fs::read(&object.backup_path)?;
    if sha256_label(&plaintext) != object.backup_sha256
        || object.backup_sha256 != object.plaintext_sha256
    {
        anyhow::bail!("principal-root migration backup hash mismatch");
    }
    let path = rooted_localhost_fs_path(data_dir, &object.object_uri)
        .ok_or_else(|| anyhow!("principal-root migration journal object path is invalid"))?;
    ensure_no_symlink_components(data_dir, &path)?;
    validate_principal_root_object_binding(
        &journal.principal_id,
        &journal.localhost_root,
        &object.object_uri,
    )?;
    durable_atomic_replace(&path, &plaintext, object.original_mode)?;
    if sha256_label(&std::fs::read(path)?) != object.plaintext_sha256 {
        anyhow::bail!("principal-root migration rollback verification failed");
    }
    Ok(())
}

fn cleanup_principal_root_migration_journal(
    data_dir: &Path,
    journal: &PrincipalRootMigrationJournalV1,
) -> anyhow::Result<()> {
    for object in &journal.objects {
        match std::fs::remove_file(&object.stage_path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).context("failed to remove migration stage object"),
        }
    }
    match std::fs::remove_dir(&journal.stage_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("failed to remove migration stage directory"),
    }
    let journal_path = principal_root_migration_journal_path(data_dir)?;
    std::fs::remove_file(&journal_path)?;
    sync_parent_directory(&journal_path)
}

fn create_owner_only_dir(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(path)?;
    Ok(())
}

/// Create a principal-root directory and every missing parent owner-only.
///
/// Protected writes refuse a parent anyone else can read, and rightly so — a
/// loose directory can mean the objects under it were already exposed. That
/// makes it the writer's job to never create one: `create_dir_all` follows
/// the process umask, so a Home whose shell saved its state before any
/// protection existed left a world-readable root behind and locked the person
/// out of ever saving a Profile.
pub(crate) fn create_owner_only_dir_all(data_dir: &Path, path: &Path) -> anyhow::Result<()> {
    // Only ever create or narrow inside this Home's own data directory. The
    // path above it belongs to the person's machine, not to us.
    if !path.starts_with(data_dir) || path == data_dir {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create directory {path:?}"))?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_owner_only_dir_all(data_dir, parent)?;
    }
    if !path.is_dir() {
        if let Err(err) = create_owner_only_dir(path) {
            if !path.is_dir() {
                return Err(err)
                    .with_context(|| format!("failed to create owner-only directory {path:?}"));
            }
        }
    }
    // A directory that already exists may predate this rule — earlier writers
    // used the process umask — so bring our own tree up to it rather than
    // leave a person permanently unable to save. This does not soften the
    // check that refuses a loose parent at write time; it stops the Runtime
    // from creating the loose parent at all, and only ever narrows access on
    // a directory this process owns.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect owner-only directory {path:?}"))?;
        if !metadata.file_type().is_symlink()
            && metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.permissions().mode() & 0o077 != 0
        {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to restrict {path:?} to owner-only"))?;
        }
    }
    Ok(())
}

fn durable_atomic_replace(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("durable replacement path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(if mode == 0 {
                0o600
            } else {
                mode
            }))?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context("failed to durably replace file")?;
    sync_parent_directory(path)
}

fn principal_root_file_mode(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn ensure_no_symlink_components(data_dir: &Path, path: &Path) -> anyhow::Result<()> {
    let relative = path
        .strip_prefix(data_dir)
        .map_err(|_| anyhow!("principal-root object path escapes the Runtime data root"))?;
    let mut current = data_dir.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("principal-root object path rejects symlink components")
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).context("failed to inspect principal-root object path")
            }
        }
    }
    Ok(())
}

fn ensure_protected_principal_root_object_parent(
    data_dir: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("protected principal-root object path has no parent"))?;
    let relative = parent.strip_prefix(data_dir).map_err(|_| {
        anyhow!("protected principal-root object path escapes the Runtime data root")
    })?;
    ensure_auth_owner_only_directory(data_dir)?;
    let mut current = data_dir.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        ensure_auth_owner_only_directory(&current)?;
    }
    Ok(())
}

/// Ensures an auth-owned parent directory is an owner-only real directory.
///
/// Protected principal-root objects contain secrets, so this intentionally
/// differs from the generic unprotected-object writer: it creates each missing
/// component with 0700 and rejects an existing symlink, non-directory, foreign
/// owner, or group/world-accessible directory before any object write.
fn ensure_auth_owner_only_directory(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to create protected parent {path:?}"))
                }
            }
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect protected parent {path:?}"))
        }
    }

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect protected parent {path:?}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("protected principal-root parent must be a real directory")
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let directory = options
            .open(path)
            .with_context(|| format!("failed to open protected parent {path:?}"))?;
        let metadata = directory
            .metadata()
            .with_context(|| format!("failed to inspect opened protected parent {path:?}"))?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!("protected principal-root parent must be owner-only: {path:?}")
        }
    }
    Ok(())
}

fn write_owner_only_new_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn is_sha256_label(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("atomic write path has no file name"))?;
    let temp = path.with_file_name(format!(
        ".{file_name}.{:016x}.tmp",
        rand::thread_rng().next_u64()
    ));
    let write_result = (|| -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(err.into());
    }
    sync_parent_directory(path)?;
    Ok(())
}

fn prune_auth_state(state: &mut AuthState, now: u64) {
    state
        .challenges
        .retain(|stored| stored.challenge.expires_at > now && stored.consumed_at.is_none());
    state.sessions.retain(|stored| {
        stored.revoked_at.is_none() && stored.grant.expires_at > now.saturating_sub(86_400)
    });
}

#[cfg(test)]
pub(crate) fn store_test_principal_root_protection(
    data_dir: &Path,
    principal_id: &str,
) -> PrincipalRootProtectionV1 {
    let localhost_root = principal_localhost_root(principal_id);
    let created_at = now_ts();
    let mut data_key = [0u8; 32];
    let mut salt = [0u8; 32];
    let mut wrap_nonce = [0u8; 12];
    let mut descriptor_nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut data_key);
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut wrap_nonce);
    rand::rngs::OsRng.fill_bytes(&mut descriptor_nonce);
    let recovery_phrase = "aaaa-bbbb-cccc-dddd-eeee-ffff-1111-2222".to_string();
    let crypto = elastos_runtime::auth::PrincipalRootCryptoProfileV1 {
        recovery_kdf: "hkdf-sha256".to_string(),
        ..elastos_runtime::auth::PrincipalRootCryptoProfileV1::default()
    };
    let wrapping_key =
        derive_recovery_wrapping_key(&recovery_phrase, &salt, principal_id, &localhost_root)
            .unwrap();
    let wrapped_data_key = encrypt_aes256_gcm_bytes(&wrapping_key, &wrap_nonce, &data_key).unwrap();
    let data_key_id = principal_data_key_id(&data_key);
    let descriptor = serde_json::json!({
        "schema": RECOVERY_DESCRIPTOR_SCHEMA,
        "principal_id": principal_id,
        "localhost_root": localhost_root,
        "data_key_id": data_key_id,
        "created_at": created_at,
    });
    let descriptor_ciphertext = encrypt_aes256_gcm_bytes(
        &data_key,
        &descriptor_nonce,
        &serde_json::to_vec(&descriptor).unwrap(),
    )
    .unwrap();
    let encrypted_root_descriptor = format!(
        "aes-256-gcm:v1:{}:{}",
        b64_url(&descriptor_nonce),
        descriptor_ciphertext
    );
    let kit = RecoveryKitV1 {
        schema: elastos_runtime::auth::RECOVERY_KIT_SCHEMA.to_string(),
        kit_id: "kit:test-principal-root".to_string(),
        protector_id: "protector:recovery:test-principal-root".to_string(),
        principal_id: principal_id.to_string(),
        localhost_root: localhost_root.clone(),
        data_key_id: data_key_id.clone(),
        recovery_phrase,
        salt: b64_url(&salt),
        nonce: b64_url(&wrap_nonce),
        wrapped_data_key,
        encrypted_root_descriptor,
        crypto: crypto.clone(),
        created_at,
        instructions: vec!["Test recovery kit.".to_string()],
    };
    verify_recovery_kit_material(&kit).unwrap();
    let archive = recovery_archive_from_kit(data_dir, &kit).unwrap();
    let protection = PrincipalRootProtectionV1 {
        schema: elastos_runtime::auth::PRINCIPAL_ROOT_PROTECTION_SCHEMA.to_string(),
        principal_id: principal_id.to_string(),
        localhost_root,
        data_key_id,
        crypto,
        protectors: vec![elastos_runtime::auth::PrincipalRootProtectorV1 {
            protector_id: kit.protector_id,
            kind: PrincipalRootProtectorKind::RecoveryKit,
            label: "Test Recovery Kit".to_string(),
            subject: None,
            created_at,
            verified_at: Some(created_at),
            envelope: Some(elastos_runtime::auth::PrincipalRootProtectorEnvelopeV1 {
                cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
                kdf: "hkdf-sha256".to_string(),
                salt: kit.salt,
                nonce: kit.nonce,
                wrapped_data_key: kit.wrapped_data_key,
            }),
            archive: Some(archive),
        }],
        created_at,
        updated_at: created_at,
    };
    store_principal_root_protection(data_dir, protection.clone()).unwrap();
    protection
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::auth::PasskeyWebAuthnBinding;

    fn test_audit_event(index: u64) -> RuntimeAuditEventV1 {
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!("audit:test:{index}"),
            event_type: "test.audit".to_string(),
            principal_id: Some("person:local:test".to_string()),
            proof_binding_id: None,
            session_id: Some("session:test".to_string()),
            challenge_id: None,
            capsule_id: Some("system".to_string()),
            result: "allowed".to_string(),
            reason: format!("test event {index}"),
            occurred_at: index,
            signer_did: None,
            signature: None,
        }
    }

    fn passkey_binding(sign_count: u32, created_at: u64, last_used_at: u64) -> ProofBinding {
        passkey_binding_with_credential("credential-1", sign_count, created_at, last_used_at)
    }

    fn passkey_binding_with_credential(
        credential_id: &str,
        sign_count: u32,
        created_at: u64,
        last_used_at: u64,
    ) -> ProofBinding {
        ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
            credential_id: credential_id.to_string(),
            public_key: "public-key".to_string(),
            sign_count,
            user_verified: true,
            origin: "https://elastos.elacitylabs.com".to_string(),
            rp_id: "elastos.elacitylabs.com".to_string(),
            created_at,
            last_used_at,
            revoked_at: None,
        })
    }

    fn recovered_root_reassignment_fixture(
        data_dir: &Path,
    ) -> (
        RecoveredRootReassignment,
        PrincipalRecord,
        AuthSessionGrantV1,
    ) {
        let updated_at = now_ts();
        let current = upsert_principal_for_binding_as_role(
            data_dir,
            passkey_binding(1, updated_at, updated_at),
            "person:local:current-passkey".to_string(),
            RuntimePrincipalRole::Guest,
            updated_at,
        )
        .unwrap();
        let current_grant = AuthSessionGrantV1 {
            schema: AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: "grant:current-passkey".to_string(),
            session_id: "auth:current-passkey".to_string(),
            principal_id: current.principal_id.clone(),
            proof_binding_id: current.proof_binding_id.clone(),
            issued_at: updated_at,
            expires_at: updated_at + 3_600,
            apps: vec!["home".to_string(), "system".to_string()],
        };
        store_session_grant(data_dir, current_grant.clone()).unwrap();

        let recovered_principal_id = "person:local:recovered-signature-test".to_string();
        let protection = store_test_principal_root_protection(data_dir, &recovered_principal_id);
        let replacement_grant = AuthSessionGrantV1 {
            schema: AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: "grant:recovered-signature-test".to_string(),
            session_id: "auth:recovered-signature-test".to_string(),
            principal_id: recovered_principal_id.clone(),
            proof_binding_id: current.proof_binding_id.clone(),
            issued_at: updated_at,
            expires_at: updated_at + 3_600,
            apps: vec!["home".to_string(), "system".to_string()],
        };
        let signed_audit_event = sign_audit_event(
            data_dir,
            RuntimeAuditEventV1 {
                schema: RuntimeAuditEventV1::SCHEMA.to_string(),
                event_id: "audit:recovered-signature-test".to_string(),
                event_type: "auth.recovery_kit.reassigned".to_string(),
                principal_id: Some(recovered_principal_id.clone()),
                proof_binding_id: Some(current.proof_binding_id.clone()),
                session_id: Some(replacement_grant.session_id.clone()),
                challenge_id: None,
                capsule_id: None,
                result: "ok".to_string(),
                reason: "principal root reassigned from verified Recovery Kit and session reissued"
                    .to_string(),
                occurred_at: updated_at,
                signer_did: None,
                signature: None,
            },
        )
        .unwrap();
        (
            RecoveredRootReassignment {
                proof_binding_id: current.proof_binding_id.clone(),
                recovered_principal_id,
                recovered_localhost_root: protection.localhost_root.clone(),
                protection,
                replacement_grant,
                signed_audit_event,
                updated_at,
            },
            current,
            current_grant,
        )
    }

    #[test]
    fn recovered_root_reassignment_rejects_invalid_audit_signatures_before_mutation() {
        for case in ["malformed", "substituted", "wrong-runtime-signer"] {
            let data_dir = tempfile::tempdir().unwrap();
            let (mut reassignment, current, current_grant) =
                recovered_root_reassignment_fixture(data_dir.path());
            match case {
                "malformed" => {
                    reassignment.signed_audit_event.signature =
                        Some("not-a-valid-signature".to_string());
                }
                "substituted" => {
                    reassignment.signed_audit_event.reason =
                        "substituted after Runtime signing".to_string();
                }
                "wrong-runtime-signer" => {
                    let foreign_dir = tempfile::tempdir().unwrap();
                    reassignment.signed_audit_event =
                        sign_audit_event(foreign_dir.path(), reassignment.signed_audit_event)
                            .unwrap();
                }
                _ => unreachable!(),
            }
            let replacement_session_id = reassignment.replacement_grant.session_id.clone();
            let before = serde_json::to_value(load_auth_state(data_dir.path()).unwrap()).unwrap();

            let error = commit_recovered_root_reassignment(data_dir.path(), reassignment)
                .unwrap_err()
                .to_string();

            assert!(
                error.contains("signature") || error.contains("signer"),
                "{case} failed for an unexpected reason: {error}"
            );
            let after = serde_json::to_value(load_auth_state(data_dir.path()).unwrap()).unwrap();
            assert_eq!(after, before, "{case} mutated auth state");
            let retained =
                load_principal_for_proof_binding(data_dir.path(), &current.proof_binding_id)
                    .unwrap();
            assert_eq!(retained.principal_id, current.principal_id);
            assert!(
                is_auth_session_active(data_dir.path(), &current_grant.session_id, now_ts())
                    .unwrap()
            );
            assert!(
                !is_auth_session_active(data_dir.path(), &replacement_session_id, now_ts())
                    .unwrap()
            );
        }
    }

    #[test]
    fn passkey_principal_upsert_preserves_creation_time() {
        let data_dir = tempfile::tempdir().unwrap();

        let first =
            upsert_principal_for_binding(data_dir.path(), passkey_binding(1, 10, 10), 10).unwrap();
        let second =
            upsert_principal_for_binding(data_dir.path(), passkey_binding(2, 20, 20), 20).unwrap();

        assert_eq!(first.principal_id, second.principal_id);
        let passkey = second.proof_binding.passkey.unwrap();
        assert_eq!(passkey.created_at, 10);
        assert_eq!(passkey.last_used_at, 20);
        assert_eq!(passkey.sign_count, 2);
        assert_eq!(second.role, RuntimePrincipalRole::Guest);
        assert!(second.localhost_root.starts_with("localhost://Users/"));
    }

    #[test]
    fn passkey_principal_role_and_root_are_explicit() {
        let data_dir = tempfile::tempdir().unwrap();
        let now = 10;
        let principal_id =
            passkey_credential_principal_id("elastos.elacitylabs.com", "credential-1").unwrap();

        let principal = upsert_principal_for_binding_as_role(
            data_dir.path(),
            passkey_binding(1, now, now),
            principal_id.clone(),
            RuntimePrincipalRole::Admin,
            now,
        )
        .unwrap();

        assert_eq!(principal.role, RuntimePrincipalRole::Admin);
        assert_eq!(
            principal.localhost_root,
            principal_localhost_root(&principal_id)
        );
        assert_eq!(active_passkey_principal_count(data_dir.path()).unwrap(), 1);
    }

    #[test]
    fn passkey_promotion_changes_active_guest_to_admin() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal = upsert_principal_for_binding_as_role(
            data_dir.path(),
            passkey_binding(1, 10, 10),
            "person:local:guest".to_string(),
            RuntimePrincipalRole::Guest,
            10,
        )
        .unwrap();

        let promoted =
            promote_passkey_to_admin(data_dir.path(), &principal.proof_binding_id, 20).unwrap();

        assert_eq!(promoted.role, RuntimePrincipalRole::Admin);
        assert_eq!(promoted.updated_at, 20);
        assert_eq!(
            active_admin_passkey_principal_count(data_dir.path()).unwrap(),
            1
        );
    }

    #[test]
    fn passkey_demotion_changes_active_admin_to_guest_but_keeps_one_admin() {
        let data_dir = tempfile::tempdir().unwrap();
        let primary = upsert_principal_for_binding_as_role(
            data_dir.path(),
            passkey_binding_with_credential("credential-1", 1, 10, 10),
            "person:local:admin-1".to_string(),
            RuntimePrincipalRole::Admin,
            10,
        )
        .unwrap();
        let secondary = upsert_principal_for_binding_as_role(
            data_dir.path(),
            passkey_binding_with_credential("credential-2", 1, 12, 12),
            "person:local:admin-2".to_string(),
            RuntimePrincipalRole::Admin,
            12,
        )
        .unwrap();

        let demoted =
            demote_passkey_to_guest(data_dir.path(), &secondary.proof_binding_id, 20).unwrap();

        assert_eq!(demoted.role, RuntimePrincipalRole::Guest);
        assert_eq!(demoted.updated_at, 20);
        assert_eq!(
            active_admin_passkey_principal_count(data_dir.path()).unwrap(),
            1
        );
        let primary =
            load_principal_for_proof_binding(data_dir.path(), &primary.proof_binding_id).unwrap();
        assert_eq!(primary.role, RuntimePrincipalRole::Admin);
    }

    #[test]
    fn passkey_demotion_rejects_last_admin() {
        let data_dir = tempfile::tempdir().unwrap();
        let admin = upsert_principal_for_binding_as_role(
            data_dir.path(),
            passkey_binding(1, 10, 10),
            "person:local:admin".to_string(),
            RuntimePrincipalRole::Admin,
            10,
        )
        .unwrap();

        let err = demote_passkey_to_guest(data_dir.path(), &admin.proof_binding_id, 20)
            .unwrap_err()
            .to_string();

        assert!(err.contains("last admin passkey cannot be demoted"));
    }

    #[test]
    fn guest_registration_defaults_off_and_can_be_toggled() {
        let data_dir = tempfile::tempdir().unwrap();

        assert!(!guest_registration_enabled(data_dir.path()).unwrap());
        assert!(set_guest_registration_enabled(data_dir.path(), true, 20).unwrap());
        assert!(guest_registration_enabled(data_dir.path()).unwrap());
        assert!(!set_guest_registration_enabled(data_dir.path(), false, 30).unwrap());
        assert!(!guest_registration_enabled(data_dir.path()).unwrap());

        let state = load_auth_state(data_dir.path()).unwrap();
        assert_eq!(state.audit.len(), 2);
        assert_eq!(state.audit_chain.len(), 2);
        assert_eq!(state.audit_chain[0].sequence, 1);
        assert_eq!(
            state.audit_chain[1].previous_hash,
            state.audit_chain[0].chain_hash
        );
        assert!(state.audit.iter().all(|event| event
            .signer_did
            .as_deref()
            .is_some_and(|did| did.starts_with("did:key:"))));
        assert!(state.audit.iter().all(|event| event
            .signature
            .as_deref()
            .is_some_and(|signature| signature.len() == 128)));
    }

    #[test]
    fn persisted_audit_events_are_always_runtime_signed() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(
            data_dir.path(),
            RuntimeAuditEventV1 {
                schema: RuntimeAuditEventV1::SCHEMA.to_string(),
                event_id: "audit:foreign-claim".to_string(),
                event_type: "test.foreign-claim".to_string(),
                principal_id: None,
                proof_binding_id: None,
                session_id: None,
                challenge_id: None,
                capsule_id: Some("component-test".to_string()),
                result: "claimed".to_string(),
                reason: "guest-supplied claim".to_string(),
                occurred_at: 1,
                signer_did: Some("did:key:guest".to_string()),
                signature: Some("guest-signature".to_string()),
            },
        )
        .unwrap();

        let state = load_auth_state(data_dir.path()).unwrap();
        let event = state.audit.last().unwrap();
        assert_ne!(event.signer_did.as_deref(), Some("did:key:guest"));
        assert_ne!(event.signature.as_deref(), Some("guest-signature"));
        assert!(event
            .signer_did
            .as_deref()
            .is_some_and(|did| did.starts_with("did:key:")));
    }

    #[test]
    fn concurrent_audit_appends_do_not_race_on_temp_file() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(
            data_dir.path(),
            RuntimeAuditEventV1 {
                schema: RuntimeAuditEventV1::SCHEMA.to_string(),
                event_id: "audit:seed".to_string(),
                event_type: "test.seed".to_string(),
                principal_id: Some("person:local:test".to_string()),
                proof_binding_id: None,
                session_id: Some("session:test".to_string()),
                challenge_id: None,
                capsule_id: Some("browser".to_string()),
                result: "allowed".to_string(),
                reason: "seed signing key".to_string(),
                occurred_at: 1,
                signer_did: None,
                signature: None,
            },
        )
        .unwrap();

        let mut handles = Vec::new();
        for index in 0..24 {
            let data_dir = data_dir.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                append_audit_event(
                    &data_dir,
                    RuntimeAuditEventV1 {
                        schema: RuntimeAuditEventV1::SCHEMA.to_string(),
                        event_id: format!("audit:browser-chain-read:{index}"),
                        event_type: "browser.chain_read.completed".to_string(),
                        principal_id: Some("person:local:test".to_string()),
                        proof_binding_id: None,
                        session_id: Some("session:test".to_string()),
                        challenge_id: Some(format!("read:{index}")),
                        capsule_id: Some("browser".to_string()),
                        result: "allowed".to_string(),
                        reason: "method=eth_call decision=provider_mediated_typed_read".to_string(),
                        occurred_at: 2 + index,
                        signer_did: None,
                        signature: None,
                    },
                )
            }));
        }

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let state = load_auth_state(data_dir.path()).unwrap();
        let read_events = state
            .audit
            .iter()
            .filter(|event| event.event_type == "browser.chain_read.completed")
            .count();
        assert_eq!(read_events, 24);
        assert_eq!(state.audit_chain.len(), state.audit.len());
    }

    #[test]
    fn audit_chain_rejects_tampered_event_content() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(
            data_dir.path(),
            RuntimeAuditEventV1 {
                schema: RuntimeAuditEventV1::SCHEMA.to_string(),
                event_id: "audit:tamper-test".to_string(),
                event_type: "test.audit".to_string(),
                principal_id: Some("person:local:test".to_string()),
                proof_binding_id: None,
                session_id: Some("session:test".to_string()),
                challenge_id: None,
                capsule_id: Some("system".to_string()),
                result: "allowed".to_string(),
                reason: "original".to_string(),
                occurred_at: 1,
                signer_did: None,
                signature: None,
            },
        )
        .unwrap();
        let path = auth_state_path(data_dir.path()).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        state["audit"][0]["reason"] = serde_json::json!("tampered");
        std::fs::write(path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

        let err = load_auth_state(data_dir.path()).unwrap_err();
        assert!(err.to_string().contains("audit event signature is invalid"));
    }

    #[test]
    fn audit_chain_activation_recovers_and_rejects_signed_state_rollback() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(data_dir.path(), test_audit_event(1)).unwrap();

        let marker_path = audit_chain_activation_path(data_dir.path()).unwrap();
        let marker = std::fs::read(&marker_path).unwrap();

        std::fs::remove_file(&marker_path).unwrap();
        verify_auth_audit_chain_ready(data_dir.path()).unwrap();
        assert_eq!(std::fs::read(&marker_path).unwrap(), marker);

        let auth_path = auth_state_path(data_dir.path()).unwrap();
        let first_state = std::fs::read(&auth_path).unwrap();
        append_audit_event(data_dir.path(), test_audit_event(2)).unwrap();
        std::fs::write(&auth_path, first_state).unwrap();

        let err = load_auth_state(data_dir.path()).unwrap_err().to_string();
        assert!(err.contains("audit chain rollback detected"));
    }

    #[test]
    fn audit_chain_activation_advances_after_an_interrupted_checkpoint_publish() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(data_dir.path(), test_audit_event(1)).unwrap();

        let marker_path = audit_chain_activation_path(data_dir.path()).unwrap();
        let stale_marker = std::fs::read(&marker_path).unwrap();
        append_audit_event(data_dir.path(), test_audit_event(2)).unwrap();
        let advanced_marker = std::fs::read(&marker_path).unwrap();
        assert_ne!(stale_marker, advanced_marker);

        std::fs::write(&marker_path, stale_marker).unwrap();
        verify_auth_audit_chain_ready(data_dir.path()).unwrap();
        assert_eq!(std::fs::read(&marker_path).unwrap(), advanced_marker);
    }

    #[test]
    fn audit_chain_activation_rejects_same_sequence_substitution() {
        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(data_dir.path(), test_audit_event(1)).unwrap();
        let activated_at = load_auth_state(data_dir.path())
            .unwrap()
            .audit_chain_state
            .unwrap()
            .activated_at;

        let mut substituted = AuthState {
            audit_chain_state: Some(
                sign_audit_chain_state(data_dir.path(), activated_at, None).unwrap(),
            ),
            ..AuthState::default()
        };
        let mut event = test_audit_event(1);
        event.reason = "substituted-at-the-same-sequence".to_string();
        let event = sign_audit_event(data_dir.path(), event).unwrap();
        push_audit_event(data_dir.path(), &mut substituted, event).unwrap();
        std::fs::write(
            auth_state_path(data_dir.path()).unwrap(),
            serde_json::to_vec_pretty(&substituted).unwrap(),
        )
        .unwrap();

        let error = load_auth_state(data_dir.path()).unwrap_err().to_string();
        assert!(error.contains("truncation or substitution detected"));
    }

    #[test]
    fn first_authority_write_activates_the_signed_audit_checkpoint() {
        let data_dir = tempfile::tempdir().unwrap();
        upsert_principal_for_binding(data_dir.path(), passkey_binding(1, 10, 10), 10).unwrap();

        let state = load_auth_state(data_dir.path()).unwrap();
        let activation = load_audit_chain_activation(data_dir.path())
            .unwrap()
            .unwrap();
        let chain_state = state.audit_chain_state.as_ref().unwrap();
        assert_eq!(activation.activated_at, chain_state.activated_at);
        assert_eq!(activation.checkpoint.head_sequence, 0);
        assert_eq!(activation.checkpoint.head_hash, AUDIT_CHAIN_GENESIS);
    }

    #[test]
    fn audit_chain_rejects_foreign_signers_for_every_artifact() {
        let data_dir = tempfile::tempdir().unwrap();
        for index in 1..=3 {
            append_audit_event(data_dir.path(), test_audit_event(index)).unwrap();
        }
        let mut state = load_auth_state(data_dir.path()).unwrap();
        retain_audit_tail(data_dir.path(), &mut state, 2).unwrap();
        let activated_at = state.audit_chain_state.as_ref().unwrap().activated_at;
        state.audit_chain_state = Some(
            sign_audit_chain_state(data_dir.path(), activated_at, state.audit_chain.last())
                .unwrap(),
        );
        let foreign_dir = tempfile::tempdir().unwrap();
        let (_, foreign_did) = elastos_identity::load_or_create_did(foreign_dir.path()).unwrap();

        let activation_path = audit_chain_activation_path(data_dir.path()).unwrap();
        let activation_bytes = std::fs::read(&activation_path).unwrap();
        let mut foreign_activation: AuditChainActivationV2 =
            serde_json::from_slice(&activation_bytes).unwrap();
        foreign_activation.signer_did = foreign_did.clone();
        std::fs::write(
            &activation_path,
            serde_json::to_vec_pretty(&foreign_activation).unwrap(),
        )
        .unwrap();
        let err = load_auth_state(data_dir.path()).unwrap_err();
        assert!(err
            .to_string()
            .contains("audit chain activation signer is not the Runtime identity"));
        std::fs::write(&activation_path, activation_bytes).unwrap();

        let mut foreign_event = state.clone();
        foreign_event.audit[0].signer_did = Some(foreign_did.clone());
        let err = verify_audit_chain(data_dir.path(), &foreign_event).unwrap_err();
        assert!(err
            .to_string()
            .contains("audit event signer is not the Runtime identity"));

        let mut foreign_link = state.clone();
        foreign_link.audit_chain[0].signer_did = foreign_did.clone();
        let err = verify_audit_chain(data_dir.path(), &foreign_link).unwrap_err();
        assert!(err
            .to_string()
            .contains("audit chain link signer is not the Runtime identity"));

        let mut foreign_anchor = state.clone();
        foreign_anchor
            .audit_chain_anchor
            .as_mut()
            .unwrap()
            .signer_did = foreign_did.clone();
        let err = verify_audit_chain(data_dir.path(), &foreign_anchor).unwrap_err();
        assert!(err
            .to_string()
            .contains("audit chain anchor signer is not the Runtime identity"));

        let mut foreign_state = state;
        foreign_state.audit_chain_state.as_mut().unwrap().signer_did = foreign_did;
        let err = verify_audit_chain(data_dir.path(), &foreign_state).unwrap_err();
        assert!(err
            .to_string()
            .contains("audit chain state signer is not the Runtime identity"));
    }

    #[test]
    fn retained_audit_chain_requires_its_signed_anchor() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut state = AuthState::default();
        ensure_audit_chain_state(data_dir.path(), &mut state).unwrap();
        save_auth_state(data_dir.path(), &state).unwrap();
        for index in 1..=3 {
            let event = sign_audit_event(data_dir.path(), test_audit_event(index)).unwrap();
            push_audit_event(data_dir.path(), &mut state, event).unwrap();
        }
        retain_audit_tail(data_dir.path(), &mut state, 2).unwrap();
        let activated_at = state.audit_chain_state.as_ref().unwrap().activated_at;
        state.audit_chain_state = Some(
            sign_audit_chain_state(data_dir.path(), activated_at, state.audit_chain.last())
                .unwrap(),
        );
        save_auth_state(data_dir.path(), &state).unwrap();

        let retained = load_auth_state(data_dir.path()).unwrap();
        let anchor = retained.audit_chain_anchor.as_ref().unwrap();
        assert_eq!(anchor.sequence, 1);
        assert_eq!(retained.audit_chain[0].sequence, 2);
        assert_eq!(retained.audit_chain[0].previous_hash, anchor.chain_hash);

        let mut truncated = retained;
        truncated.audit.remove(0);
        truncated.audit_chain.remove(0);
        let activated_at = truncated.audit_chain_state.as_ref().unwrap().activated_at;
        truncated.audit_chain_state = Some(
            sign_audit_chain_state(data_dir.path(), activated_at, truncated.audit_chain.last())
                .unwrap(),
        );
        let err = save_auth_state(data_dir.path(), &truncated)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not continue from its retained anchor"));
    }

    #[test]
    fn legacy_unchained_authority_is_rejected_before_expiry_pruning() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut legacy = AuthState::default();
        legacy.challenges.push(StoredAuthChallenge {
            challenge: AuthChallengeV1 {
                schema: AuthChallengeV1::SCHEMA.to_string(),
                challenge_id: "challenge:legacy-expired".to_string(),
                domain: "localhost".to_string(),
                uri: "http://localhost/apps/home/".to_string(),
                statement: "Sign in to ElastOS Runtime.".to_string(),
                address: "0x1111111111111111111111111111111111111111".to_string(),
                chain_id: 20,
                nonce: "legacy-expired".to_string(),
                issued_at: 0,
                expires_at: 1,
                resources: vec!["elastos://auth/challenge/legacy-expired".to_string()],
            },
            consumed_at: None,
        });
        let path = auth_state_path(data_dir.path()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

        let err = load_auth_state(data_dir.path()).unwrap_err().to_string();
        assert!(err.contains("unchained auth state is unsupported"));
        assert!(err.contains("fresh data root"));
        assert!(err.contains("no automatic migration"));
    }

    #[cfg(unix)]
    #[test]
    fn auth_secret_files_are_private_and_symlink_paths_fail_closed() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let data_dir = tempfile::tempdir().unwrap();
        append_audit_event(data_dir.path(), test_audit_event(1)).unwrap();
        for path in [
            auth_state_path(data_dir.path()).unwrap(),
            audit_chain_activation_path(data_dir.path()).unwrap(),
            auth_state_lock_path(data_dir.path()).unwrap(),
            audit_chain_activation_lock_path(data_dir.path()).unwrap(),
        ] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let marker = audit_chain_activation_path(data_dir.path()).unwrap();
        let target = marker.with_file_name("attacker-activation.json");
        std::fs::write(&target, b"{}").unwrap();
        std::fs::remove_file(&marker).unwrap();
        symlink(&target, &marker).unwrap();
        let error = load_auth_state(data_dir.path()).unwrap_err().to_string();
        assert!(error.contains("regular non-symlink file"));

        let poisoned_data_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let relative_auth_parent = auth_state_path(poisoned_data_dir.path())
            .unwrap()
            .parent()
            .unwrap()
            .strip_prefix(poisoned_data_dir.path())
            .unwrap()
            .to_path_buf();
        let first_component = relative_auth_parent
            .components()
            .next()
            .expect("auth path must be below the data root");
        symlink(
            outside.path(),
            poisoned_data_dir.path().join(first_component.as_os_str()),
        )
        .unwrap();
        let error = append_audit_event(poisoned_data_dir.path(), test_audit_event(1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("regular non-symlink directories"));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_session_grants_do_not_lose_tokens() {
        let data_dir = tempfile::tempdir().unwrap();
        let now = now_ts();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(24));
        let mut handles = Vec::new();

        for index in 0..24 {
            let data_dir = data_dir.path().to_path_buf();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store_session_grant(
                    &data_dir,
                    AuthSessionGrantV1 {
                        schema: AuthSessionGrantV1::SCHEMA.to_string(),
                        grant_id: format!("grant:{index}"),
                        session_id: format!("auth:{index}"),
                        principal_id: "person:local:test".to_string(),
                        proof_binding_id: "proof:passkey:test".to_string(),
                        issued_at: now,
                        expires_at: now + 1_000,
                        apps: vec!["home".to_string(), "system".to_string()],
                    },
                )
            }));
        }

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let state = load_auth_state(data_dir.path()).unwrap();
        for index in 0..24 {
            let session_id = format!("auth:{index}");
            assert!(
                state
                    .sessions
                    .iter()
                    .any(|stored| stored.grant.session_id == session_id
                        && stored.revoked_at.is_none()),
                "missing active session {session_id}",
            );
        }
        assert_eq!(state.sessions.len(), 24);
    }

    #[test]
    fn atomic_write_supports_concurrent_writers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(24));
        let mut handles = Vec::new();

        for index in 0..24 {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                atomic_write(&path, format!("writer-{index}").as_bytes())
            }));
        }

        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("writer-"));
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn atomic_write_cleans_up_a_staged_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::create_dir(&path).unwrap();

        assert!(atomic_write(&path, b"replacement").is_err());
        assert!(path.is_dir());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn principal_root_object_stays_plaintext_without_protection() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:plain-root";
        let localhost_root = principal_localhost_root(principal_id);
        let object_uri = format!("{localhost_root}/Documents/plain.md");
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::set_permissions(
                path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        write_principal_root_object(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &object_uri,
            &path,
            b"plain body",
        )
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"plain body");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        assert_eq!(
            read_principal_root_object(
                data_dir.path(),
                principal_id,
                &localhost_root,
                &object_uri,
                &path,
            )
            .unwrap(),
            b"plain body"
        );
    }

    #[test]
    fn principal_root_object_encrypts_when_root_is_protected() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:protected-root";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/secret.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();

        write_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
            b"# Secret\n",
        )
        .unwrap();

        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("# Secret"));
        let envelope: PrincipalRootObjectEnvelopeV1 = serde_json::from_str(&stored).unwrap();
        assert_eq!(envelope.schema, PRINCIPAL_ROOT_OBJECT_SCHEMA);
        assert_eq!(envelope.principal_id, principal_id);
        assert_eq!(envelope.object_uri, object_uri);
        assert_eq!(
            read_principal_root_object(
                data_dir.path(),
                principal_id,
                &protection.localhost_root,
                &object_uri,
                &path,
            )
            .unwrap(),
            b"# Secret\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn protected_principal_root_writes_create_owner_only_parents() {
        use std::os::unix::fs::PermissionsExt;

        let data_dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(data_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let principal_id = "person:local:protected-owner-only";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!(
            "{}/.AppData/ElastOS/People/contact-state.json",
            protection.localhost_root
        );
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();

        write_protected_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
            b"protected",
        )
        .unwrap();

        let relative_parent = path
            .parent()
            .unwrap()
            .strip_prefix(data_dir.path())
            .unwrap();
        let mut current = data_dir.path().to_path_buf();
        assert_eq!(
            std::fs::metadata(&current).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for component in relative_parent.components() {
            current.push(component.as_os_str());
            assert_eq!(
                std::fs::metadata(&current).unwrap().permissions().mode() & 0o777,
                0o700,
                "{} is not owner-only",
                current.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn protected_principal_root_writes_reject_insecure_or_symlink_parents_without_writing() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        for parent_kind in ["insecure", "symlink"] {
            let data_dir = tempfile::tempdir().unwrap();
            std::fs::set_permissions(data_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let principal_id = format!("person:local:protected-parent-{parent_kind}");
            let protection = store_test_principal_root_protection(data_dir.path(), &principal_id);
            let object_uri = format!(
                "{}/.AppData/ElastOS/People/contact-state.json",
                protection.localhost_root
            );
            let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
            let users = data_dir.path().join("Users");

            if parent_kind == "insecure" {
                std::fs::create_dir(&users).unwrap();
                std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o755)).unwrap();
            } else {
                let outside = tempfile::tempdir().unwrap();
                symlink(outside.path(), &users).unwrap();
            }

            let error = write_protected_principal_root_object(
                data_dir.path(),
                &principal_id,
                &protection.localhost_root,
                &object_uri,
                &path,
                b"protected",
            )
            .unwrap_err();
            assert!(error
                .to_string()
                .contains("protected principal-root parent"));
            assert!(!path.exists());
        }
    }

    #[test]
    fn principal_root_write_rejects_a_path_that_does_not_match_its_uri() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:protected-wrong-path";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/expected.md", protection.localhost_root);
        let escaped = data_dir
            .path()
            .join("outside")
            .join("..")
            .join("escaped")
            .join("wrong.md");

        let error = write_protected_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &escaped,
            b"protected",
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("does not match its canonical URI path"));
        assert!(!data_dir.path().join("escaped").exists());
    }

    #[test]
    fn principal_root_object_rejects_plaintext_when_root_is_protected() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:protected-plaintext";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/plaintext.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"plaintext").unwrap();

        let err = read_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("protected principal-root object is not encrypted"));
    }

    #[test]
    fn principal_root_object_read_rejects_symlink_and_non_regular_paths() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:protected-read-path";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/read-check.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(data_dir.path().join("missing-object"), &path).unwrap();
            let err = read_principal_root_object(
                data_dir.path(),
                principal_id,
                &protection.localhost_root,
                &object_uri,
                &path,
            )
            .unwrap_err();
            assert!(err.to_string().contains("failed to read"));
            std::fs::remove_file(&path).unwrap();
        }

        std::fs::create_dir_all(&path).unwrap();
        let err = read_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
        )
        .unwrap_err();
        assert!(err.to_string().contains("regular file"));
    }

    #[test]
    fn root_protection_activation_rejects_untouched_declared_plaintext_without_mutation() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:gba-transition";
        let localhost_root = principal_localhost_root(principal_id);
        let object_uri = format!("{localhost_root}/.AppData/LocalHost/GBA/ucity/rom-id.sav");
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"existing uCity save").unwrap();
        let inventory = [PrincipalRootProtectedObjectDeclarationV1::root(format!(
            "{localhost_root}/.AppData/LocalHost/GBA"
        ))];
        let before = std::fs::read(
            auth_state_path(data_dir.path())
                .unwrap_or_else(|_| data_dir.path().join("missing-auth-state")),
        )
        .ok();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &inventory,
        )
        .expect_err("untouched declared plaintext must block root protection");
        let outcome = err
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .expect("typed migration-required outcome");

        assert_eq!(outcome.schema, PRINCIPAL_ROOT_MIGRATION_REQUIRED_SCHEMA);
        assert_eq!(outcome.code, "declared_plaintext_objects");
        assert_eq!(outcome.plaintext_object_count, 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"existing uCity save");
        assert_eq!(
            std::fs::read(auth_state_path(data_dir.path()).unwrap()).ok(),
            before
        );
        assert!(
            load_principal_root_protection(data_dir.path(), principal_id, &localhost_root)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn recovery_activation_migrates_first_run_plaintext_under_new_protection() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:first-run-shell-state";
        let localhost_root = principal_localhost_root(principal_id);
        // The shell persists Home browser state before a person can reach
        // Recovery — the exact object that used to deadlock first-run
        // activation (activation demanded a clean root, migration demanded
        // existing protection).
        let object_uri = format!("{localhost_root}/.AppData/ElastOS/Home/browser-state.json");
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"schema":"home-state","windows":[]}"#).unwrap();
        let inventory = [PrincipalRootProtectedObjectDeclarationV1::exact(
            object_uri.clone(),
        )];

        let activation =
            begin_declarative_principal_root_protection_activation_migrating_plaintext(
                data_dir.path(),
                principal_id,
                &localhost_root,
                &inventory,
            )
            .expect("first-run plaintext must not block migrating activation");
        assert_eq!(activation.plaintext_objects.len(), 1);
        assert_eq!(activation.plaintext_objects[0].object_uri, object_uri);

        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        assert_eq!(protection.localhost_root, localhost_root);

        let receipt = migrate_principal_root_plaintext_objects_under_activation(
            &activation.guard,
            data_dir.path(),
            principal_id,
            &localhost_root,
            activation.plaintext_objects.clone(),
        )
        .expect("plaintext migrates online under the activation guard");
        assert_eq!(receipt.object_count, 1);
        drop(activation);

        let migrated = std::fs::read(&path).unwrap();
        assert!(
            looks_like_principal_root_object_envelope(&migrated),
            "migrated object must be an envelope"
        );

        begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &inventory,
        )
        .expect("strict activation passes once the root is clean");
    }

    #[test]
    fn root_protection_activation_rejects_plaintext_despite_existing_protection() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:configured-plaintext";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/plaintext.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"legacy plaintext").unwrap();
        let before = std::fs::read(auth_state_path(data_dir.path()).unwrap()).unwrap();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect_err("a protection record must not excuse declared plaintext");
        let outcome = err
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .expect("typed migration-required outcome");

        assert_eq!(outcome.code, "declared_plaintext_objects");
        assert_eq!(outcome.plaintext_object_count, 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"legacy plaintext");
        assert_eq!(
            std::fs::read(auth_state_path(data_dir.path()).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn root_protection_activation_accepts_exact_retained_key_binding() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:retained-binding";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/exact.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        write_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
            b"exact retained key",
        )
        .unwrap();

        let _activation = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect("the exact retained protection must authenticate its envelope");
    }

    #[test]
    fn root_protection_activation_rejects_substituted_data_key_binding() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:retained-key";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let other_data_dir = tempfile::tempdir().unwrap();
        let other_protection =
            store_test_principal_root_protection(other_data_dir.path(), "person:local:other-key");
        let other_data_key =
            principal_root_data_key_from_protection(other_data_dir.path(), &other_protection)
                .unwrap();
        let object_uri = format!("{}/Documents/substituted.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let encrypted = encrypt_principal_root_object_bytes(
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &other_protection,
            &other_data_key,
            b"wrong key",
        )
        .unwrap();
        std::fs::write(&path, &encrypted).unwrap();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect_err("a substituted data-key id must fail closed");

        assert!(err.to_string().contains("envelope binding is invalid"));
        assert_eq!(std::fs::read(path).unwrap(), encrypted);
    }

    #[test]
    fn root_protection_activation_rejects_tampered_ciphertext() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:tampered-envelope";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/tampered.md", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        write_principal_root_object(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &object_uri,
            &path,
            b"authenticated body",
        )
        .unwrap();
        let mut envelope: PrincipalRootObjectEnvelopeV1 =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut ciphertext = b64_url_decode(&envelope.ciphertext).unwrap();
        ciphertext[0] ^= 0x01;
        envelope.ciphertext = b64_url(&ciphertext);
        let tampered = serde_json::to_vec_pretty(&envelope).unwrap();
        std::fs::write(&path, &tampered).unwrap();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect_err("tampered ciphertext must fail authentication");

        assert!(err.to_string().contains("envelope authentication failed"));
        assert_eq!(std::fs::read(path).unwrap(), tampered);
    }

    #[test]
    fn root_protection_activation_rejects_malformed_envelope_distinctly() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:malformed-envelope";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!("{}/Documents/malformed.json", protection.localhost_root);
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let malformed = format!(
            "{{\"schema\":\"{PRINCIPAL_ROOT_OBJECT_SCHEMA}\",\"data_key_id\":\"{}\",\"nonce\":\"AAAAAAAAAAAAAAAA\",\"ciphertext\":",
            protection.data_key_id
        )
        .into_bytes();
        std::fs::write(&path, &malformed).unwrap();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect_err("a malformed envelope must not be classified as plaintext");

        assert!(err.to_string().contains("envelope is malformed"));
        assert!(err
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .is_none());
        assert_eq!(std::fs::read(path).unwrap(), malformed);
    }

    #[test]
    fn fresh_root_protection_rejects_preexisting_envelope_shaped_data() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:fresh-envelope";
        let localhost_root = principal_localhost_root(principal_id);
        let other_data_dir = tempfile::tempdir().unwrap();
        let other_protection =
            store_test_principal_root_protection(other_data_dir.path(), "person:local:source-key");
        let other_data_key =
            principal_root_data_key_from_protection(other_data_dir.path(), &other_protection)
                .unwrap();
        let object_uri = format!("{localhost_root}/Documents/envelope.json");
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let encrypted = encrypt_principal_root_object_bytes(
            principal_id,
            &localhost_root,
            &object_uri,
            &other_protection,
            &other_data_key,
            b"unbound encrypted body",
        )
        .unwrap();
        std::fs::write(&path, &encrypted).unwrap();

        let err = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::exact(object_uri)],
        )
        .expect_err("fresh activation requires an empty declared inventory");

        assert!(err
            .to_string()
            .contains("exact verified protection binding is required"));
        assert_eq!(std::fs::read(path).unwrap(), encrypted);
    }

    #[test]
    fn declarative_preflight_verifies_envelopes_even_when_plaintext_exists() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:mixed-tamper";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let data_key =
            principal_root_data_key_from_protection(data_dir.path(), &protection).unwrap();
        let plaintext_uri = format!("{}/Documents/plain.txt", protection.localhost_root);
        let encrypted_uri = format!("{}/Documents/encrypted.txt", protection.localhost_root);
        let plaintext_path = rooted_localhost_fs_path(data_dir.path(), &plaintext_uri).unwrap();
        let encrypted_path = rooted_localhost_fs_path(data_dir.path(), &encrypted_uri).unwrap();
        std::fs::create_dir_all(plaintext_path.parent().unwrap()).unwrap();
        std::fs::write(&plaintext_path, b"legacy plaintext").unwrap();
        let mut envelope: PrincipalRootObjectEnvelopeV1 = serde_json::from_slice(
            &encrypt_principal_root_object_bytes(
                principal_id,
                &protection.localhost_root,
                &encrypted_uri,
                &protection,
                &data_key,
                b"protected bytes",
            )
            .unwrap(),
        )
        .unwrap();
        let replacement = if envelope.ciphertext.starts_with('A') {
            "B"
        } else {
            "A"
        };
        envelope.ciphertext.replace_range(..1, replacement);
        let tampered = serde_json::to_vec_pretty(&envelope).unwrap();
        std::fs::write(&encrypted_path, &tampered).unwrap();

        let error = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &protection.localhost_root,
            &[
                PrincipalRootProtectedObjectDeclarationV1::exact(plaintext_uri),
                PrincipalRootProtectedObjectDeclarationV1::exact(encrypted_uri),
            ],
        )
        .expect_err("tampering must fail before plaintext migration is considered");

        assert!(error.to_string().contains("envelope authentication failed"));
        assert!(error
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .is_none());
        assert_eq!(std::fs::read(plaintext_path).unwrap(), b"legacy plaintext");
        assert_eq!(std::fs::read(encrypted_path).unwrap(), tampered);
    }

    #[test]
    fn offline_migration_uses_explicit_hashes_owner_only_backups_and_round_trip() {
        let data_dir = tempfile::tempdir().unwrap();
        let operator_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:offline-migration";
        let protection = store_test_principal_root_protection(data_dir.path(), principal_id);
        let object_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
            protection.localhost_root
        );
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let plaintext = b"legacy save bytes";
        std::fs::write(&path, plaintext).unwrap();
        let plan = PrincipalRootMigrationPlanV1 {
            schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: protection.localhost_root.clone(),
            objects: vec![PrincipalRootMigrationSelectionV1 {
                object_uri: object_uri.clone(),
                plaintext_sha256: sha256_label(plaintext),
            }],
        };
        let plan_path = operator_dir.path().join("migration-plan.json");
        let plan_bytes = serde_json::to_vec_pretty(&plan).unwrap();
        let plan_text = String::from_utf8(plan_bytes.clone()).unwrap();
        assert!(!plan_text.contains("recovery_kit"));
        assert!(!plan_text.contains("recovery_phrase"));
        assert!(!plan_text.contains("wrapped_data_key"));
        assert!(!plan_text.contains("encrypted_root_descriptor"));
        let mut plan_with_secret = serde_json::to_value(&plan).unwrap();
        plan_with_secret["recovery_kit"] =
            serde_json::json!({"recovery_phrase": "must never enter a plan"});
        assert!(serde_json::from_value::<PrincipalRootMigrationPlanV1>(plan_with_secret).is_err());
        write_owner_only_new_file(&plan_path, &plan_bytes).unwrap();
        let backup_dir = operator_dir.path().join("backups");

        let receipt =
            migrate_principal_root_objects_offline(data_dir.path(), &plan_path, &backup_dir)
                .unwrap();

        assert_eq!(receipt.schema, PRINCIPAL_ROOT_MIGRATION_RECEIPT_SCHEMA);
        assert_eq!(receipt.object_count, 1);
        assert!(receipt.objects[0].round_trip_verified);
        assert!(!serde_json::to_string(&receipt)
            .unwrap()
            .contains(&object_uri));
        assert_ne!(std::fs::read(&path).unwrap(), plaintext);
        assert_eq!(
            read_principal_root_object(
                data_dir.path(),
                principal_id,
                &protection.localhost_root,
                &object_uri,
                &path,
            )
            .unwrap(),
            plaintext
        );
        let backups = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".plaintext.backup")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert!(backup_dir
            .join(PRINCIPAL_ROOT_MIGRATION_RECEIPT_FILE)
            .is_file());
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), plaintext);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                backups[0].metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    struct OfflineMigrationFixture {
        data_dir: tempfile::TempDir,
        operator_dir: tempfile::TempDir,
        principal_id: String,
        localhost_root: String,
        object_uris: Vec<String>,
        object_paths: Vec<PathBuf>,
        plaintexts: Vec<Vec<u8>>,
        plan_path: PathBuf,
        backup_dir: PathBuf,
    }

    impl OfflineMigrationFixture {
        fn new(object_count: usize) -> Self {
            let data_dir = tempfile::tempdir().unwrap();
            let operator_dir = tempfile::tempdir().unwrap();
            let principal_id = "person:local:migration-fixture".to_string();
            let protection = store_test_principal_root_protection(data_dir.path(), &principal_id);
            let mut object_uris = Vec::new();
            let mut object_paths = Vec::new();
            let mut plaintexts = Vec::new();
            let mut objects = Vec::new();
            for index in 0..object_count {
                let object_uri = format!(
                    "{}/.AppData/LocalHost/GBA/ucity/save-{index}.sav",
                    protection.localhost_root
                );
                let object_path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();
                std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
                let plaintext = format!("legacy save {index}").into_bytes();
                std::fs::write(&object_path, &plaintext).unwrap();
                objects.push(PrincipalRootMigrationSelectionV1 {
                    object_uri: object_uri.clone(),
                    plaintext_sha256: sha256_label(&plaintext),
                });
                object_uris.push(object_uri);
                object_paths.push(object_path);
                plaintexts.push(plaintext);
            }
            let plan = PrincipalRootMigrationPlanV1 {
                schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
                principal_id: principal_id.clone(),
                localhost_root: protection.localhost_root.clone(),
                objects,
            };
            let plan_path = operator_dir.path().join("migration-plan.json");
            write_owner_only_new_file(&plan_path, &serde_json::to_vec_pretty(&plan).unwrap())
                .unwrap();
            let backup_dir = operator_dir.path().join("backups");
            Self {
                data_dir,
                operator_dir,
                principal_id,
                localhost_root: protection.localhost_root,
                object_uris,
                object_paths,
                plaintexts,
                plan_path,
                backup_dir,
            }
        }

        fn assert_all_plaintext(&self) {
            for (path, plaintext) in self.object_paths.iter().zip(&self.plaintexts) {
                assert_eq!(std::fs::read(path).unwrap(), *plaintext);
            }
        }

        fn assert_all_encrypted(&self) {
            for ((uri, path), plaintext) in self
                .object_uris
                .iter()
                .zip(&self.object_paths)
                .zip(&self.plaintexts)
            {
                let stored = std::fs::read(path).unwrap();
                assert_ne!(stored, *plaintext);
                assert_eq!(
                    read_principal_root_object(
                        self.data_dir.path(),
                        &self.principal_id,
                        &self.localhost_root,
                        uri,
                        path,
                    )
                    .unwrap(),
                    *plaintext
                );
            }
        }
    }

    #[test]
    fn offline_migration_changes_only_exactly_selected_objects() {
        let fixture = OfflineMigrationFixture::new(1);
        let unselected_uri = format!(
            "{}/.AppData/LocalHost/GBA/ucity/unselected.sav",
            fixture.localhost_root
        );
        let unselected_path =
            rooted_localhost_fs_path(fixture.data_dir.path(), &unselected_uri).unwrap();
        std::fs::write(&unselected_path, b"unselected legacy save").unwrap();

        migrate_principal_root_objects_offline(
            fixture.data_dir.path(),
            &fixture.plan_path,
            &fixture.backup_dir,
        )
        .unwrap();

        fixture.assert_all_encrypted();
        assert_eq!(
            std::fs::read(unselected_path).unwrap(),
            b"unselected legacy save"
        );
    }

    #[test]
    fn offline_migration_faults_leave_all_plaintext_or_all_verified_encrypted() {
        for fault in [
            PrincipalRootMigrationTestFault::Backup(0),
            PrincipalRootMigrationTestFault::Backup(1),
            PrincipalRootMigrationTestFault::Stage(0),
            PrincipalRootMigrationTestFault::Stage(1),
            PrincipalRootMigrationTestFault::JournalPrepared,
            PrincipalRootMigrationTestFault::JournalCommitting,
            PrincipalRootMigrationTestFault::Commit(0),
            PrincipalRootMigrationTestFault::Commit(1),
            PrincipalRootMigrationTestFault::JournalCommitted,
        ] {
            let fixture = OfflineMigrationFixture::new(2);
            inject_principal_root_migration_test_fault(fixture.data_dir.path(), fault);
            migrate_principal_root_objects_offline(
                fixture.data_dir.path(),
                &fixture.plan_path,
                &fixture.backup_dir,
            )
            .expect_err("injected migration fault must fail");

            if fault == PrincipalRootMigrationTestFault::JournalCommitted {
                fixture.assert_all_encrypted();
            } else {
                fixture.assert_all_plaintext();
            }
            assert!(
                !principal_root_migration_journal_path(fixture.data_dir.path())
                    .unwrap()
                    .exists()
            );
        }
    }

    #[test]
    fn offline_migration_recovers_partial_commit_after_restart() {
        let fixture = OfflineMigrationFixture::new(2);
        inject_principal_root_migration_test_fault(
            fixture.data_dir.path(),
            PrincipalRootMigrationTestFault::CrashAfterCommit(0),
        );
        migrate_principal_root_objects_offline(
            fixture.data_dir.path(),
            &fixture.plan_path,
            &fixture.backup_dir,
        )
        .expect_err("simulated crash must interrupt migration");
        assert!(
            principal_root_migration_journal_path(fixture.data_dir.path())
                .unwrap()
                .exists()
        );
        assert_ne!(
            std::fs::read(&fixture.object_paths[0]).unwrap(),
            fixture.plaintexts[0]
        );
        assert_eq!(
            std::fs::read(&fixture.object_paths[1]).unwrap(),
            fixture.plaintexts[1]
        );

        migrate_principal_root_objects_offline(
            fixture.data_dir.path(),
            &fixture.plan_path,
            &fixture.backup_dir,
        )
        .expect_err("recovered backups remain operator-owned and require a fresh target");

        fixture.assert_all_plaintext();
        assert!(
            !principal_root_migration_journal_path(fixture.data_dir.path())
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn offline_upgrade_preflights_every_configured_root_before_mutation() {
        let data_dir = tempfile::tempdir().unwrap();
        let first =
            store_test_principal_root_protection(data_dir.path(), "person:local:first-root");
        let second =
            store_test_principal_root_protection(data_dir.path(), "person:local:second-root");
        let first_uri = format!("{}/Documents/first.txt", first.localhost_root);
        let first_path = rooted_localhost_fs_path(data_dir.path(), &first_uri).unwrap();
        std::fs::create_dir_all(first_path.parent().unwrap()).unwrap();
        std::fs::write(&first_path, b"first plaintext").unwrap();
        let backup_dir = data_dir
            .path()
            .join("backups/principal-root-upgrade-missing-root");
        let declarations = vec![PrincipalRootUpgradeDeclarationV1 {
            principal_id: first.principal_id.clone(),
            localhost_root: first.localhost_root.clone(),
            inventory: vec![PrincipalRootProtectedObjectDeclarationV1::exact(first_uri)],
        }];

        let error = migrate_declared_principal_roots_offline(data_dir.path(), &backup_dir, || {
            Ok(declarations)
        })
        .expect_err("omitting a configured protected root must fail before migration");

        assert!(error
            .to_string()
            .contains("must exactly match configured protections"));
        assert_eq!(std::fs::read(first_path).unwrap(), b"first plaintext");
        assert!(!backup_dir.exists());
        assert_eq!(
            load_principal_root_protection(
                data_dir.path(),
                &second.principal_id,
                &second.localhost_root,
            )
            .unwrap(),
            Some(second)
        );
    }

    #[test]
    fn offline_upgrade_rejects_tampered_later_root_before_first_root_mutates() {
        let data_dir = tempfile::tempdir().unwrap();
        let first =
            store_test_principal_root_protection(data_dir.path(), "person:local:first-plaintext");
        let second =
            store_test_principal_root_protection(data_dir.path(), "person:local:second-tampered");
        let first_uri = format!("{}/Documents/first.txt", first.localhost_root);
        let second_uri = format!("{}/Documents/second.txt", second.localhost_root);
        let first_path = rooted_localhost_fs_path(data_dir.path(), &first_uri).unwrap();
        let second_path = rooted_localhost_fs_path(data_dir.path(), &second_uri).unwrap();
        std::fs::create_dir_all(first_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second_path.parent().unwrap()).unwrap();
        std::fs::write(&first_path, b"first plaintext").unwrap();
        let second_data_key =
            principal_root_data_key_from_protection(data_dir.path(), &second).unwrap();
        let mut second_envelope: PrincipalRootObjectEnvelopeV1 = serde_json::from_slice(
            &encrypt_principal_root_object_bytes(
                &second.principal_id,
                &second.localhost_root,
                &second_uri,
                &second,
                &second_data_key,
                b"second protected",
            )
            .unwrap(),
        )
        .unwrap();
        let replacement = if second_envelope.ciphertext.starts_with('A') {
            "B"
        } else {
            "A"
        };
        second_envelope.ciphertext.replace_range(..1, replacement);
        let tampered = serde_json::to_vec_pretty(&second_envelope).unwrap();
        std::fs::write(&second_path, &tampered).unwrap();
        let declarations = vec![
            PrincipalRootUpgradeDeclarationV1 {
                principal_id: first.principal_id.clone(),
                localhost_root: first.localhost_root.clone(),
                inventory: vec![PrincipalRootProtectedObjectDeclarationV1::exact(first_uri)],
            },
            PrincipalRootUpgradeDeclarationV1 {
                principal_id: second.principal_id.clone(),
                localhost_root: second.localhost_root.clone(),
                inventory: vec![PrincipalRootProtectedObjectDeclarationV1::exact(second_uri)],
            },
        ];
        let backup_dir = data_dir
            .path()
            .join("backups/principal-root-upgrade-tampered-root");

        let error = migrate_declared_principal_roots_offline(data_dir.path(), &backup_dir, || {
            Ok(declarations)
        })
        .expect_err("tampering in a later root must fail the complete preflight");

        assert!(error.to_string().contains("envelope authentication failed"));
        assert_eq!(std::fs::read(first_path).unwrap(), b"first plaintext");
        assert_eq!(std::fs::read(second_path).unwrap(), tampered);
        assert!(!backup_dir.exists());
    }

    #[test]
    fn offline_upgrade_recovers_incomplete_journal_and_requires_an_explicit_rerun() {
        let fixture = OfflineMigrationFixture::new(2);
        inject_principal_root_migration_test_fault(
            fixture.data_dir.path(),
            PrincipalRootMigrationTestFault::CrashAfterCommit(0),
        );
        migrate_principal_root_objects_offline(
            fixture.data_dir.path(),
            &fixture.plan_path,
            &fixture.backup_dir,
        )
        .expect_err("simulated crash must leave a journal");
        assert!(
            principal_root_migration_journal_path(fixture.data_dir.path())
                .unwrap()
                .exists()
        );
        let backup_dir = fixture
            .data_dir
            .path()
            .join("backups/principal-root-upgrade-rerun");
        let declaration = PrincipalRootUpgradeDeclarationV1 {
            principal_id: fixture.principal_id.clone(),
            localhost_root: fixture.localhost_root.clone(),
            inventory: vec![PrincipalRootProtectedObjectDeclarationV1::root(format!(
                "{}/.AppData/LocalHost/GBA",
                fixture.localhost_root
            ))],
        };
        let readiness_error = verify_declared_principal_roots_ready(
            fixture.data_dir.path(),
            std::slice::from_ref(&declaration),
        )
        .expect_err("startup readiness must reject an incomplete journal");
        assert!(readiness_error
            .to_string()
            .contains("migration is incomplete"));

        let error =
            migrate_declared_principal_roots_offline(fixture.data_dir.path(), &backup_dir, || {
                Ok(vec![declaration])
            })
            .expect_err("recovery must stop this upgrade attempt");

        assert!(error
            .to_string()
            .contains("recovered an incomplete principal-root migration"));
        fixture.assert_all_plaintext();
        assert!(
            !principal_root_migration_journal_path(fixture.data_dir.path())
                .unwrap()
                .exists()
        );
        assert!(!backup_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn declarative_inventory_and_migration_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:inventory-symlink";
        let localhost_root = principal_localhost_root(principal_id);
        let root_uri = format!("{localhost_root}/.AppData/LocalHost/GBA");
        let root_path = rooted_localhost_fs_path(data_dir.path(), &root_uri).unwrap();
        std::fs::create_dir_all(&root_path).unwrap();
        let outside = data_dir.path().join("outside.sav");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root_path.join("linked.sav")).unwrap();
        let error = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::root(root_uri)],
        )
        .unwrap_err();
        assert!(error.to_string().contains("rejects symlinks"));

        let fixture = OfflineMigrationFixture::new(1);
        std::fs::remove_file(&fixture.object_paths[0]).unwrap();
        let outside = fixture.operator_dir.path().join("outside.sav");
        std::fs::write(&outside, &fixture.plaintexts[0]).unwrap();
        symlink(outside, &fixture.object_paths[0]).unwrap();
        let error = migrate_principal_root_objects_offline(
            fixture.data_dir.path(),
            &fixture.plan_path,
            &fixture.backup_dir,
        )
        .unwrap_err();
        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn declarative_inventory_and_migration_plans_are_bounded() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:inventory-bound";
        let localhost_root = principal_localhost_root(principal_id);
        let inventory = (0..=MAX_PRINCIPAL_ROOT_INVENTORY_DECLARATIONS)
            .map(|index| {
                PrincipalRootProtectedObjectDeclarationV1::exact(format!(
                    "{localhost_root}/Documents/{index}.txt"
                ))
            })
            .collect::<Vec<_>>();
        let error = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &inventory,
        )
        .unwrap_err();
        assert!(error.to_string().contains("declaration bound"));

        let deep_root_uri = format!("{localhost_root}/.AppData/Deep");
        let mut deep_path = rooted_localhost_fs_path(data_dir.path(), &deep_root_uri).unwrap();
        for index in 0..=MAX_PRINCIPAL_ROOT_INVENTORY_DEPTH {
            deep_path.push(index.to_string());
        }
        std::fs::create_dir_all(&deep_path).unwrap();
        std::fs::write(deep_path.join("object.bin"), b"deep").unwrap();
        let error = begin_declarative_principal_root_protection_activation(
            data_dir.path(),
            principal_id,
            &localhost_root,
            &[PrincipalRootProtectedObjectDeclarationV1::root(
                deep_root_uri,
            )],
        )
        .unwrap_err();
        assert!(error.to_string().contains("depth bound"));

        let plan = PrincipalRootMigrationPlanV1 {
            schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.clone(),
            objects: (0..=MAX_MIGRATION_OBJECTS)
                .map(|index| PrincipalRootMigrationSelectionV1 {
                    object_uri: format!("{localhost_root}/Documents/{index}.txt"),
                    plaintext_sha256: sha256_label(format!("{index}").as_bytes()),
                })
                .collect(),
        };
        let error = validate_principal_root_migration_plan(&plan).unwrap_err();
        assert!(error.to_string().contains("1-64 objects"));
    }

    #[test]
    fn passkey_revoke_marks_binding_and_sessions_revoked() {
        let data_dir = tempfile::tempdir().unwrap();
        let now = 100;
        let principal =
            upsert_principal_for_binding(data_dir.path(), passkey_binding(1, now, now), now)
                .unwrap();
        let grant = AuthSessionGrantV1 {
            schema: AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: "grant-1".to_string(),
            session_id: "session-1".to_string(),
            principal_id: principal.principal_id.clone(),
            proof_binding_id: principal.proof_binding_id.clone(),
            issued_at: now,
            expires_at: now + 100,
            apps: vec!["home".to_string()],
        };
        store_session_grant(data_dir.path(), grant).unwrap();

        revoke_passkey_binding(data_dir.path(), &principal.proof_binding_id, now + 1).unwrap();

        assert!(!is_auth_session_active(data_dir.path(), "session-1", now + 2).unwrap());
        let passkey = list_passkey_principals(data_dir.path()).unwrap()[0]
            .proof_binding
            .passkey
            .clone()
            .unwrap();
        assert_eq!(passkey.revoked_at, Some(now + 1));
        let record =
            load_principal_for_proof_binding(data_dir.path(), &principal.proof_binding_id).unwrap();
        let err = ensure_proof_binding_not_revoked(&record)
            .unwrap_err()
            .to_string();
        assert!(err.contains("revoked"));
    }
}
