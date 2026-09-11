//! Credential persistence for passkey identity
//!
//! Stores WebAuthn credentials as encrypted JSON on disk using AES-256-GCM.
//! Multiple passkey principals may exist on one device. A shared device key is
//! auto-generated on first run and protects the local credential store at rest.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use curve25519_dalek::edwards::CompressedEdwardsY;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[path = "store_files.rs"]
mod files;
use files::IdentityFiles;

/// A stored passkey credential
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    /// Base64url-encoded credential ID
    pub credential_id: String,
    /// COSE public key bytes (base64url-encoded)
    pub public_key: String,
    /// Signature counter (for clone detection)
    pub sign_count: u32,
    /// Relying party ID
    pub rp_id: String,
}

/// Persisted identity data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityData {
    pub user_id: String,
    pub credentials: Vec<StoredCredential>,
    #[serde(default)]
    removal_generation: u64,
}

/// Immutable key history at accepted verification. Sign counters and additions
/// can advance while Runtime reconciles another registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestCredentialHistory {
    removal_generation: u64,
    keys: Vec<[u8; 32]>,
}

fn credential_key_digest(key: &StoredCredential) -> anyhow::Result<[u8; 32]> {
    Ok(Sha256::digest(serde_json::to_vec(&(
        &key.credential_id,
        &key.rp_id,
        &key.public_key,
    ))?)
    .into())
}

/// On-disk encrypted envelope
#[derive(Serialize, Deserialize)]
struct EncryptedEnvelope {
    version: u8,
    nonce: String,
    ciphertext: String,
}

/// Manages credential persistence on disk
pub struct IdentityStore {
    path: PathBuf,
    pub(crate) data: Option<IdentityData>,
    device_key: Zeroizing<[u8; 32]>,
    loaded_digest: Option<[u8; 32]>,
    #[cfg(test)]
    save_fault: Option<files::SaveFault>,
}

/// Multicodec prefix for Ed25519 public keys.
const MULTICODEC_ED25519_PUB: [u8; 2] = [0xed, 0x01];
const DID_KEY_PREFIX: &str = "did:key:z";
const DID_KEY_ED25519_PREFIX: &str = "did:key:z6Mk";
const DID_KEY_ED25519_LEN: usize = 56;
const DID_KEY_DECODED_BYTES: usize = 34;

fn encode_canonical_did_key_bytes(key_bytes: [u8; 32]) -> String {
    let mut encoded = Vec::with_capacity(34);
    encoded.extend_from_slice(&MULTICODEC_ED25519_PUB);
    encoded.extend_from_slice(&key_bytes);
    format!("did:key:z{}", bs58::encode(&encoded).into_string())
}

/// Encode a signing-key-derived Ed25519 public key as `did:key:z6Mk...`.
pub fn encode_signing_key_did(signing_key: &ed25519_dalek::SigningKey) -> String {
    encode_canonical_did_key_bytes(signing_key.verifying_key().to_bytes())
}

/// Encode one canonical Ed25519 verifying key as `did:key:z6Mk...`.
pub fn encode_did_key(verifying_key: &ed25519_dalek::VerifyingKey) -> anyhow::Result<String> {
    let bytes = verifying_key.to_bytes();
    validate_canonical_ed25519_verifying_key_bytes(bytes)?;
    Ok(encode_canonical_did_key_bytes(bytes))
}

/// Accept only one canonical Ed25519 public-key encoding for authority use.
pub fn validate_canonical_ed25519_verifying_key_bytes(
    bytes: [u8; 32],
) -> anyhow::Result<ed25519_dalek::VerifyingKey> {
    let compressed = CompressedEdwardsY(bytes);
    let point = compressed
        .decompress()
        .ok_or_else(|| anyhow::anyhow!("Ed25519 key must decompress"))?;
    if point.compress().to_bytes() != bytes {
        anyhow::bail!("Ed25519 key is not canonically compressed");
    }
    if point.is_small_order() {
        anyhow::bail!("Ed25519 key must not be small order");
    }
    if !point.is_torsion_free() {
        anyhow::bail!("Ed25519 key must be torsion free");
    }
    ed25519_dalek::VerifyingKey::from_bytes(&bytes)
        .map_err(|error| anyhow::anyhow!("invalid Ed25519 verifying key: {error}"))
}

/// Decode the one canonical Ed25519 `did:key` representation.
///
/// The round-trip check rejects alternate base58 spellings and any future
/// multicodec variant. Callers that need a Profile identity should retain the
/// returned key bytes as authority and treat the DID text as a projection.
pub fn decode_did_key(did: &str) -> anyhow::Result<ed25519_dalek::VerifyingKey> {
    if did.len() != DID_KEY_ED25519_LEN || !did.starts_with(DID_KEY_ED25519_PREFIX) {
        anyhow::bail!("DID must be one canonical Ed25519 did:key");
    }
    let multibase = did
        .strip_prefix(DID_KEY_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("DID must start with did:key:z"))?;
    let mut bytes = [0u8; DID_KEY_DECODED_BYTES];
    let decoded = bs58::decode(multibase)
        .onto(&mut bytes)
        .map_err(|error| anyhow::anyhow!("invalid base58 in DID: {error}"))?;
    if decoded != DID_KEY_DECODED_BYTES || bytes[..2] != MULTICODEC_ED25519_PUB {
        anyhow::bail!("DID must contain one Ed25519 public key");
    }
    let key_bytes: [u8; 32] = bytes[2..]
        .try_into()
        .map_err(|_| anyhow::anyhow!("DID must contain one Ed25519 public key"))?;
    let verifying_key = validate_canonical_ed25519_verifying_key_bytes(key_bytes)?;
    if encode_canonical_did_key_bytes(verifying_key.to_bytes()) != did {
        anyhow::bail!("DID is not canonical");
    }
    Ok(verifying_key)
}

/// Load the device key and derive a stable DID identity from it.
///
/// Returns `(SigningKey, did_string)`. The device_key file stays on disk
/// for existing local profiles (encryption at rest), but identity is always
/// the derived DID.
///
/// Derivation: `SHA-256("elastos-did-v1" || device_key)` → Ed25519 SigningKey.
pub fn load_or_create_did(data_dir: &Path) -> anyhow::Result<(ed25519_dalek::SigningKey, String)> {
    let device_key = load_or_create_device_key(data_dir)?;
    let (signing_key, did) = derive_did(&device_key);
    Ok((signing_key, did))
}

/// Read the existing device identity for authority checks. Initialization and
/// recovery own creation and durability; this path only validates current files.
pub fn load_existing_did(data_dir: &Path) -> anyhow::Result<(ed25519_dalek::SigningKey, String)> {
    let files = IdentityFiles::open_existing(data_dir)?;
    let key = read_device_key(&files)?
        .ok_or_else(|| anyhow::anyhow!("identity device.key is missing"))?;
    Ok(derive_did(&key))
}

/// Load the locally persisted DID nickname, if present.
pub fn load_nickname(data_dir: &Path) -> anyhow::Result<Option<String>> {
    let device_key = load_or_create_device_key(data_dir)?;
    load_nickname_with_device_key(data_dir, &device_key)
}

/// Validate and normalize a local DID nickname/handle.
pub fn validate_nickname(nickname: &str) -> anyhow::Result<String> {
    let nickname = nickname.trim();
    if nickname.is_empty() {
        anyhow::bail!("nickname must not be empty");
    }
    if nickname.chars().count() > 32 {
        anyhow::bail!("nickname must be 32 characters or fewer");
    }
    if nickname.chars().any(|ch| ch.is_control()) {
        anyhow::bail!("nickname must not contain control characters");
    }
    Ok(nickname.to_string())
}

/// Load the locally persisted DID nickname using an explicit device key.
pub fn load_nickname_with_device_key(
    data_dir: &Path,
    device_key: &[u8; 32],
) -> anyhow::Result<Option<String>> {
    let path = nickname_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }

    let encrypted = std::fs::read(&path)?;
    let plaintext = decrypt_data(&derive_storage_key(device_key), &encrypted)?;
    let nickname = String::from_utf8(plaintext)?;
    let nickname = nickname.trim().to_string();
    if nickname.is_empty() {
        Ok(None)
    } else {
        Ok(Some(nickname))
    }
}

/// Persist the local DID nickname without requiring a running host/runtime.
pub fn save_nickname(data_dir: &Path, nickname: &str) -> anyhow::Result<()> {
    let device_key = load_or_create_device_key(data_dir)?;
    save_nickname_with_device_key(data_dir, &device_key, nickname)
}

/// Persist the local DID nickname using an explicit device key.
pub fn save_nickname_with_device_key(
    data_dir: &Path,
    device_key: &[u8; 32],
    nickname: &str,
) -> anyhow::Result<()> {
    let nickname = validate_nickname(nickname)?;

    let path = nickname_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let encrypted = encrypt_data(&derive_storage_key(device_key), nickname.as_bytes())?;
    std::fs::write(&path, encrypted)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Derive an Ed25519 SigningKey + `did:key` from a 32-byte secret.
///
/// Derivation: `SHA-256("elastos-did-v1" || secret)` → Ed25519 SigningKey → `did:key:z6Mk...`
///
/// The secret is typically the device key (stable DID) or random bytes (ephemeral DID).
pub fn derive_did(secret: &[u8; 32]) -> (ed25519_dalek::SigningKey, String) {
    let mut hasher = Sha256::new();
    hasher.update(b"elastos-did-v1");
    hasher.update(secret);
    let derived: [u8; 32] = hasher.finalize().into();
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&derived);
    let did = encode_signing_key_did(&signing_key);
    (signing_key, did)
}

/// Canonical shared device-key path at `{data_dir}/identity/device.key`.
pub fn device_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("identity").join("device.key")
}

/// Load or create the shared device key at `{data_dir}/identity/device.key`.
///
/// Returns 32 random bytes wrapped in `Zeroizing`. The key file is created
/// with 0600 permissions on Unix.
pub fn load_or_create_device_key(data_dir: &Path) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let files = IdentityFiles::open(data_dir)?;
    if let Some(key) = read_device_key(&files)? {
        files.sync()?;
        Ok(key)
    } else {
        let mut key = Zeroizing::new([0u8; 32]);
        rand::thread_rng().fill_bytes(key.as_mut());
        files.replace(
            c"device.key",
            key.as_ref(),
            #[cfg(test)]
            None,
        )?;
        Ok(key)
    }
}

fn read_device_key(files: &IdentityFiles) -> anyhow::Result<Option<Zeroizing<[u8; 32]>>> {
    if let Some(bytes) = files.read(c"device.key")? {
        let bytes = Zeroizing::new(bytes);
        if bytes.len() != 32 {
            anyhow::bail!(
                "device.key has invalid length {} (expected 32)",
                bytes.len()
            );
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&bytes);
        if let Some(credentials) = files.read(c"credentials.json")? {
            decrypt_data(&key, &credentials).map_err(|_| {
                anyhow::anyhow!("identity credentials cannot be verified with device.key")
            })?;
        }
        Ok(Some(key))
    } else {
        if files.read(c"credentials.json")?.is_some() {
            anyhow::bail!("identity credentials exist but device.key is missing");
        }
        Ok(None)
    }
}

/// Encrypt plaintext bytes with AES-256-GCM, returning a JSON envelope.
fn encrypt_data(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("AES key init: {}", e))?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    // Keep the current aes-gcm API until the upstream generic-array 1.x
    // migration is available through a compatible aes-gcm release.
    #[allow(deprecated)]
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {}", e))?;

    let envelope = EncryptedEnvelope {
        version: 1,
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

/// Decrypt an `EncryptedEnvelope` (as raw bytes) with AES-256-GCM.
fn decrypt_data(key: &[u8; 32], data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let envelope: EncryptedEnvelope = serde_json::from_slice(data)?;
    let nonce_bytes = hex::decode(&envelope.nonce)?;
    let ciphertext = hex::decode(&envelope.ciphertext)?;

    if nonce_bytes.len() != 12 {
        anyhow::bail!("invalid nonce length {}", nonce_bytes.len());
    }

    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("AES key init: {}", e))?;
    // See encrypt_data above for the aes-gcm/generic-array migration context.
    #[allow(deprecated)]
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| anyhow::anyhow!("decryption failed: {}", e))
}

fn derive_storage_key(device_key: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, device_key);
    let mut okm = [0u8; 32];
    hk.expand(b"elastos-did-storage", &mut okm)
        .expect("HKDF expand");
    okm
}

fn nickname_path(data_dir: &Path) -> PathBuf {
    data_dir.join("did").join("nickname.enc")
}

impl IdentityStore {
    /// Create a new store at the given directory.
    ///
    /// Loads or creates the device key automatically.
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join("identity").join("credentials.json");
        let device_key = load_or_create_device_key(data_dir)?;
        Ok(Self {
            path,
            data: None,
            device_key,
            loaded_digest: None,
            #[cfg(test)]
            save_fault: None,
        })
    }

    /// Load credentials from disk (encrypted).
    pub fn load(&mut self) -> anyhow::Result<()> {
        let files = self.files();
        self.data = None;
        self.loaded_digest = None;
        self.reload_locked(&files?)
    }

    fn files(&self) -> anyhow::Result<IdentityFiles> {
        let files = IdentityFiles::open(
            self.path
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| anyhow::anyhow!("identity store has no data root"))?,
        )?;
        let key = Zeroizing::new(
            files
                .read(c"device.key")?
                .ok_or_else(|| anyhow::anyhow!("identity device.key is missing"))?,
        );
        if key.as_slice() != self.device_key.as_ref() {
            anyhow::bail!("identity device.key changed or is corrupt");
        }
        Ok(files)
    }

    fn reload_locked(&mut self, files: &IdentityFiles) -> anyhow::Result<()> {
        self.data = None;
        self.loaded_digest = None;
        if let Some(raw) = files.read(c"credentials.json")? {
            let plaintext = decrypt_data(&self.device_key, &raw)?;
            let data: IdentityData = serde_json::from_slice(&plaintext)?;
            validate_credentials(&data)?;
            self.loaded_digest = Some(Sha256::digest(&raw).into());
            self.data = Some(data);
        }
        Ok(())
    }

    /// Save only if the loaded encrypted snapshot is still current. Failed
    /// saves discard the proposed cache and reload durable facts.
    pub fn save(&mut self) -> anyhow::Result<()> {
        let files = match self.files() {
            Ok(files) => files,
            Err(error) => {
                self.data = None;
                self.loaded_digest = None;
                return Err(error);
            }
        };
        let result = (|| {
            let current = files.read(c"credentials.json")?;
            if current.as_ref().map(|raw| Sha256::digest(raw).into()) != self.loaded_digest {
                anyhow::bail!("identity credentials changed; reload before retrying");
            }
            self.save_locked(&files)
        })();
        if result.is_err() {
            // An unreadable durable store leaves the cache empty, never speculative.
            let _ = self.reload_locked(&files);
        }
        result
    }

    fn save_locked(&mut self, files: &IdentityFiles) -> anyhow::Result<()> {
        let Some(data) = &self.data else {
            return Ok(());
        };
        validate_credentials(data)?;
        let encrypted = encrypt_data(&self.device_key, &serde_json::to_vec(data)?)?;
        files.replace(
            c"credentials.json",
            &encrypted,
            #[cfg(test)]
            self.save_fault.take(),
        )?;
        self.loaded_digest = Some(Sha256::digest(&encrypted).into());
        Ok(())
    }

    /// Reconcile only a first-owner candidate bound by Runtime's durable owner.
    pub(crate) fn persist_first_owner(
        &mut self,
        candidate: &StoredCredential,
    ) -> anyhow::Result<String> {
        let files = self.files()?;
        let result = (|| {
            self.reload_locked(&files)?;
            if let Some(data) = &self.data {
                let [existing] = data.credentials.as_slice() else {
                    anyhow::bail!("Enrollment credential history conflicts");
                };
                if existing.credential_id != candidate.credential_id
                    || existing.public_key != candidate.public_key
                    || existing.rp_id != candidate.rp_id
                    || existing.sign_count < candidate.sign_count
                {
                    anyhow::bail!("Enrollment credential conflicts");
                }
                files.sync()?;
                return Ok(data.user_id.clone());
            }
            let user_id = self.add_credential(candidate.clone());
            self.save_locked(&files)?;
            Ok(user_id)
        })();
        if result.is_err() {
            let _ = self.reload_locked(&files);
        }
        result
    }

    pub(crate) fn guest_credential_history(&self) -> anyhow::Result<GuestCredentialHistory> {
        let data = self
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("guest credential history missing"))?;
        if data.credentials.len() > 1024 || data.removal_generation == u64::MAX {
            anyhow::bail!("guest credential history limit exceeded");
        }
        Ok(GuestCredentialHistory {
            removal_generation: data.removal_generation,
            keys: data
                .credentials
                .iter()
                .map(credential_key_digest)
                .collect::<anyhow::Result<_>>()?,
        })
    }

    /// Append only to the exact verified history, or reconcile the same saved key.
    pub(crate) fn persist_guest_candidate(
        &mut self,
        candidate: &StoredCredential,
        previous_history: &GuestCredentialHistory,
    ) -> anyhow::Result<StoredCredential> {
        let files = self.files()?;
        let result = (|| {
            self.reload_locked(&files)?;
            let current_history = self.guest_credential_history()?;
            if current_history.removal_generation != previous_history.removal_generation
                || !previous_history
                    .keys
                    .iter()
                    .all(|key| current_history.keys.contains(key))
            {
                anyhow::bail!("guest credential history removed or changed keys");
            }
            let data = self
                .data
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("guest credential history missing"))?;
            if let Some(existing) = data
                .credentials
                .iter()
                .find(|item| item.credential_id == candidate.credential_id)
            {
                if existing.public_key != candidate.public_key
                    || existing.rp_id != candidate.rp_id
                    || existing.sign_count < candidate.sign_count
                {
                    anyhow::bail!("guest credential conflicts");
                }
                files.sync()?;
                return Ok(existing.clone());
            }
            self.add_credential(candidate.clone());
            self.save_locked(&files)?;
            Ok(candidate.clone())
        })();
        if result.is_err() {
            let _ = self.reload_locked(&files);
        }
        result
    }

    /// Return the device key as a hex string (for passing to providers).
    pub fn device_key_hex(&self) -> String {
        hex::encode(self.device_key.as_ref())
    }

    /// Access the underlying identity data (if loaded).
    pub fn data(&self) -> Option<&IdentityData> {
        self.data.as_ref()
    }

    /// Check if a user is registered
    pub fn is_registered(&self) -> bool {
        self.data
            .as_ref()
            .map(|d| !d.credentials.is_empty())
            .unwrap_or(false)
    }

    /// Get the user ID (if registered)
    pub fn user_id(&self) -> Option<&str> {
        self.data.as_ref().map(|d| d.user_id.as_str())
    }

    /// Get all credentials
    pub fn get_credentials(&self) -> Vec<StoredCredential> {
        self.data
            .as_ref()
            .map(|d| d.credentials.clone())
            .unwrap_or_default()
    }

    /// Add a credential and set user ID
    pub fn add_credential(&mut self, credential: StoredCredential) -> String {
        let user_id = generate_user_id(&credential.credential_id);

        if let Some(ref mut data) = self.data {
            data.credentials.push(credential);
        } else {
            self.data = Some(IdentityData {
                user_id: user_id.clone(),
                credentials: vec![credential],
                removal_generation: 0,
            });
        }

        // Safe: we just set self.data above in both branches
        self.data
            .as_ref()
            .expect("data was just set")
            .user_id
            .clone()
    }

    /// Update sign count for a credential
    pub fn update_sign_count(&mut self, credential_id: &str, new_count: u32) {
        if let Some(ref mut data) = self.data {
            for cred in &mut data.credentials {
                if cred.credential_id == credential_id {
                    cred.sign_count = new_count;
                }
            }
        }
    }

    /// Remove a passkey credential by ID.
    pub fn remove_credential(&mut self, credential_id: &str) -> bool {
        let Some(ref mut data) = self.data else {
            return false;
        };
        let before = data.credentials.len();
        data.credentials
            .retain(|cred| cred.credential_id != credential_id);
        let removed = data.credentials.len() != before;
        if removed {
            data.removal_generation = data.removal_generation.saturating_add(1);
        }
        removed
    }
}

fn validate_credentials(data: &IdentityData) -> anyhow::Result<()> {
    let mut ids = std::collections::HashSet::new();
    if data
        .credentials
        .iter()
        .any(|credential| !ids.insert(&credential.credential_id))
    {
        anyhow::bail!("identity store contains duplicate credential IDs");
    }
    Ok(())
}

/// Generate a deterministic user ID from a credential ID
fn generate_user_id(credential_id: &str) -> String {
    let hash = Sha256::digest(credential_id.as_bytes());
    format!("identity_{}", hex::encode(&hash[..16]))
}

#[cfg(test)]
mod tests {
    use curve25519_dalek::{constants, edwards::CompressedEdwardsY};

    use super::*;

    fn alias_pair() -> ([u8; 32], [u8; 32]) {
        let canonical = [
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let noncanonical = [
            0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0x7f,
        ];
        (canonical, noncanonical)
    }

    fn weak_public_key_bytes() -> [u8; 32] {
        [
            236, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127,
        ]
    }

    fn non_torsion_free_public_key_bytes() -> [u8; 32] {
        (constants::ED25519_BASEPOINT_POINT + constants::EIGHT_TORSION[1])
            .compress()
            .to_bytes()
    }

    fn did_for_key_bytes(bytes: [u8; 32]) -> String {
        let mut encoded = MULTICODEC_ED25519_PUB.to_vec();
        encoded.extend_from_slice(&bytes);
        format!("did:key:z{}", bs58::encode(encoded).into_string())
    }

    #[test]
    fn test_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        store.load().unwrap();

        assert!(!store.is_registered());
        assert!(store.user_id().is_none());
        assert!(store.get_credentials().is_empty());

        let cred = StoredCredential {
            credential_id: "dGVzdC1jcmVk".to_string(),
            public_key: "dGVzdC1rZXk".to_string(),
            sign_count: 0,
            rp_id: "localhost".to_string(),
        };
        let user_id = store.add_credential(cred);
        assert!(user_id.starts_with("identity_"));
        store.save().unwrap();

        let mut store2 = IdentityStore::new(dir.path()).unwrap();
        store2.load().unwrap();
        assert!(store2.is_registered());
        assert_eq!(store2.user_id(), Some(user_id.as_str()));
        assert_eq!(store2.get_credentials().len(), 1);
    }

    fn persistence_credential(id: &str, sign_count: u32) -> StoredCredential {
        StoredCredential {
            credential_id: id.to_string(),
            public_key: "public-key".to_string(),
            sign_count,
            rp_id: "localhost".to_string(),
        }
    }

    #[test]
    fn competing_credential_saves_preserve_the_winner_and_reload_the_loser() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = IdentityStore::new(dir.path()).unwrap();
        let mut second = IdentityStore::new(dir.path()).unwrap();
        first.load().unwrap();
        second.load().unwrap();
        first.add_credential(persistence_credential("first", 0));
        second.add_credential(persistence_credential("second", 0));
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                let saved = first.save().is_ok();
                (saved, first.get_credentials())
            });
            let b = scope.spawn(|| {
                barrier.wait();
                let saved = second.save().is_ok();
                (saved, second.get_credentials())
            });
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.0).count(), 1);
        let mut disk = IdentityStore::new(dir.path()).unwrap();
        disk.load().unwrap();
        let winner = disk.get_credentials();
        assert_eq!(winner.len(), 1);
        for (_, credentials) in results {
            assert_eq!(credentials[0].credential_id, winner[0].credential_id);
        }
    }

    #[test]
    fn guest_history_allows_counter_advance_and_unrelated_addition() {
        let dir = tempfile::tempdir().unwrap();
        let mut current = IdentityStore::new(dir.path()).unwrap();
        current.add_credential(persistence_credential("admin", 1));
        current.save().unwrap();
        let history = current.guest_credential_history().unwrap();
        let mut pending = IdentityStore::new(dir.path()).unwrap();
        pending.load().unwrap();
        current.update_sign_count("admin", 3);
        current.add_credential(persistence_credential("other-guest", 0));
        current.save().unwrap();
        let guest = persistence_credential("guest", 0);
        pending.persist_guest_candidate(&guest, &history).unwrap();
        assert_eq!(pending.get_credentials().len(), 3);
        assert_eq!(pending.get_credentials()[0].sign_count, 3);
        let bytes = std::fs::read(&pending.path).unwrap();
        pending.persist_guest_candidate(&guest, &history).unwrap();
        assert_eq!(std::fs::read(&pending.path).unwrap(), bytes);
    }

    #[test]
    fn guest_history_rejects_removed_or_conflicting_keys_without_writes() {
        for change in [
            "remove-admin",
            "change-admin",
            "remove-guest",
            "conflict-guest",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = IdentityStore::new(dir.path()).unwrap();
            store.add_credential(persistence_credential("admin", 1));
            store.save().unwrap();
            let history = store.guest_credential_history().unwrap();
            let guest = persistence_credential("guest", 0);
            match change {
                "remove-admin" => {
                    assert!(store.remove_credential("admin"));
                }
                "change-admin" => {
                    store.data.as_mut().unwrap().credentials[0].public_key = "changed".into()
                }
                "remove-guest" => {
                    store.persist_guest_candidate(&guest, &history).unwrap();
                    assert!(store.remove_credential("guest"));
                }
                "conflict-guest" => {
                    let mut conflict = guest.clone();
                    conflict.public_key = "changed".into();
                    store.add_credential(conflict);
                }
                _ => unreachable!(),
            }
            store.save().unwrap();
            let bytes = std::fs::read(&store.path).unwrap();
            assert!(
                store.persist_guest_candidate(&guest, &history).is_err(),
                "{change}"
            );
            assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
        }
    }

    #[test]
    fn stale_credential_save_cannot_lower_counter_or_resurrect_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let mut current = IdentityStore::new(dir.path()).unwrap();
        current.add_credential(persistence_credential("credential", 1));
        current.save().unwrap();
        let mut stale = IdentityStore::new(dir.path()).unwrap();
        stale.load().unwrap();
        current.update_sign_count("credential", 3);
        current.save().unwrap();
        let bytes = std::fs::read(&current.path).unwrap();
        stale.update_sign_count("credential", 2);
        assert!(stale.save().is_err());
        assert_eq!(std::fs::read(&current.path).unwrap(), bytes);
        assert_eq!(stale.get_credentials()[0].sign_count, 3);
        current.remove_credential("credential");
        current.save().unwrap();
        stale.update_sign_count("credential", 4);
        assert!(stale.save().is_err());
        assert!(stale.get_credentials().is_empty());
    }

    #[test]
    fn existing_did_reads_current_identity_without_creating_missing_state() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(load_existing_did(&missing).is_err());
        assert!(!missing.exists());
        assert!(load_existing_did(dir.path()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        let (_, expected) = load_or_create_did(dir.path()).unwrap();
        let key = std::fs::read(device_key_path(dir.path())).unwrap();
        assert_eq!(load_existing_did(dir.path()).unwrap().1, expected);
        std::fs::remove_file(device_key_path(dir.path())).unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        assert!(!device_key_path(dir.path()).exists());
        let files = IdentityFiles::open(dir.path()).unwrap();
        files.replace(c"device.key", &key, None).unwrap();
        drop(files);
        std::fs::remove_file(dir.path().join("identity/identity.lock")).unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        assert!(!dir.path().join("identity/identity.lock").exists());
        assert_eq!(std::fs::read(device_key_path(dir.path())).unwrap(), key);
    }

    #[test]
    fn existing_did_revalidates_key_and_credentials_on_every_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        store.add_credential(persistence_credential("credential", 0));
        store.save().unwrap();
        let expected = load_existing_did(dir.path()).unwrap().1;
        let key = std::fs::read(device_key_path(dir.path())).unwrap();
        let credentials = std::fs::read(&store.path).unwrap();
        for invalid in [vec![0; 32], vec![0; 33], vec![]] {
            std::fs::write(device_key_path(dir.path()), &invalid).unwrap();
            assert!(load_existing_did(dir.path()).is_err());
            assert_eq!(std::fs::read(device_key_path(dir.path())).unwrap(), invalid);
            assert_eq!(std::fs::read(&store.path).unwrap(), credentials);
        }
        std::fs::write(device_key_path(dir.path()), &key).unwrap();
        assert_eq!(load_existing_did(dir.path()).unwrap().1, expected);
        std::fs::write(&store.path, b"corrupt").unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        assert_eq!(std::fs::read(&store.path).unwrap(), b"corrupt");
        assert_eq!(std::fs::read(device_key_path(dir.path())).unwrap(), key);
    }

    #[cfg(unix)]
    #[test]
    fn existing_did_preserves_private_descriptor_checks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        load_or_create_did(dir.path()).unwrap();
        let key = device_key_path(dir.path());
        let bytes = std::fs::read(&key).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = dir.path().join("key-alias");
        std::fs::hard_link(&key, &alias).unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        std::fs::remove_file(&key).unwrap();
        symlink(&alias, &key).unwrap();
        assert!(load_existing_did(dir.path()).is_err());
        assert_eq!(std::fs::read(&alias).unwrap(), bytes);
    }

    #[test]
    fn missing_device_key_with_credentials_fails_without_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        store.add_credential(persistence_credential("credential", 0));
        store.save().unwrap();
        let bytes = std::fs::read(&store.path).unwrap();
        std::fs::remove_file(device_key_path(dir.path())).unwrap();
        assert!(IdentityStore::new(dir.path()).is_err());
        assert!(!device_key_path(dir.path()).exists());
        assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
    }

    #[test]
    fn credential_insert_failure_reloads_old_or_new_state() {
        for fault in [
            files::SaveFault::BeforeReplace,
            files::SaveFault::AfterReplace,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = IdentityStore::new(dir.path()).unwrap();
            let old = persistence_credential("old", 0);
            let new = persistence_credential("new", 0);
            store.add_credential(old.clone());
            store.save().unwrap();
            let bytes = std::fs::read(&store.path).unwrap();
            store.add_credential(new.clone());
            store.save_fault = Some(fault);
            let error = store.save().unwrap_err();
            if fault == files::SaveFault::BeforeReplace {
                assert!(error.to_string().contains("pre-replacement"));
                assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
                assert_eq!(store.get_credentials(), vec![old]);
            } else {
                assert!(error.to_string().contains("indeterminate"));
                assert_ne!(std::fs::read(&store.path).unwrap(), bytes);
                assert_eq!(store.get_credentials(), vec![old, new]);
            }
            assert!(std::fs::read_dir(dir.path().join("identity"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")));
            let mut disk = IdentityStore::new(dir.path()).unwrap();
            disk.load().unwrap();
            assert_eq!(store.get_credentials(), disk.get_credentials());
        }
    }

    #[test]
    fn competing_fresh_identity_managers_share_one_device_key() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        let keys = std::thread::scope(|scope| {
            let workers = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        load_or_create_device_key(dir.path()).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(keys.iter().all(|key| **key == *keys[0]));
        let bytes = std::fs::read(device_key_path(dir.path())).unwrap();
        assert_eq!(bytes.as_slice(), keys[0].as_ref());
        let (_, did) = load_or_create_did(dir.path()).unwrap();
        assert_eq!(did, derive_did(&keys[0]).1);
        assert_eq!(load_nickname(dir.path()).unwrap(), None);
        assert_eq!(std::fs::read(device_key_path(dir.path())).unwrap(), bytes);
    }

    #[test]
    fn corrupt_or_removed_key_clears_cached_credentials_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        store.add_credential(persistence_credential("old", 0));
        store.save().unwrap();
        let bytes = std::fs::read(&store.path).unwrap();
        std::fs::write(device_key_path(dir.path()), b"corrupt").unwrap();
        assert!(IdentityStore::new(dir.path()).is_err());
        store.add_credential(persistence_credential("unsaved", 0));
        assert!(store.save().is_err());
        assert!(store.get_credentials().is_empty());
        assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(device_key_path(dir.path())).unwrap(),
            b"corrupt"
        );
        std::fs::write(device_key_path(dir.path()), [0; 32]).unwrap();
        assert!(load_or_create_device_key(dir.path()).is_err());
        assert_eq!(std::fs::read(device_key_path(dir.path())).unwrap(), [0; 32]);
        assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
    }

    #[test]
    fn failed_counter_save_reloads_durable_counter_before_retry() {
        for fault in [
            files::SaveFault::BeforeReplace,
            files::SaveFault::AfterReplace,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = IdentityStore::new(dir.path()).unwrap();
            store.add_credential(persistence_credential("credential", 1));
            store.save().unwrap();
            let old = std::fs::read(&store.path).unwrap();
            store.update_sign_count("credential", 2);
            store.save_fault = Some(fault);
            assert!(store.save().is_err());
            let expected = if fault == files::SaveFault::BeforeReplace {
                1
            } else {
                2
            };
            assert_eq!(store.get_credentials()[0].sign_count, expected);
            if expected == 1 {
                assert_eq!(std::fs::read(&store.path).unwrap(), old);
            }
            let mut later = IdentityStore::new(dir.path()).unwrap();
            later.load().unwrap();
            later.update_sign_count("credential", 3);
            later.save().unwrap();
            let latest = std::fs::read(&store.path).unwrap();
            assert!(store.save().is_err());
            assert_eq!(store.get_credentials()[0].sign_count, 3);
            assert_eq!(std::fs::read(&store.path).unwrap(), latest);
        }
    }

    #[test]
    fn duplicate_credential_save_preserves_bytes_and_reloads_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        let old = persistence_credential("credential", 0);
        store.add_credential(old.clone());
        store.save().unwrap();
        let bytes = std::fs::read(&store.path).unwrap();
        store.add_credential(persistence_credential("credential", 1));
        assert!(store.save().is_err());
        assert_eq!(store.get_credentials(), vec![old]);
        assert_eq!(std::fs::read(&store.path).unwrap(), bytes);
    }

    #[test]
    fn test_user_id_deterministic() {
        let id1 = generate_user_id("test-cred-id");
        let id2 = generate_user_id("test-cred-id");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("identity_"));
    }

    #[test]
    fn test_encrypted_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();

        let cred = StoredCredential {
            credential_id: "enc-test-cred".to_string(),
            public_key: "enc-test-key".to_string(),
            sign_count: 5,
            rp_id: "localhost".to_string(),
        };
        store.add_credential(cred);
        store.save().unwrap();

        // Verify raw file contains "ciphertext" (encrypted), not "user_id" (plaintext)
        let raw =
            std::fs::read_to_string(dir.path().join("identity").join("credentials.json")).unwrap();
        assert!(raw.contains("ciphertext"), "file should be encrypted");
        assert!(
            !raw.contains("user_id"),
            "file should not contain plaintext fields"
        );

        // Reload and verify data is intact
        let mut store2 = IdentityStore::new(dir.path()).unwrap();
        store2.load().unwrap();
        assert!(store2.is_registered());
        assert_eq!(store2.get_credentials().len(), 1);
        assert_eq!(store2.get_credentials()[0].sign_count, 5);
    }

    #[test]
    fn test_device_key_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let key1 = load_or_create_device_key(dir.path()).unwrap();
        let key2 = load_or_create_device_key(dir.path()).unwrap();
        assert_eq!(*key1, *key2, "device key should be stable across calls");
        assert_ne!(*key1, [0u8; 32], "device key should not be all zeros");
    }

    #[test]
    fn test_remove_credential() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = IdentityStore::new(dir.path()).unwrap();
        store.load().unwrap();

        let cred = StoredCredential {
            credential_id: "remove-test-cred".to_string(),
            public_key: "remove-test-key".to_string(),
            sign_count: 0,
            rp_id: "localhost".to_string(),
        };
        store.add_credential(cred);

        assert!(store.remove_credential("remove-test-cred"));
        assert!(!store.is_registered());
        assert!(!store.remove_credential("remove-test-cred"));
    }

    #[test]
    fn test_load_or_create_did_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let (sk1, did1) = load_or_create_did(dir.path()).unwrap();
        let (sk2, did2) = load_or_create_did(dir.path()).unwrap();
        assert_eq!(
            sk1.to_bytes(),
            sk2.to_bytes(),
            "same device_key must produce same signing key"
        );
        assert_eq!(did1, did2, "same device_key must produce same DID");
    }

    #[test]
    fn test_nickname_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        save_nickname(dir.path(), "alice").unwrap();
        assert_eq!(load_nickname(dir.path()).unwrap().as_deref(), Some("alice"));
    }

    #[test]
    fn test_did_format() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, did) = load_or_create_did(dir.path()).unwrap();
        assert!(
            did.starts_with("did:key:z6Mk"),
            "DID must start with did:key:z6Mk, got: {}",
            did
        );
        assert_eq!(decode_did_key(&did).unwrap(), sk.verifying_key());
    }

    #[test]
    fn did_decode_rejects_invalid_and_noncanonical_profile_text() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let did = encode_signing_key_did(&key);
        assert!(decode_did_key("did:example:alice").is_err());
        assert!(decode_did_key(&did.to_ascii_uppercase()).is_err());
        assert!(decode_did_key(&(did.clone() + "x")).is_err());
        assert!(decode_did_key(&did[..did.len() - 1]).is_err());

        let mut wrong_codec = vec![0xec, 0x01];
        wrong_codec.extend_from_slice(key.verifying_key().as_bytes());
        let wrong_codec = format!("did:key:z{}", bs58::encode(wrong_codec).into_string());
        assert!(decode_did_key(&wrong_codec).is_err());

        let (canonical_alias, noncanonical_alias) = alias_pair();
        assert!(decode_did_key(&did_for_key_bytes(canonical_alias)).is_err());
        assert!(decode_did_key(&did_for_key_bytes(noncanonical_alias)).is_err());
        assert!(decode_did_key(&did_for_key_bytes(weak_public_key_bytes())).is_err());
        assert!(decode_did_key(&did_for_key_bytes(non_torsion_free_public_key_bytes())).is_err());
    }

    #[test]
    fn canonical_ed25519_validator_accepts_generated_keys_and_rejects_aliases() {
        let generated = ed25519_dalek::SigningKey::from_bytes(&[7; 32]).verifying_key();
        assert_eq!(
            validate_canonical_ed25519_verifying_key_bytes(generated.to_bytes()).unwrap(),
            generated
        );

        let (canonical_alias, noncanonical_alias) = alias_pair();
        let canonical_point = CompressedEdwardsY(canonical_alias).decompress().unwrap();
        let alias_point = CompressedEdwardsY(noncanonical_alias).decompress().unwrap();
        assert_eq!(canonical_point, alias_point);
        assert_eq!(canonical_point.compress().to_bytes(), canonical_alias);
        assert!(validate_canonical_ed25519_verifying_key_bytes(canonical_alias).is_err());
        assert!(validate_canonical_ed25519_verifying_key_bytes(noncanonical_alias).is_err());
    }

    #[test]
    fn canonical_ed25519_validator_rejects_weak_and_non_torsion_free_points() {
        let weak = CompressedEdwardsY(weak_public_key_bytes())
            .decompress()
            .unwrap();
        assert!(weak.is_small_order());
        assert!(!weak.is_torsion_free());
        assert!(validate_canonical_ed25519_verifying_key_bytes(weak_public_key_bytes()).is_err());

        let non_torsion_free = CompressedEdwardsY(non_torsion_free_public_key_bytes())
            .decompress()
            .unwrap();
        assert!(!non_torsion_free.is_small_order());
        assert!(!non_torsion_free.is_torsion_free());
        assert!(
            validate_canonical_ed25519_verifying_key_bytes(non_torsion_free_public_key_bytes())
                .is_err()
        );
    }

    #[test]
    fn encode_did_key_rejects_noncanonical_and_weak_reconstructed_verifying_keys() {
        let (canonical_alias, noncanonical_alias) = alias_pair();
        let canonical_alias_key = ed25519_dalek::VerifyingKey::from_bytes(&canonical_alias)
            .expect("dalek accepts alias bytes as a verifying key");
        let noncanonical_alias_key = ed25519_dalek::VerifyingKey::from_bytes(&noncanonical_alias)
            .expect("dalek accepts alias bytes as a verifying key");
        let weak_key = ed25519_dalek::VerifyingKey::from_bytes(&weak_public_key_bytes())
            .expect("dalek accepts weak-order bytes as a verifying key");
        let non_torsion_free_key =
            ed25519_dalek::VerifyingKey::from_bytes(&non_torsion_free_public_key_bytes())
                .expect("dalek accepts non-torsion-free bytes as a verifying key");

        assert!(encode_did_key(&canonical_alias_key).is_err());
        assert!(encode_did_key(&noncanonical_alias_key).is_err());
        assert!(encode_did_key(&weak_key).is_err());
        assert!(encode_did_key(&non_torsion_free_key).is_err());
    }

    #[test]
    fn test_derive_did_deterministic() {
        let key = [42u8; 32];
        let (sk1, did1) = derive_did(&key);
        let (sk2, did2) = derive_did(&key);
        assert_eq!(sk1.to_bytes(), sk2.to_bytes());
        assert_eq!(did1, did2);
        assert!(did1.starts_with("did:key:z6Mk"));
    }
}
