//! Runtime-owned authentication state for proof-bound sessions.

use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::{anyhow, Context};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_runtime::auth::{
    validate_principal_root_protection, validate_recovery_kit_package, AuthChallengeV1,
    AuthSessionGrantV1, PrincipalRootProtectionV1, PrincipalRootProtectorKind,
    PrincipalRootRecoveryArchiveV1, ProofBinding, RecoveryKitPackageProtectionV1,
    RecoveryKitPackageV1, RecoveryKitV1, RuntimeAuditEventV1, DEFAULT_PRINCIPAL_ROOT_CIPHER,
};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const AUTH_STATE_SCHEMA: &str = "elastos.auth.state/v1";
const AUTH_STATE_ROOT: &str = "ElastOS/System/Auth";
const AUTH_STATE_FILE: &str = "auth-state.json";
const RECOVERY_ARCHIVE_KEY_FILE: &str = "recovery-archive.key";
pub(crate) const AUDIT_EVENT_DOMAIN: &str = "elastos.audit.event.v1";
const RECOVERY_DESCRIPTOR_SCHEMA: &str = "elastos.principal.root-descriptor/v1";
const PRINCIPAL_ROOT_OBJECT_SCHEMA: &str = "elastos.principal-root.object/v1";
pub const PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED: &str =
    "protected principal-root object is not encrypted";
const PRINCIPAL_ROOT_OBJECT_AAD_DOMAIN: &str = "elastos.principal-root.object.v1";
const RECOVERY_KIT_PACKAGE_AAD_DOMAIN: &str = "elastos.recovery-kit.package.v1";
const RECOVERY_KIT_PACKAGE_KDF_PARAMS: &str = "m=19456,t=2,p=1,len=32";

static AUTH_STATE_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn auth_state_mutation_lock() -> &'static Mutex<()> {
    AUTH_STATE_MUTATION_LOCK.get_or_init(|| Mutex::new(()))
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
            guest_registration_enabled: false,
        }
    }
}

pub fn auth_state_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, AUTH_STATE_ROOT)
        .ok_or_else(|| anyhow!("invalid auth state root"))
        .map(|root| root.join(AUTH_STATE_FILE))
}

fn auth_state_lock_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    auth_state_path(data_dir).map(|path| path.with_file_name(format!("{AUTH_STATE_FILE}.lock")))
}

fn open_auth_state_lock(data_dir: &Path) -> anyhow::Result<File> {
    let path = auth_state_lock_path(data_dir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open auth state lock {path:?}"))
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

pub fn load_auth_state(data_dir: &Path) -> anyhow::Result<AuthState> {
    let path = auth_state_path(data_dir)?;
    if !path.is_file() {
        return Ok(AuthState::default());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("failed to read {path:?}"))?;
    let mut state: AuthState = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse auth state {path:?}"))?;
    if state.schema != AUTH_STATE_SCHEMA {
        anyhow::bail!("unsupported auth state schema");
    }
    normalize_principal_records(&mut state);
    prune_auth_state(&mut state, now_ts());
    Ok(state)
}

pub(crate) fn save_auth_state(data_dir: &Path, state: &AuthState) -> anyhow::Result<()> {
    let path = auth_state_path(data_dir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = path.with_file_name(format!(
        "{AUTH_STATE_FILE}.{}.{}.tmp",
        std::process::id(),
        unique
    ));
    std::fs::write(&temp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
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

pub(crate) fn password_protected_recovery_kit_package(
    kit: &RecoveryKitV1,
    password: &str,
) -> anyhow::Result<RecoveryKitPackageV1> {
    let mut salt = [0u8; 32];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let key = derive_recovery_package_key(
        password,
        &salt,
        &kit.principal_id,
        &kit.localhost_root,
        &kit.kit_id,
    )?;
    let bytes = serde_json::to_vec(kit)?;
    let encrypted_recovery_kit = encrypt_aes256_gcm_bytes_with_aad(
        &key,
        &nonce,
        &bytes,
        recovery_kit_package_aad(kit).as_bytes(),
    )?;
    let package = RecoveryKitPackageV1 {
        schema: elastos_runtime::auth::RECOVERY_KIT_PACKAGE_SCHEMA.to_string(),
        principal_id: kit.principal_id.clone(),
        localhost_root: kit.localhost_root.clone(),
        kit_id: kit.kit_id.clone(),
        created_at: kit.created_at,
        protection: RecoveryKitPackageProtectionV1 {
            cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
            kdf: "argon2id".to_string(),
            kdf_params: RECOVERY_KIT_PACKAGE_KDF_PARAMS.to_string(),
            salt: b64_url(&salt),
            nonce: b64_url(&nonce),
            encrypted_recovery_kit,
        },
    };
    validate_recovery_kit_package(&package).map_err(anyhow::Error::msg)?;
    Ok(package)
}

pub(crate) fn recovery_kit_from_password_package(
    package: &RecoveryKitPackageV1,
    password: &str,
) -> anyhow::Result<RecoveryKitV1> {
    validate_recovery_kit_package(package).map_err(anyhow::Error::msg)?;
    let salt = b64_url_decode(&package.protection.salt)?;
    let nonce = b64_url_decode(&package.protection.nonce)?;
    if salt.len() != 32 {
        anyhow::bail!("recovery kit package salt must be 32 bytes");
    }
    if nonce.len() != 12 {
        anyhow::bail!("recovery kit package nonce must be 12 bytes");
    }
    let key = derive_recovery_package_key(
        password,
        &salt,
        &package.principal_id,
        &package.localhost_root,
        &package.kit_id,
    )?;
    let ciphertext = b64_url_decode(&package.protection.encrypted_recovery_kit)?;
    let plaintext = decrypt_aes256_gcm_bytes_with_aad(
        &key,
        &nonce,
        &ciphertext,
        recovery_kit_package_aad(package).as_bytes(),
    )
    .map_err(|_| anyhow!("invalid recovery kit package password or ciphertext"))?;
    let kit: RecoveryKitV1 = serde_json::from_slice(&plaintext)?;
    if kit.principal_id != package.principal_id
        || kit.localhost_root != package.localhost_root
        || kit.kit_id != package.kit_id
    {
        anyhow::bail!("recovery kit package binding mismatch");
    }
    verify_recovery_kit_material(&kit)?;
    Ok(kit)
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

fn derive_recovery_package_key(
    password: &str,
    salt: &[u8],
    principal_id: &str,
    localhost_root: &str,
    kit_id: &str,
) -> anyhow::Result<[u8; 32]> {
    let params = Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|err| anyhow!("invalid recovery package KDF params: {err}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    let input = format!("{principal_id}:{localhost_root}:{kit_id}:{password}");
    argon2
        .hash_password_into(input.as_bytes(), salt, &mut key)
        .map_err(|err| anyhow!("recovery package key derivation failed: {err}"))?;
    Ok(key)
}

fn recovery_kit_package_aad(binding: impl RecoveryKitPackageBinding) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        RECOVERY_KIT_PACKAGE_AAD_DOMAIN,
        binding.principal_id(),
        binding.localhost_root(),
        binding.kit_id()
    )
}

trait RecoveryKitPackageBinding {
    fn principal_id(&self) -> &str;
    fn localhost_root(&self) -> &str;
    fn kit_id(&self) -> &str;
}

impl RecoveryKitPackageBinding for &RecoveryKitV1 {
    fn principal_id(&self) -> &str {
        &self.principal_id
    }

    fn localhost_root(&self) -> &str {
        &self.localhost_root
    }

    fn kit_id(&self) -> &str {
        &self.kit_id
    }
}

impl RecoveryKitPackageBinding for &RecoveryKitPackageV1 {
    fn principal_id(&self) -> &str {
        &self.principal_id
    }

    fn localhost_root(&self) -> &str {
        &self.localhost_root
    }

    fn kit_id(&self) -> &str {
        &self.kit_id
    }
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
        push_audit_event(state, event);
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

pub fn reassign_passkey_binding_to_recovered_root(
    data_dir: &Path,
    proof_binding_id: &str,
    recovered_principal_id: &str,
    recovered_localhost_root: &str,
    updated_at: u64,
) -> anyhow::Result<PrincipalRecord> {
    mutate_auth_state(data_dir, |state| {
        normalize_principal_records(state);
        ensure_recovered_root_reassignable_in_state(
            state,
            proof_binding_id,
            recovered_principal_id,
            recovered_localhost_root,
        )?;

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
        principal.principal_id = recovered_principal_id.to_string();
        principal.localhost_root = recovered_localhost_root.to_string();
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
        Ok(record)
    })
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
        push_audit_event(state, event);
        Ok(())
    })
}

pub fn sign_audit_event(
    data_dir: &Path,
    mut event: RuntimeAuditEventV1,
) -> anyhow::Result<RuntimeAuditEventV1> {
    if event.signature.is_some() {
        if event
            .signer_did
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            anyhow::bail!("signed audit event missing signer DID");
        }
        return Ok(event);
    }
    let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir)?;
    event.signer_did = Some(signer_did);
    event.signature = None;
    let bytes = serde_json::to_vec(&event)?;
    let (signature, _) =
        crate::crypto::domain_separated_sign(&signing_key, AUDIT_EVENT_DOMAIN, &bytes);
    event.signature = Some(signature);
    Ok(event)
}

fn push_audit_event(state: &mut AuthState, event: RuntimeAuditEventV1) {
    state.audit.push(event);
    if state.audit.len() > 512 {
        let keep_from = state.audit.len() - 512;
        state.audit.drain(0..keep_from);
    }
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
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, path)?;
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
