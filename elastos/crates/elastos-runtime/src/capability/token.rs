//! Capability token types and cryptographic operations
//!
//! CRITICAL: This structure is signed. Adding fields later breaks existing tokens.
//! Design for extensibility NOW.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use uuid::Uuid;

use crate::primitives::time::SecureTimestamp;

/// Unique identifier for a capability token
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenId(pub(crate) [u8; 16]);

impl TokenId {
    /// Generate a new random token ID
    pub fn new() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }

    /// Create from bytes
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Create from hex string
    pub fn from_hex(hex_str: &str) -> Result<Self, String> {
        let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex: {}", e))?;
        if bytes.len() != 16 {
            return Err(format!("Expected 16 bytes, got {}", bytes.len()));
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl Default for TokenId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Resource identifier (elastos://Qm123 or localhost://Users/self/Documents/photos/*)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId(pub(crate) String);

impl ResourceId {
    pub fn new(resource: impl Into<String>) -> Self {
        Self(resource.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Normalize a resource path: collapse repeated `/` and reject `.`/`..` segments
    /// in the path portion (after the `://` scheme).
    fn normalize(s: &str) -> String {
        // Find the path portion (after "://host" or "://")
        if let Some(scheme_end) = s.find("://") {
            let (scheme, rest) = s.split_at(scheme_end + 3);
            let normalized: Vec<&str> = rest
                .split('/')
                .filter(|seg| !seg.is_empty() && *seg != ".")
                .collect();
            format!("{}{}", scheme, normalized.join("/"))
        } else {
            s.to_string()
        }
    }

    /// Check if this resource ID matches a pattern
    /// Supports wildcards: localhost://Users/self/Documents/photos/* matches localhost://Users/self/Documents/photos/vacation.jpg
    /// Normalizes paths (collapses `//`, rejects `.`/`..` segments) before comparison
    pub fn matches(&self, pattern: &ResourceId) -> bool {
        let pattern_norm = Self::normalize(pattern.as_str());
        let resource_norm = Self::normalize(self.as_str());

        if pattern_norm.ends_with("/*") {
            let prefix = &pattern_norm[..pattern_norm.len() - 1];
            if !resource_norm.starts_with(prefix) {
                return false;
            }
            // Reject path traversal: the suffix after the prefix must not contain ".."
            let suffix = &resource_norm[prefix.len()..];
            !suffix.split('/').any(|seg| seg == "..")
        } else {
            resource_norm == pattern_norm
        }
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Actions that can be performed on a resource
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Write,
    Execute,
    Message,
    Delete,
    Admin,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Read => write!(f, "read"),
            Action::Write => write!(f, "write"),
            Action::Execute => write!(f, "execute"),
            Action::Message => write!(f, "message"),
            Action::Delete => write!(f, "delete"),
            Action::Admin => write!(f, "admin"),
        }
    }
}

/// Extensible constraints for capability tokens
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenConstraints {
    /// Revocation epoch - reject if < global epoch
    pub(crate) epoch: u64,

    /// Can this token be passed to another capsule?
    pub(crate) delegatable: bool,

    /// Ceiling for data classification (0-255, None = no restriction)
    pub(crate) max_classification: Option<u8>,

    /// Use-limited tokens (None = unlimited)
    pub(crate) max_uses: Option<u32>,
}

impl TokenConstraints {
    /// Create constraints with specific values
    pub fn new(
        epoch: u64,
        delegatable: bool,
        max_classification: Option<u8>,
        max_uses: Option<u32>,
    ) -> Self {
        Self {
            epoch,
            delegatable,
            max_classification,
            max_uses,
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn delegatable(&self) -> bool {
        self.delegatable
    }
    pub fn max_classification(&self) -> Option<u8> {
        self.max_classification
    }
    pub fn max_uses(&self) -> Option<u32> {
        self.max_uses
    }
}

/// Capability token - cryptographic proof of permission
///
/// NOTE: This structure is signed. Adding fields later breaks existing tokens.
/// Design for extensibility NOW.
///
/// Fields are `pub(crate)` — only the runtime can construct and mutate tokens.
/// External consumers use the read-only accessor methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    // === VERSIONING (future-proofing) ===
    /// Token format version (start at 1)
    pub(crate) version: u8,

    // === CORE IDENTITY ===
    /// Unique token identifier
    pub(crate) id: TokenId,

    /// Who can use this token (capsule ID)
    pub(crate) capsule: String,

    /// Ed25519 pubkey of issuer (for multi-key future)
    pub(crate) issuer: [u8; 32],

    // === PERMISSION ===
    /// What resource this grants access to
    pub(crate) resource: ResourceId,

    /// What action is allowed
    pub(crate) action: Action,

    // === CONSTRAINTS (extensible) ===
    pub(crate) constraints: TokenConstraints,

    // === TEMPORAL ===
    /// When created (audit + anti-backdating)
    pub(crate) issued_at: SecureTimestamp,

    /// When expires (None = until revoked)
    pub(crate) expiry: Option<SecureTimestamp>,

    // === SIGNATURE ===
    /// Ed25519 signature over all above fields
    #[serde(with = "signature_serde")]
    pub(crate) signature: [u8; 64],
}

mod signature_serde {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(sig: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BASE64.encode(sig).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = BASE64.decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }
}

impl CapabilityToken {
    /// Current token format version
    pub const CURRENT_VERSION: u8 = 1;

    /// Create a new unsigned token (signature will be zeroed)
    pub fn new(
        capsule: String,
        issuer: [u8; 32],
        resource: ResourceId,
        action: Action,
        constraints: TokenConstraints,
        issued_at: SecureTimestamp,
        expiry: Option<SecureTimestamp>,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            id: TokenId::new(),
            capsule,
            issuer,
            resource,
            action,
            constraints,
            issued_at,
            expiry,
            signature: [0u8; 64],
        }
    }

    // === Read-only accessors ===

    pub fn version(&self) -> u8 {
        self.version
    }
    pub fn id(&self) -> &TokenId {
        &self.id
    }
    pub fn capsule(&self) -> &str {
        &self.capsule
    }
    pub fn issuer(&self) -> &[u8; 32] {
        &self.issuer
    }
    pub fn resource(&self) -> &ResourceId {
        &self.resource
    }
    pub fn action(&self) -> Action {
        self.action
    }
    pub fn constraints(&self) -> &TokenConstraints {
        &self.constraints
    }
    pub fn issued_at(&self) -> &SecureTimestamp {
        &self.issued_at
    }
    pub fn expiry(&self) -> Option<&SecureTimestamp> {
        self.expiry.as_ref()
    }

    /// Get the bytes to be signed (everything except signature)
    ///
    /// All variable-length fields are length-prefixed (8-byte LE) to prevent
    /// collision between tokens with different field boundaries (e.g.,
    /// capsule="abc",resource="def" vs capsule="ab",resource="cdef").
    ///
    /// All `Option` fields use explicit `[0]`/`[1]` discriminants so that
    /// `None` and `Some(0)` hash differently.
    pub fn signable_bytes(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();

        // Version
        hasher.update([self.version]);

        // ID (fixed-length, no prefix needed)
        hasher.update(self.id.0);

        // Capsule (variable-length: length-prefix)
        hasher.update((self.capsule.len() as u64).to_le_bytes());
        hasher.update(self.capsule.as_bytes());

        // Issuer (fixed-length, no prefix needed)
        hasher.update(self.issuer);

        // Resource (variable-length: length-prefix)
        hasher.update((self.resource.0.len() as u64).to_le_bytes());
        hasher.update(self.resource.0.as_bytes());

        // Action
        hasher.update([self.action as u8]);

        // Constraints
        hasher.update(self.constraints.epoch.to_le_bytes());
        hasher.update([self.constraints.delegatable as u8]);

        // Option fields: explicit discriminant
        match self.constraints.max_classification {
            None => hasher.update([0u8]),
            Some(v) => {
                hasher.update([1u8]);
                hasher.update([v]);
            }
        }
        match self.constraints.max_uses {
            None => hasher.update([0u8]),
            Some(v) => {
                hasher.update([1u8]);
                hasher.update(v.to_le_bytes());
            }
        }

        // Temporal
        hasher.update(self.issued_at.unix_secs.to_le_bytes());
        hasher.update(self.issued_at.monotonic_seq.to_le_bytes());
        match &self.expiry {
            None => hasher.update([0u8]),
            Some(expiry) => {
                hasher.update([1u8]);
                hasher.update(expiry.unix_secs.to_le_bytes());
                hasher.update(expiry.monotonic_seq.to_le_bytes());
            }
        }

        hasher.finalize().to_vec()
    }

    /// Sign the token with the given signing key
    pub fn sign(&mut self, signing_key: &SigningKey) {
        let message = self.signable_bytes();
        let signature = signing_key.sign(&message);
        self.signature = signature.to_bytes();
    }

    /// Verify the token's signature against the given public key
    pub fn verify_signature(&self, public_key: &VerifyingKey) -> bool {
        let message = self.signable_bytes();
        let signature = Signature::from_bytes(&self.signature);
        public_key.verify(&message, &signature).is_ok()
    }

    /// Serialize to bytes for storage
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        bincode::serialize(self).map_err(|e| format!("token serialization failed: {}", e))
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        bincode::deserialize(bytes).map_err(|e| format!("failed to deserialize token: {}", e))
    }

    /// Serialize to base64 for JSON transport
    pub fn to_base64(&self) -> Result<String, String> {
        self.to_bytes().map(|bytes| BASE64.encode(bytes))
    }

    /// Deserialize from base64
    pub fn from_base64(s: &str) -> Result<Self, String> {
        let bytes = BASE64
            .decode(s)
            .map_err(|e| format!("invalid base64: {}", e))?;
        Self::from_bytes(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::time::SecureTimestamp;

    fn create_test_token() -> CapabilityToken {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();

        let mut token = CapabilityToken::new(
            "test-capsule".to_string(),
            verifying_key.to_bytes(),
            ResourceId::new("localhost://Users/self/Documents/test/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );

        token.sign(&signing_key);
        token
    }

    #[test]
    fn test_token_creation() {
        let token = create_test_token();
        assert_eq!(token.version, CapabilityToken::CURRENT_VERSION);
        assert_eq!(token.capsule, "test-capsule");
        assert_eq!(token.action, Action::Read);
    }

    #[test]
    fn test_token_signature() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();

        let mut token = CapabilityToken::new(
            "test-capsule".to_string(),
            verifying_key.to_bytes(),
            ResourceId::new("localhost://Users/self/Documents/test/file.txt"),
            Action::Read,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );

        token.sign(&signing_key);
        assert!(token.verify_signature(&verifying_key));

        // Tamper with token
        token.capsule = "evil-capsule".to_string();
        assert!(!token.verify_signature(&verifying_key));
    }

    #[test]
    fn test_token_serialization() {
        let token = create_test_token();

        let bytes = token.to_bytes().unwrap();
        let restored = CapabilityToken::from_bytes(&bytes).unwrap();

        assert_eq!(token.id.0, restored.id.0);
        assert_eq!(token.capsule, restored.capsule);
        assert_eq!(token.action, restored.action);
    }

    #[test]
    fn test_token_base64() {
        let token = create_test_token();

        let b64 = token.to_base64().unwrap();
        let restored = CapabilityToken::from_base64(&b64).unwrap();

        assert_eq!(token.id.0, restored.id.0);
    }

    #[test]
    fn test_resource_matching() {
        let pattern = ResourceId::new("localhost://Users/self/Documents/photos/*");
        let resource1 = ResourceId::new("localhost://Users/self/Documents/photos/vacation.jpg");
        let resource2 = ResourceId::new("localhost://Users/self/Documents/documents/report.pdf");

        assert!(resource1.matches(&pattern));
        assert!(!resource2.matches(&pattern));

        let exact = ResourceId::new("localhost://Users/self/Documents/exact/file.txt");
        let exact_match = ResourceId::new("localhost://Users/self/Documents/exact/file.txt");
        let exact_no_match = ResourceId::new("localhost://Users/self/Documents/exact/other.txt");

        assert!(exact_match.matches(&exact));
        assert!(!exact_no_match.matches(&exact));
    }

    #[test]
    fn test_resource_wildcard_rejects_path_traversal() {
        let pattern = ResourceId::new("localhost://Users/self/Documents/photos/*");

        // Direct traversal
        let traversal = ResourceId::new("localhost://Users/self/Documents/photos/../../etc/passwd");
        assert!(!traversal.matches(&pattern));

        // Traversal in deeper path
        let deep_traversal =
            ResourceId::new("localhost://Users/self/Documents/photos/sub/../../../etc/shadow");
        assert!(!deep_traversal.matches(&pattern));

        // Single dot is fine (current directory)
        let single_dot = ResourceId::new("localhost://Users/self/Documents/photos/./file.jpg");
        assert!(single_dot.matches(&pattern));

        // Legitimate nested paths still work
        let nested =
            ResourceId::new("localhost://Users/self/Documents/photos/2024/vacation/img.jpg");
        assert!(nested.matches(&pattern));
    }

    #[test]
    fn test_resource_wildcard_normalizes_double_slashes() {
        let pattern = ResourceId::new("localhost://Users/self/Documents/photos/*");

        // Double slash in resource should be collapsed before matching
        let double_slash = ResourceId::new("localhost://Users/self/Documents/photos//evil.jpg");
        assert!(double_slash.matches(&pattern));

        // Extra slashes in prefix path are normalized — both sides resolve to same path
        let extra_slash = ResourceId::new("localhost://Users/self/Documents//photos/file.jpg");
        assert!(extra_slash.matches(&pattern)); // normalizes to storage/photos/file.jpg

        // Multiple slashes all collapse
        let multi = ResourceId::new("localhost://Users/self/Documents/photos///deep///file.jpg");
        assert!(multi.matches(&pattern));

        // Traversal with double slashes still rejected
        let traversal =
            ResourceId::new("localhost://Users/self/Documents/photos//../../etc/passwd");
        assert!(!traversal.matches(&pattern));
    }

    #[test]
    fn test_resource_exact_match_normalizes() {
        let exact = ResourceId::new("localhost://Users/self/Documents/file.txt");

        // Same path with extra slashes normalizes to match
        let with_slashes = ResourceId::new("localhost://Users/self/Documents//file.txt");
        assert!(with_slashes.matches(&exact));

        // Dot segments normalize away
        let with_dot = ResourceId::new("localhost://Users/self/Documents/./file.txt");
        assert!(with_dot.matches(&exact));
    }

    // --- elastos:// capability tests ---

    #[test]
    fn test_elastos_sub_wildcard_matches_sub_paths() {
        let elastos_peer_wildcard = ResourceId::new("elastos://peer/*");
        let elastos_req = ResourceId::new("elastos://peer/alice/shared");

        assert!(elastos_req.matches(&elastos_peer_wildcard));
    }

    #[test]
    fn test_elastos_wildcard_covers_all_sub_providers() {
        // elastos://* covers all sub-provider paths
        let elastos_wildcard = ResourceId::new("elastos://*");
        let peer_req = ResourceId::new("elastos://peer/alice");
        let did_req = ResourceId::new("elastos://did/my-id");
        let cid_req = ResourceId::new("elastos://QmHash123");

        assert!(peer_req.matches(&elastos_wildcard));
        assert!(did_req.matches(&elastos_wildcard));
        assert!(cid_req.matches(&elastos_wildcard));
    }

    #[test]
    fn test_token_id() {
        let id1 = TokenId::new();
        let id2 = TokenId::new();
        assert_ne!(id1, id2);

        let display = format!("{}", id1);
        assert_eq!(display.len(), 32); // hex encoding of 16 bytes
    }

    // === H4a: Hash collision specification tests ===

    #[test]
    fn test_no_hash_collision_on_variable_length_fields() {
        // capsule="abc", resource="def" vs capsule="ab", resource="cdef"
        // These must produce different hashes (length-prefix prevents collision).
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let vk = signing_key.verifying_key();

        let t1 = CapabilityToken::new(
            "abc".to_string(),
            vk.to_bytes(),
            ResourceId::new("def"),
            Action::Read,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );

        let t2 = CapabilityToken::new(
            "ab".to_string(),
            vk.to_bytes(),
            ResourceId::new("cdef"),
            Action::Read,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );

        assert_ne!(
            t1.signable_bytes(),
            t2.signable_bytes(),
            "Variable-length field boundary shift must produce different hashes"
        );
    }

    #[test]
    fn test_no_hash_collision_on_option_none_vs_some_zero() {
        // max_classification: None vs Some(0) — semantically different
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let vk = signing_key.verifying_key();
        let now = SecureTimestamp::now();

        let t1 = CapabilityToken::new(
            "capsule".to_string(),
            vk.to_bytes(),
            ResourceId::new("resource"),
            Action::Read,
            TokenConstraints::new(0, false, None, None),
            now,
            None,
        );

        let t2 = CapabilityToken::new(
            "capsule".to_string(),
            vk.to_bytes(),
            ResourceId::new("resource"),
            Action::Read,
            TokenConstraints::new(0, false, Some(0), None),
            now,
            None,
        );

        assert_ne!(
            t1.signable_bytes(),
            t2.signable_bytes(),
            "None and Some(0) must produce different hashes"
        );
    }

    #[test]
    fn test_no_hash_collision_on_max_uses_none_vs_some_zero() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let vk = signing_key.verifying_key();
        let now = SecureTimestamp::now();

        let t1 = CapabilityToken::new(
            "capsule".to_string(),
            vk.to_bytes(),
            ResourceId::new("resource"),
            Action::Read,
            TokenConstraints::new(0, false, None, None),
            now,
            None,
        );

        let t2 = CapabilityToken::new(
            "capsule".to_string(),
            vk.to_bytes(),
            ResourceId::new("resource"),
            Action::Read,
            TokenConstraints::new(0, false, None, Some(0)),
            now,
            None,
        );

        assert_ne!(
            t1.signable_bytes(),
            t2.signable_bytes(),
            "max_uses None and Some(0) must produce different hashes"
        );
    }

    #[test]
    fn test_tampered_token_fails_signature() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();

        let mut token = CapabilityToken::new(
            "legit-capsule".to_string(),
            verifying_key.to_bytes(),
            ResourceId::new("localhost://Users/self/Documents/secret/*"),
            Action::Read,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );

        token.sign(&signing_key);
        assert!(token.verify_signature(&verifying_key));

        // Tamper: change action
        token.action = Action::Admin;
        assert!(
            !token.verify_signature(&verifying_key),
            "Tampered action must fail signature"
        );

        // Tamper: change resource
        token.action = Action::Read; // restore
        token.resource = ResourceId::new("localhost://Users/self/Documents/*");
        assert!(
            !token.verify_signature(&verifying_key),
            "Tampered resource must fail signature"
        );
    }
}
