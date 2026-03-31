//! Credential persistence for passkey identity
//!
//! Stores WebAuthn credentials as encrypted JSON on disk using AES-256-GCM.
//! Single user per device. A shared device key is auto-generated on first run.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// A stored passkey credential
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Multicodec prefix for Ed25519 public keys.
pub const MULTICODEC_ED25519_PUB: [u8; 2] = [0xed, 0x01];

/// Encode an Ed25519 verifying key as `did:key:z6Mk...` (multicodec + base58).
pub fn encode_did_key(verifying_key: &ed25519_dalek::VerifyingKey) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&MULTICODEC_ED25519_PUB);
    bytes.extend_from_slice(verifying_key.as_bytes());
    format!("did:key:z{}", bs58::encode(&bytes).into_string())
}

/// Load the device key and derive a stable DID identity from it.
///
/// Returns `(SigningKey, did_string)`. The device_key file stays on disk
/// for backwards compatibility (encryption at rest), but identity is always
/// the derived DID.
///
/// Derivation: `SHA-256("elastos-did-v1" || device_key)` → Ed25519 SigningKey.
pub fn load_or_create_did(data_dir: &Path) -> anyhow::Result<(ed25519_dalek::SigningKey, String)> {
    let device_key = load_or_create_device_key(data_dir)?;
    let (signing_key, did) = derive_did(&device_key);
    Ok((signing_key, did))
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
    let did = encode_did_key(&signing_key.verifying_key());
    (signing_key, did)
}

/// Load or create the shared device key at `{data_dir}/identity/device.key`.
///
/// Returns 32 random bytes wrapped in `Zeroizing`. The key file is created
/// with 0600 permissions on Unix.
pub fn load_or_create_device_key(data_dir: &Path) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let key_dir = data_dir.join("identity");
    let key_path = key_dir.join("device.key");

    if key_path.exists() {
        let bytes = std::fs::read(&key_path)?;
        if bytes.len() != 32 {
            anyhow::bail!(
                "device.key has invalid length {} (expected 32)",
                bytes.len()
            );
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Zeroizing::new(key))
    } else {
        std::fs::create_dir_all(&key_dir)?;
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        std::fs::write(&key_path, key)?;

        // Set 0600 permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(Zeroizing::new(key))
    }
}

/// Encrypt plaintext bytes with AES-256-GCM, returning a JSON envelope.
fn encrypt_data(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("AES key init: {}", e))?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
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
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| anyhow::anyhow!("decryption failed: {}", e))
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
        })
    }

    /// Load credentials from disk (encrypted).
    pub fn load(&mut self) -> anyhow::Result<()> {
        if self.path.exists() {
            let raw = std::fs::read(&self.path)?;
            let plaintext = decrypt_data(&self.device_key, &raw)?;
            self.data = Some(serde_json::from_slice(&plaintext)?);
        }
        Ok(())
    }

    /// Save credentials to disk (encrypted).
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(ref data) = self.data {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_vec(data)?;
            let encrypted = encrypt_data(&self.device_key, &json)?;
            std::fs::write(&self.path, encrypted)?;
        }
        Ok(())
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
}

/// Generate a deterministic user ID from a credential ID
fn generate_user_id(credential_id: &str) -> String {
    let hash = Sha256::digest(credential_id.as_bytes());
    format!("identity_{}", hex::encode(&hash[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_did_format() {
        let dir = tempfile::tempdir().unwrap();
        let (_sk, did) = load_or_create_did(dir.path()).unwrap();
        assert!(
            did.starts_with("did:key:z6Mk"),
            "DID must start with did:key:z6Mk, got: {}",
            did
        );
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
