//! Signature verification for capsules
use std::path::Path;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use elastos_common::{CapsuleManifest, ElastosError, Result};

/// Re-export for use by CLI
pub use ed25519_dalek::SigningKey;

/// Verifies capsule signatures
pub struct SignatureVerifier {
    /// Trusted public keys for verification
    trusted_keys: Vec<VerifyingKey>,
}

impl SignatureVerifier {
    /// Create a new signature verifier with no trusted keys
    pub fn new() -> Self {
        Self {
            trusted_keys: Vec::new(),
        }
    }

    /// Add a trusted public key
    pub fn add_trusted_key(&mut self, key: VerifyingKey) {
        if !self.trusted_keys.iter().any(|k| k == &key) {
            self.trusted_keys.push(key);
        }
    }

    /// Add a trusted key from hex-encoded bytes
    pub fn add_trusted_key_hex(&mut self, hex_key: &str) -> Result<()> {
        let bytes = hex::decode(hex_key.trim())
            .map_err(|e| ElastosError::InvalidManifest(format!("Invalid hex key: {}", e)))?;

        let key_bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ElastosError::InvalidManifest("Key must be 32 bytes".into()))?;

        let key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|e| ElastosError::InvalidManifest(format!("Invalid public key: {}", e)))?;

        self.add_trusted_key(key);
        Ok(())
    }

    /// Load trusted keys from a file (one hex-encoded key per line)
    pub fn load_trusted_keys(&mut self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)?;
        let mut count = 0;

        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            self.add_trusted_key_hex(line)?;
            count += 1;
        }

        tracing::info!("Loaded {} trusted keys from {:?}", count, path);
        Ok(count)
    }

    /// Get the number of trusted keys
    pub fn trusted_key_count(&self) -> usize {
        self.trusted_keys.len()
    }

    /// Verify a capsule's signature
    ///
    /// The signature covers: SHA256(manifest_json_without_signature) || SHA256(content)
    pub fn verify_capsule(&self, manifest: &CapsuleManifest, content_hash: &[u8]) -> Result<bool> {
        let signature_b64 = manifest
            .signature
            .as_ref()
            .ok_or_else(|| ElastosError::InvalidManifest("Missing signature".into()))?;

        let sig_bytes = BASE64.decode(signature_b64).map_err(|e| {
            ElastosError::InvalidManifest(format!("Invalid signature encoding: {}", e))
        })?;

        let signature = Signature::from_slice(&sig_bytes)
            .map_err(|e| ElastosError::InvalidManifest(format!("Invalid signature: {}", e)))?;

        // Build the message that was signed
        let message = build_signing_message(manifest, content_hash)?;

        // Try each trusted key
        for key in &self.trusted_keys {
            if key.verify(&message, &signature).is_ok() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Check if verification is enabled (has trusted keys)
    pub fn is_enabled(&self) -> bool {
        !self.trusted_keys.is_empty()
    }
}

impl Default for SignatureVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the message to sign/verify
///
/// Format: SHA256(manifest_json_without_signature) || content_hash
fn build_signing_message(manifest: &CapsuleManifest, content_hash: &[u8]) -> Result<Vec<u8>> {
    // Create a copy of manifest without the signature for hashing
    let mut manifest_for_hash = manifest.clone();
    manifest_for_hash.signature = None;

    let manifest_json = serde_json::to_string(&manifest_for_hash).map_err(|e| {
        ElastosError::InvalidManifest(format!("Failed to serialize manifest: {}", e))
    })?;

    let manifest_hash = Sha256::digest(manifest_json.as_bytes());

    let mut message = Vec::with_capacity(64);
    message.extend_from_slice(&manifest_hash);
    message.extend_from_slice(content_hash);

    Ok(message)
}

/// Sign a capsule manifest and content
pub fn sign_capsule(
    signing_key: &SigningKey,
    manifest: &mut CapsuleManifest,
    content_hash: &[u8],
) -> Result<()> {
    // Clear any existing signature before signing
    manifest.signature = None;

    let message = build_signing_message(manifest, content_hash)?;
    let signature = signing_key.sign(&message);

    manifest.signature = Some(BASE64.encode(signature.to_bytes()));

    Ok(())
}

/// Generate a new signing keypair
pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let signing_key = SigningKey::generate(&mut rand::thread_rng());
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key)
}

/// Hash content using SHA-256
pub fn hash_content(content: &[u8]) -> Vec<u8> {
    Sha256::digest(content).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleType, Permissions, ResourceLimits};

    fn create_test_manifest() -> CapsuleManifest {
        CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "test-capsule".into(),
            description: Some("Test".into()),
            author: Some("Test Author".into()),
            capsule_type: CapsuleType::Wasm,
            entrypoint: "main.wasm".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    #[test]
    fn test_generate_keypair() {
        let (signing_key, verifying_key) = generate_keypair();
        assert_eq!(signing_key.verifying_key(), verifying_key);
    }

    #[test]
    fn test_sign_and_verify() {
        let (signing_key, verifying_key) = generate_keypair();

        let mut manifest = create_test_manifest();
        let content = b"test content";
        let content_hash = hash_content(content);

        // Sign
        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();
        assert!(manifest.signature.is_some());

        // Verify
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);

        let result = verifier.verify_capsule(&manifest, &content_hash).unwrap();
        assert!(result);
    }

    #[test]
    fn test_verify_with_wrong_key() {
        let (signing_key, _) = generate_keypair();
        let (_, wrong_verifying_key) = generate_keypair();

        let mut manifest = create_test_manifest();
        let content = b"test content";
        let content_hash = hash_content(content);

        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(wrong_verifying_key);

        let result = verifier.verify_capsule(&manifest, &content_hash).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_verify_tampered_content() {
        let (signing_key, verifying_key) = generate_keypair();

        let mut manifest = create_test_manifest();
        let content = b"original content";
        let content_hash = hash_content(content);

        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        // Try to verify with different content
        let tampered_hash = hash_content(b"tampered content");

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);

        let result = verifier.verify_capsule(&manifest, &tampered_hash).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_verify_tampered_manifest() {
        let (signing_key, verifying_key) = generate_keypair();

        let mut manifest = create_test_manifest();
        let content = b"test content";
        let content_hash = hash_content(content);

        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        // Tamper with manifest
        manifest.name = "tampered-name".into();

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);

        let result = verifier.verify_capsule(&manifest, &content_hash).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_add_trusted_key_hex() {
        let (_, verifying_key) = generate_keypair();
        let hex_key = hex::encode(verifying_key.as_bytes());

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key_hex(&hex_key).unwrap();

        assert_eq!(verifier.trusted_key_count(), 1);
    }

    #[test]
    fn test_verify_without_trusted_keys() {
        let (signing_key, _) = generate_keypair();

        let mut manifest = create_test_manifest();
        let content_hash = hash_content(b"test");

        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        let verifier = SignatureVerifier::new();
        let result = verifier.verify_capsule(&manifest, &content_hash).unwrap();
        assert!(!result); // No trusted keys, so verification fails
    }

    #[test]
    fn test_verify_missing_signature() {
        let manifest = create_test_manifest(); // No signature

        let verifier = SignatureVerifier::new();
        let result = verifier.verify_capsule(&manifest, &[]);

        assert!(result.is_err());
    }
}
