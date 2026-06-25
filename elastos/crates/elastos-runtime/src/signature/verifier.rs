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

    /// Verify a capsule's signature, returning the matched trusted key.
    ///
    /// Returns `Ok(Some(key))` when a trusted key's real ed25519 check passes (so
    /// the caller learns WHICH trusted key signed — the only honest signer
    /// evidence the keyset carries; there is no DID in the manifest or keyset),
    /// `Ok(None)` when no trusted key matches (including when none are
    /// configured), and `Err` on a structurally invalid/missing signature.
    ///
    /// The signature covers: SHA256(manifest_json_without_signature) || SHA256(content)
    pub fn verify_capsule_signer(
        &self,
        manifest: &CapsuleManifest,
        content_hash: &[u8],
    ) -> Result<Option<VerifyingKey>> {
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

        // Try each trusted key; return the one that verifies.
        for key in &self.trusted_keys {
            if key.verify(&message, &signature).is_ok() {
                return Ok(Some(*key));
            }
        }

        Ok(None)
    }

    /// Verify a capsule's signature (boolean). Thin wrapper over
    /// [`SignatureVerifier::verify_capsule_signer`] so there is one canonical
    /// verify path.
    pub fn verify_capsule(&self, manifest: &CapsuleManifest, content_hash: &[u8]) -> Result<bool> {
        Ok(self
            .verify_capsule_signer(manifest, content_hash)?
            .is_some())
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

    // Canonicalize the signed form (AUD-1): round-trip through serde_json::Value so
    // EVERY object key serializes in deterministic sorted order. The manifest's
    // `providers` is a HashMap whose iteration order is process-random, so a direct
    // to_string would sign in one process and FALSE-FAIL verification in another for
    // any capsule with >=2 providers. serde_json::Value's object map is a sorted
    // BTreeMap (preserve_order is off in this workspace), so to_value -> to_string is
    // a stable canonical form used identically by sign and verify.
    let manifest_value = serde_json::to_value(&manifest_for_hash).map_err(|e| {
        ElastosError::InvalidManifest(format!("Failed to serialize manifest: {}", e))
    })?;
    let manifest_json = serde_json::to_string(&manifest_value).map_err(|e| {
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

/// A short, non-secret fingerprint of a trusted public key: the first 16 hex
/// chars of SHA-256(pubkey). This is the honest "verified signer" identity the
/// ed25519 keyset can yield — never the self-asserted manifest author, never the
/// raw signature bytes.
pub fn key_fingerprint(key: &VerifyingKey) -> String {
    Sha256::digest(key.to_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
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
            role: elastos_common::CapsuleRole::App,
            capsule_type: CapsuleType::Wasm,
            entrypoint: "main.wasm".into(),
            requires: Vec::new(),
            provides: None,
            authority: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    #[test]
    fn signed_manifest_with_multiple_providers_verifies_across_serialization() {
        // AUD-1 determinism: `providers` is a HashMap (process-random iteration order).
        // The signed form is canonicalized (sorted keys), so a manifest signed in one
        // process verifies after a serialize -> deserialize round-trip (the real
        // sign-in-trust_cmd / verify-at-launch boundary), even with many providers.
        let (signing_key, verifying_key) = generate_keypair();
        let mut manifest = create_test_manifest();
        let mut providers = std::collections::HashMap::new();
        for k in ["zeta", "alpha", "mike", "bravo", "yankee", "delta"] {
            providers.insert(k.to_string(), format!("vm-{k}"));
        }
        manifest.providers = Some(providers);
        let content_hash = hash_content(b"entrypoint bytes");
        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        // Cross-process boundary: serialize the signed manifest and deserialize a FRESH
        // copy (a new HashMap whose iteration order may differ from the signer's).
        let json = serde_json::to_string(&manifest).unwrap();
        let reparsed: CapsuleManifest = serde_json::from_str(&json).unwrap();

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);
        assert_eq!(
            verifier
                .verify_capsule_signer(&reparsed, &content_hash)
                .unwrap(),
            Some(verifying_key),
            "a multi-provider signed manifest must verify after a serialize/deserialize round-trip"
        );
    }

    // ── Flint G2 (loop 3a): verified-signer capability ───────────────

    #[test]
    fn verify_capsule_signer_returns_the_matched_key() {
        let (signing_key, verifying_key) = generate_keypair();
        let mut manifest = create_test_manifest();
        let content_hash = hash_content(b"hello world");
        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);

        // A real ed25519 check resolves the exact trusted key that signed.
        let signer = verifier
            .verify_capsule_signer(&manifest, &content_hash)
            .unwrap();
        assert_eq!(
            signer,
            Some(verifying_key),
            "must return the matched trusted key"
        );

        // The fingerprint is a stable, 16-hex, non-secret signer identity.
        let fp = key_fingerprint(&verifying_key);
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fp, key_fingerprint(&verifying_key), "fingerprint is stable");
    }

    #[test]
    fn verify_capsule_bool_wrapper_matches_signer_across_cases() {
        // One canonical verify path: the bool wrapper must agree with the signer
        // method on every case, and "Some/true" must come from a real check.
        let (signing_key, verifying_key) = generate_keypair();
        let (_other_sk, other_vk) = generate_keypair();
        let mut manifest = create_test_manifest();
        let content_hash = hash_content(b"payload");
        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        let mut trusted = SignatureVerifier::new();
        trusted.add_trusted_key(verifying_key);
        // Trusted key present -> true / Some.
        assert!(trusted.verify_capsule(&manifest, &content_hash).unwrap());
        assert_eq!(
            trusted.verify_capsule(&manifest, &content_hash).unwrap(),
            trusted
                .verify_capsule_signer(&manifest, &content_hash)
                .unwrap()
                .is_some()
        );

        // Untrusted key -> false / None (verification, not presence).
        let mut wrong = SignatureVerifier::new();
        wrong.add_trusted_key(other_vk);
        assert!(!wrong.verify_capsule(&manifest, &content_hash).unwrap());
        assert!(wrong
            .verify_capsule_signer(&manifest, &content_hash)
            .unwrap()
            .is_none());

        // No trusted keys (dev bypass surface) -> false / None.
        let empty = SignatureVerifier::new();
        assert!(!empty.verify_capsule(&manifest, &content_hash).unwrap());
        assert!(empty
            .verify_capsule_signer(&manifest, &content_hash)
            .unwrap()
            .is_none());

        // Tampered content -> false / None (signature is domain-bound).
        let tampered = hash_content(b"different");
        assert!(!trusted.verify_capsule(&manifest, &tampered).unwrap());
        assert!(trusted
            .verify_capsule_signer(&manifest, &tampered)
            .unwrap()
            .is_none());

        // Missing signature -> both Err (fail closed identically).
        let unsigned = create_test_manifest();
        assert!(trusted.verify_capsule(&unsigned, &content_hash).is_err());
        assert!(trusted
            .verify_capsule_signer(&unsigned, &content_hash)
            .is_err());
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
