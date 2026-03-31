//! User Namespace Management
//!
//! A namespace maps user-friendly paths to content-addressed identifiers (CIDs).
//! This is the core abstraction that enables "log in from anywhere" - the namespace
//! itself is content-addressed and signed by the user's key, making it portable
//! across devices.
//!
//! ```text
//! User's Namespace (signed by user's Ed25519 key)
//! ├── photos/vacation.jpg  →  elastos://Qm123...
//! ├── documents/notes.txt  →  elastos://Qm456...
//! └── apps/editor/         →  elastos://Qm789...
//! ```
//!
//! When a user accesses `localhost://Public/photos/vacation.jpg`:
//! 1. Look up path in namespace → get CID
//! 2. Check local cache for CID
//! 3. If not cached, fetch from IPFS/peers
//! 4. Return content (verified by hash)

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use elastos_common::localhost::parse_localhost_uri;
use elastos_common::SecureTimestamp;

/// Current namespace format version
pub const NAMESPACE_VERSION: u32 = 1;

/// A content identifier (CID) - either IPFS or SHA256
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentId(pub String);

impl ContentId {
    /// Create a new CID from an IPFS hash
    pub fn ipfs(hash: impl Into<String>) -> Self {
        let h = hash.into();
        if h.starts_with("elastos://") {
            Self(h)
        } else {
            Self(format!("elastos://{}", h))
        }
    }

    /// Create a new CID from SHA256 hash of content
    pub fn from_content(content: &[u8]) -> Self {
        let hash = Sha256::digest(content);
        Self(format!("elastos://sha256:{}", hex::encode(hash)))
    }

    /// Get the raw identifier without the elastos:// prefix
    pub fn raw(&self) -> &str {
        self.0.strip_prefix("elastos://").unwrap_or(&self.0)
    }

    /// Check if this is an IPFS CID
    pub fn is_ipfs(&self) -> bool {
        let raw = self.raw();
        raw.starts_with("Qm") || raw.starts_with("baf")
    }

    /// Check if this is a SHA256 hash
    pub fn is_sha256(&self) -> bool {
        self.raw().starts_with("sha256:")
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ContentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ContentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// An entry in the namespace - either a file or directory
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NamespaceEntry {
    /// A file - content identified by CID
    File {
        /// Content identifier
        cid: ContentId,
        /// Size in bytes
        size: u64,
        /// MIME type (optional)
        #[serde(skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
        /// When the entry was last modified
        modified_at: SecureTimestamp,
    },

    /// A directory - contains child entries
    Directory {
        /// Child entries (name -> entry)
        children: BTreeMap<String, NamespaceEntry>,
        /// When the directory was last modified
        modified_at: SecureTimestamp,
    },
}

impl NamespaceEntry {
    /// Create a new file entry
    pub fn file(cid: ContentId, size: u64, content_type: Option<String>) -> Self {
        Self::File {
            cid,
            size,
            content_type,
            modified_at: SecureTimestamp::now(),
        }
    }

    /// Create a new empty directory
    pub fn directory() -> Self {
        Self::Directory {
            children: BTreeMap::new(),
            modified_at: SecureTimestamp::now(),
        }
    }

    /// Check if this is a file
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File { .. })
    }

    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }

    /// Get the CID if this is a file
    pub fn cid(&self) -> Option<&ContentId> {
        match self {
            Self::File { cid, .. } => Some(cid),
            Self::Directory { .. } => None,
        }
    }

    /// Get the size if this is a file
    pub fn size(&self) -> u64 {
        match self {
            Self::File { size, .. } => *size,
            Self::Directory { children, .. } => children.values().map(|e| e.size()).sum(),
        }
    }

    /// Get the modification timestamp
    pub fn modified_at(&self) -> &SecureTimestamp {
        match self {
            Self::File { modified_at, .. } => modified_at,
            Self::Directory { modified_at, .. } => modified_at,
        }
    }

    /// Get children if this is a directory
    pub fn children(&self) -> Option<&BTreeMap<String, NamespaceEntry>> {
        match self {
            Self::Directory { children, .. } => Some(children),
            Self::File { .. } => None,
        }
    }

    /// Get mutable children if this is a directory
    pub fn children_mut(&mut self) -> Option<&mut BTreeMap<String, NamespaceEntry>> {
        match self {
            Self::Directory {
                children,
                modified_at,
            } => {
                *modified_at = SecureTimestamp::now();
                Some(children)
            }
            Self::File { .. } => None,
        }
    }
}

/// A user's namespace - maps paths to content IDs
///
/// This is the portable, content-addressed "filesystem" that follows
/// the user across devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {
    /// Namespace format version
    pub version: u32,

    /// Owner's public key (32 bytes, hex-encoded for JSON)
    pub owner: String,

    /// Root directory entries
    pub root: NamespaceEntry,

    /// When this namespace version was created
    pub created_at: SecureTimestamp,

    /// When this namespace was last modified
    pub modified_at: SecureTimestamp,

    /// Signature over all above fields (base64-encoded)
    /// None if namespace is unsigned (local draft)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl Namespace {
    /// Create a new empty namespace for a user
    pub fn new(owner_pubkey: &VerifyingKey) -> Self {
        let now = SecureTimestamp::now();
        Self {
            version: NAMESPACE_VERSION,
            owner: hex::encode(owner_pubkey.as_bytes()),
            root: NamespaceEntry::directory(),
            created_at: now,
            modified_at: now,
            signature: None,
        }
    }

    /// Create a namespace from an owner's hex-encoded public key
    pub fn with_owner_hex(owner_hex: &str) -> Result<Self, NamespaceError> {
        // Validate the hex is valid
        let bytes =
            hex::decode(owner_hex).map_err(|e| NamespaceError::InvalidOwner(e.to_string()))?;

        if bytes.len() != 32 {
            return Err(NamespaceError::InvalidOwner(
                "Owner key must be 32 bytes".into(),
            ));
        }

        let now = SecureTimestamp::now();
        Ok(Self {
            version: NAMESPACE_VERSION,
            owner: owner_hex.to_string(),
            root: NamespaceEntry::directory(),
            created_at: now,
            modified_at: now,
            signature: None,
        })
    }

    /// Normalize a path (remove leading/trailing slashes, handle empty)
    fn normalize_path(path: &str) -> &str {
        let path = if let Some((_root, rest)) = parse_localhost_uri(path) {
            rest
        } else {
            path
        };
        let path = path.trim_matches('/');
        if path.is_empty() {
            ""
        } else {
            path
        }
    }

    /// Split a path into components
    fn path_components(path: &str) -> Vec<&str> {
        let path = Self::normalize_path(path);
        if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect()
        }
    }

    /// Resolve a path to its entry
    pub fn resolve(&self, path: &str) -> Option<&NamespaceEntry> {
        let components = Self::path_components(path);

        if components.is_empty() {
            return Some(&self.root);
        }

        let mut current = &self.root;
        for component in components {
            match current {
                NamespaceEntry::Directory { children, .. } => {
                    current = children.get(component)?;
                }
                NamespaceEntry::File { .. } => return None,
            }
        }

        Some(current)
    }

    /// Resolve a path to its mutable entry
    fn resolve_mut(&mut self, path: &str) -> Option<&mut NamespaceEntry> {
        let components = Self::path_components(path);

        if components.is_empty() {
            return Some(&mut self.root);
        }

        let mut current = &mut self.root;
        for component in components {
            match current {
                NamespaceEntry::Directory { children, .. } => {
                    current = children.get_mut(component)?;
                }
                NamespaceEntry::File { .. } => return None,
            }
        }

        Some(current)
    }

    /// Get the parent directory of a path, creating intermediate dirs if needed
    fn get_or_create_parent(&mut self, path: &str) -> Result<&mut NamespaceEntry, NamespaceError> {
        let components = Self::path_components(path);

        if components.is_empty() {
            return Ok(&mut self.root);
        }

        // All but the last component
        let parent_components = &components[..components.len() - 1];

        let mut current = &mut self.root;
        for component in parent_components {
            // Ensure current is a directory
            if !current.is_directory() {
                return Err(NamespaceError::NotADirectory(component.to_string()));
            }

            let children = current.children_mut().unwrap();

            // Create directory if it doesn't exist
            if !children.contains_key(*component) {
                children.insert(component.to_string(), NamespaceEntry::directory());
            }

            current = children.get_mut(*component).unwrap();
        }

        if !current.is_directory() {
            return Err(NamespaceError::NotADirectory(
                parent_components.last().unwrap_or(&"").to_string(),
            ));
        }

        Ok(current)
    }

    /// Add or update an entry at the given path
    pub fn put(&mut self, path: &str, entry: NamespaceEntry) -> Result<(), NamespaceError> {
        let components = Self::path_components(path);

        if components.is_empty() {
            return Err(NamespaceError::InvalidPath("Cannot replace root".into()));
        }

        let name = components.last().unwrap().to_string();
        let parent = self.get_or_create_parent(path)?;

        let children = parent.children_mut().unwrap();
        children.insert(name, entry);

        self.modified_at = SecureTimestamp::now();
        self.signature = None; // Invalidate signature on modification

        Ok(())
    }

    /// Remove an entry at the given path
    pub fn remove(&mut self, path: &str) -> Result<NamespaceEntry, NamespaceError> {
        let components = Self::path_components(path);

        if components.is_empty() {
            return Err(NamespaceError::InvalidPath("Cannot remove root".into()));
        }

        let name = components.last().unwrap();

        // Get parent path
        let parent_path = if components.len() == 1 {
            "".to_string()
        } else {
            components[..components.len() - 1].join("/")
        };

        let parent = self
            .resolve_mut(&parent_path)
            .ok_or_else(|| NamespaceError::NotFound(parent_path.clone()))?;

        let children = parent
            .children_mut()
            .ok_or(NamespaceError::NotADirectory(parent_path))?;

        let entry = children
            .remove(*name)
            .ok_or_else(|| NamespaceError::NotFound(path.to_string()))?;

        self.modified_at = SecureTimestamp::now();
        self.signature = None;

        Ok(entry)
    }

    /// List entries at a path (returns name -> entry pairs)
    pub fn list(&self, path: &str) -> Result<Vec<(&str, &NamespaceEntry)>, NamespaceError> {
        let entry = self
            .resolve(path)
            .ok_or_else(|| NamespaceError::NotFound(path.to_string()))?;

        match entry {
            NamespaceEntry::Directory { children, .. } => {
                Ok(children.iter().map(|(k, v)| (k.as_str(), v)).collect())
            }
            NamespaceEntry::File { .. } => Err(NamespaceError::NotADirectory(path.to_string())),
        }
    }

    /// Count total entries in the namespace
    pub fn entry_count(&self) -> u64 {
        fn count_recursive(entry: &NamespaceEntry) -> u64 {
            match entry {
                NamespaceEntry::File { .. } => 1,
                NamespaceEntry::Directory { children, .. } => {
                    children.values().map(count_recursive).sum()
                }
            }
        }
        count_recursive(&self.root)
    }

    /// Calculate total size of all files
    pub fn total_size(&self) -> u64 {
        self.root.size()
    }

    /// Sign the namespace with the owner's private key
    pub fn sign(&mut self, signing_key: &SigningKey) -> Result<(), NamespaceError> {
        // Verify the signing key matches the owner
        let verifying_key = signing_key.verifying_key();
        let expected_owner = hex::encode(verifying_key.as_bytes());

        if self.owner != expected_owner {
            return Err(NamespaceError::SigningKeyMismatch);
        }

        // Clear existing signature before computing new one
        self.signature = None;

        // Serialize and hash
        let message = self.signing_message()?;

        // Sign
        let signature = signing_key.sign(&message);
        self.signature = Some(BASE64.encode(signature.to_bytes()));

        Ok(())
    }

    /// Verify the namespace signature
    pub fn verify(&self) -> Result<bool, NamespaceError> {
        let signature_b64 = self.signature.as_ref().ok_or(NamespaceError::NotSigned)?;

        // Decode owner public key
        let owner_bytes =
            hex::decode(&self.owner).map_err(|e| NamespaceError::InvalidOwner(e.to_string()))?;

        let owner_bytes: [u8; 32] = owner_bytes
            .try_into()
            .map_err(|_| NamespaceError::InvalidOwner("Invalid key length".into()))?;

        let verifying_key = VerifyingKey::from_bytes(&owner_bytes)
            .map_err(|e| NamespaceError::InvalidOwner(e.to_string()))?;

        // Decode signature
        let sig_bytes = BASE64
            .decode(signature_b64)
            .map_err(|e| NamespaceError::InvalidSignature(e.to_string()))?;

        let signature = Signature::from_slice(&sig_bytes)
            .map_err(|e| NamespaceError::InvalidSignature(e.to_string()))?;

        // Build message (without signature field)
        let mut ns_for_verify = self.clone();
        ns_for_verify.signature = None;
        let message = ns_for_verify.signing_message()?;

        // Verify
        Ok(verifying_key.verify(&message, &signature).is_ok())
    }

    /// Get the message to sign (hash of serialized namespace without signature)
    fn signing_message(&self) -> Result<Vec<u8>, NamespaceError> {
        let json = serde_json::to_string(self)
            .map_err(|e| NamespaceError::SerializationError(e.to_string()))?;

        Ok(Sha256::digest(json.as_bytes()).to_vec())
    }

    /// Compute the CID of this namespace (for content-addressing the namespace itself)
    pub fn to_cid(&self) -> Result<ContentId, NamespaceError> {
        let json = serde_json::to_string(self)
            .map_err(|e| NamespaceError::SerializationError(e.to_string()))?;

        Ok(ContentId::from_content(json.as_bytes()))
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, NamespaceError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| NamespaceError::SerializationError(e.to_string()))
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, NamespaceError> {
        serde_json::from_str(json).map_err(|e| NamespaceError::SerializationError(e.to_string()))
    }
}

/// Errors that can occur during namespace operations
#[derive(Debug, Clone)]
pub enum NamespaceError {
    /// Path not found
    NotFound(String),
    /// Path is not a directory
    NotADirectory(String),
    /// Path is not a file
    NotAFile(String),
    /// Invalid path
    InvalidPath(String),
    /// Invalid owner key
    InvalidOwner(String),
    /// Namespace is not signed
    NotSigned,
    /// Invalid signature
    InvalidSignature(String),
    /// Signing key doesn't match owner
    SigningKeyMismatch,
    /// Serialization error
    SerializationError(String),
    /// IO error
    IoError(String),
    /// Content fetch error
    FetchError(String),
}

impl std::fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "path not found: {}", p),
            Self::NotADirectory(p) => write!(f, "not a directory: {}", p),
            Self::NotAFile(p) => write!(f, "not a file: {}", p),
            Self::InvalidPath(p) => write!(f, "invalid path: {}", p),
            Self::InvalidOwner(e) => write!(f, "invalid owner: {}", e),
            Self::NotSigned => write!(f, "namespace is not signed"),
            Self::InvalidSignature(e) => write!(f, "invalid signature: {}", e),
            Self::SigningKeyMismatch => write!(f, "signing key doesn't match owner"),
            Self::SerializationError(e) => write!(f, "serialization error: {}", e),
            Self::IoError(e) => write!(f, "IO error: {}", e),
            Self::FetchError(e) => write!(f, "fetch error: {}", e),
        }
    }
}

impl std::error::Error for NamespaceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    fn generate_keypair() -> (SigningKey, ed25519_dalek::VerifyingKey) {
        let signing = SigningKey::generate(&mut OsRng);
        let verifying = signing.verifying_key();
        (signing, verifying)
    }

    #[test]
    fn test_content_id_creation() {
        let cid = ContentId::ipfs("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG");
        assert!(cid.is_ipfs());
        assert!(!cid.is_sha256());
        assert_eq!(cid.raw(), "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG");

        let cid = ContentId::from_content(b"hello world");
        assert!(cid.is_sha256());
        assert!(!cid.is_ipfs());
    }

    #[test]
    fn test_namespace_creation() {
        let (_, verifying_key) = generate_keypair();
        let ns = Namespace::new(&verifying_key);

        assert_eq!(ns.version, NAMESPACE_VERSION);
        assert_eq!(ns.owner, hex::encode(verifying_key.as_bytes()));
        assert!(ns.root.is_directory());
        assert!(ns.signature.is_none());
    }

    #[test]
    fn test_namespace_put_and_resolve() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        // Add a file
        let entry = NamespaceEntry::file(
            ContentId::ipfs("QmTest123"),
            1024,
            Some("image/jpeg".into()),
        );
        ns.put("photos/vacation.jpg", entry).unwrap();

        // Resolve it
        let resolved = ns.resolve("photos/vacation.jpg").unwrap();
        assert!(resolved.is_file());
        assert_eq!(resolved.cid().unwrap().raw(), "QmTest123");
        assert_eq!(resolved.size(), 1024);

        // Resolve the directory
        let dir = ns.resolve("photos").unwrap();
        assert!(dir.is_directory());
    }

    #[test]
    fn test_namespace_list() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "photos/a.jpg",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();
        ns.put(
            "photos/b.jpg",
            NamespaceEntry::file(ContentId::ipfs("Qm2"), 200, None),
        )
        .unwrap();
        ns.put(
            "photos/subdir/c.jpg",
            NamespaceEntry::file(ContentId::ipfs("Qm3"), 300, None),
        )
        .unwrap();

        let entries = ns.list("photos").unwrap();
        assert_eq!(entries.len(), 3); // a.jpg, b.jpg, subdir

        let names: Vec<_> = entries.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"a.jpg"));
        assert!(names.contains(&"b.jpg"));
        assert!(names.contains(&"subdir"));
    }

    #[test]
    fn test_namespace_remove() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "photos/test.jpg",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();
        assert!(ns.resolve("photos/test.jpg").is_some());

        let removed = ns.remove("photos/test.jpg").unwrap();
        assert!(removed.is_file());
        assert!(ns.resolve("photos/test.jpg").is_none());
    }

    #[test]
    fn test_namespace_sign_and_verify() {
        let (signing_key, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "test.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();

        // Sign
        ns.sign(&signing_key).unwrap();
        assert!(ns.signature.is_some());

        // Verify
        assert!(ns.verify().unwrap());

        // Tamper
        ns.put(
            "another.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm2"), 200, None),
        )
        .unwrap();

        // Signature was cleared on modification
        assert!(ns.signature.is_none());
    }

    #[test]
    fn test_namespace_sign_wrong_key() {
        let (_, verifying_key) = generate_keypair();
        let (wrong_signing_key, _) = generate_keypair();

        let mut ns = Namespace::new(&verifying_key);

        let result = ns.sign(&wrong_signing_key);
        assert!(matches!(result, Err(NamespaceError::SigningKeyMismatch)));
    }

    #[test]
    fn test_namespace_entry_count() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        assert_eq!(ns.entry_count(), 0);

        ns.put(
            "a.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();
        ns.put(
            "b.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm2"), 200, None),
        )
        .unwrap();
        ns.put(
            "dir/c.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm3"), 300, None),
        )
        .unwrap();

        assert_eq!(ns.entry_count(), 3);
    }

    #[test]
    fn test_namespace_total_size() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "a.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();
        ns.put(
            "b.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm2"), 200, None),
        )
        .unwrap();
        ns.put(
            "dir/c.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm3"), 300, None),
        )
        .unwrap();

        assert_eq!(ns.total_size(), 600);
    }

    #[test]
    fn test_namespace_to_cid() {
        let (_, verifying_key) = generate_keypair();
        let ns = Namespace::new(&verifying_key);

        let cid = ns.to_cid().unwrap();
        assert!(cid.is_sha256());
    }

    #[test]
    fn test_namespace_json_roundtrip() {
        let (signing_key, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "test.txt",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();
        ns.sign(&signing_key).unwrap();

        let json = ns.to_json().unwrap();
        let ns2 = Namespace::from_json(&json).unwrap();

        assert_eq!(ns.owner, ns2.owner);
        assert_eq!(ns.signature, ns2.signature);
        assert!(ns2.verify().unwrap());
    }

    #[test]
    fn test_path_normalization() {
        let (_, verifying_key) = generate_keypair();
        let mut ns = Namespace::new(&verifying_key);

        ns.put(
            "localhost://Public/photos/test.jpg",
            NamespaceEntry::file(ContentId::ipfs("Qm1"), 100, None),
        )
        .unwrap();

        // All these should resolve to the same entry
        assert!(ns.resolve("photos/test.jpg").is_some());
        assert!(ns.resolve("/photos/test.jpg").is_some());
        assert!(ns.resolve("localhost://Public/photos/test.jpg").is_some());
    }
}
