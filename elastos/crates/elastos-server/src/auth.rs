//! Runtime-owned authentication state for proof-bound sessions.

use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Write},
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

static AUTH_STATE_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static AUDIT_CHAIN_ACTIVATION_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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

fn auth_state_mutation_lock() -> &'static Mutex<()> {
    AUTH_STATE_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
}

fn audit_chain_activation_mutation_lock() -> &'static Mutex<()> {
    AUDIT_CHAIN_ACTIVATION_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePrincipalRole {
    Admin,
    Guest,
}

impl Default for RuntimePrincipalRole {
    fn default() -> Self {
        // Existing local runtimes with a single pre-role passkey remain recoverable.
        Self::Admin
    }
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

pub fn read_principal_root_object(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
) -> anyhow::Result<Vec<u8>> {
    validate_principal_root_object_binding(principal_id, localhost_root, object_uri)?;
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {path:?}"))?;
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

pub fn write_principal_root_object(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    object_uri: &str,
    path: &Path,
    plaintext: &[u8],
) -> anyhow::Result<()> {
    validate_principal_root_object_binding(principal_id, localhost_root, object_uri)?;
    let bytes = if let Some(protection) =
        load_principal_root_protection(data_dir, principal_id, localhost_root)?
    {
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
    if let Err(err) = std::fs::write(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(err.into());
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(err.into());
    }
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
    }

    #[test]
    fn principal_root_object_stays_plaintext_without_protection() {
        let data_dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:plain-root";
        let localhost_root = principal_localhost_root(principal_id);
        let object_uri = format!("{localhost_root}/Documents/plain.md");
        let path = rooted_localhost_fs_path(data_dir.path(), &object_uri).unwrap();

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
