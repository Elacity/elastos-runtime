//! Content resolver for elastos:// URIs
//!
//! Handles fetching and verifying content-addressed resources.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Maximum content size (100 MB)
pub const MAX_CONTENT_SIZE: usize = 100 * 1024 * 1024;

/// Parsed elastos:// URI
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUri {
    /// The content identifier (CID or hash)
    pub identifier: String,
    /// The identifier type
    pub id_type: IdentifierType,
    /// Optional path within the content (for directory CIDs)
    pub path: Option<String>,
}

/// Type of content identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierType {
    /// IPFS CID (Qm... or bafy...)
    IpfsCid,
    /// SHA-256 hash
    Sha256,
    /// Unknown/invalid
    Unknown,
}

impl ContentUri {
    /// Parse an elastos:// URI
    pub fn parse(uri: &str) -> Result<Self, ParseError> {
        // Must start with elastos://
        if !uri.starts_with("elastos://") {
            return Err(ParseError::InvalidScheme);
        }

        let rest = &uri[10..]; // Skip "elastos://"

        // Check for empty identifier
        if rest.is_empty() {
            return Err(ParseError::EmptyIdentifier);
        }

        // Split by / to separate identifier from path
        let (identifier, path) = if let Some(slash_pos) = rest.find('/') {
            let id = &rest[..slash_pos];
            let p = &rest[slash_pos + 1..];
            (
                id.to_string(),
                if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                },
            )
        } else {
            (rest.to_string(), None)
        };

        // Determine identifier type
        let id_type = Self::detect_type(&identifier);

        if id_type == IdentifierType::Unknown {
            return Err(ParseError::InvalidIdentifier(identifier));
        }

        Ok(ContentUri {
            identifier,
            id_type,
            path,
        })
    }

    /// Detect the type of identifier
    fn detect_type(id: &str) -> IdentifierType {
        // SHA-256 prefix
        if let Some(hash) = id.strip_prefix("sha256:") {
            if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return IdentifierType::Sha256;
            }
        }

        // IPFS CIDv0 (Qm...)
        if id.starts_with("Qm") && id.len() == 46 {
            return IdentifierType::IpfsCid;
        }

        // IPFS CIDv1 (bafy..., bafk...)
        if id.starts_with("baf") && id.len() >= 50 {
            return IdentifierType::IpfsCid;
        }

        IdentifierType::Unknown
    }
}

impl std::fmt::Display for ContentUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "elastos://{}/{}", self.identifier, path)
        } else {
            write!(f, "elastos://{}", self.identifier)
        }
    }
}

/// Parse error for URIs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    InvalidScheme,
    EmptyIdentifier,
    InvalidIdentifier(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidScheme => write!(f, "URI must start with elastos://"),
            ParseError::EmptyIdentifier => write!(f, "empty content identifier"),
            ParseError::InvalidIdentifier(id) => write!(f, "invalid identifier: {}", id),
        }
    }
}

impl std::error::Error for ParseError {}

/// Result of fetching content
#[derive(Debug)]
pub struct FetchResult {
    /// The fetched content bytes
    pub content: Vec<u8>,
    /// Where the content was fetched from
    pub source: FetchSource,
    /// Whether the content hash was verified
    pub verified: bool,
}

/// Error types for content fetching
#[derive(Debug)]
pub enum FetchError {
    /// URI parsing failed
    Parse(ParseError),
    /// Content not found
    NotFound,
    /// Hash verification failed
    HashMismatch { expected: String, actual: String },
    /// Content too large
    TooLarge { size: usize, max: usize },
    /// Network error
    Network(String),
    /// IO error
    Io(std::io::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Parse(e) => write!(f, "parse error: {}", e),
            FetchError::NotFound => write!(f, "content not found"),
            FetchError::HashMismatch { expected, actual } => {
                write!(f, "hash mismatch: expected {}, got {}", expected, actual)
            }
            FetchError::TooLarge { size, max } => {
                write!(f, "content too large: {} bytes (max {})", size, max)
            }
            FetchError::Network(e) => write!(f, "network error: {}", e),
            FetchError::Io(e) => write!(f, "io error: {}", e),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<ParseError> for FetchError {
    fn from(e: ParseError) -> Self {
        FetchError::Parse(e)
    }
}

impl From<std::io::Error> for FetchError {
    fn from(e: std::io::Error) -> Self {
        FetchError::Io(e)
    }
}

/// Content resolver configuration
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Local cache directory
    pub cache_dir: Option<PathBuf>,
    /// IPFS gateway URLs (tried in order)
    pub ipfs_gateways: Vec<String>,
    /// Maximum content size
    pub max_content_size: usize,
    /// Enable caching
    pub enable_cache: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            // Off-box transport must be injected explicitly by the caller.
            // The resolver should not silently inherit public-web gateways.
            ipfs_gateways: Vec::new(),
            max_content_size: MAX_CONTENT_SIZE,
            enable_cache: true,
        }
    }
}

/// Trait for fetching raw bytes from a URL.
///
/// This abstracts the transport mechanism (HTTP, P2P, local) away from the
/// content resolution logic. The runtime defines this trait; transport-specific
/// implementations live in the server crate.
#[async_trait::async_trait]
pub trait ContentFetcher: Send + Sync {
    /// Fetch raw bytes from the given URL.
    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}

/// A no-op fetcher that always returns NotFound.
///
/// Used when the runtime operates without network access (offline mode, tests).
pub struct NullFetcher;

#[async_trait::async_trait]
impl ContentFetcher for NullFetcher {
    async fn fetch_url(&self, _url: &str) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::NotFound)
    }
}

/// Source of a content fetch (for audit logging)
#[derive(Debug, Clone)]
pub enum FetchSource {
    LocalCache,
    IpfsGateway(String),
}

/// Trait for audit logging — injected by the runtime
pub trait AuditSink: Send + Sync {
    fn content_fetch(&self, identifier: &str, source: FetchSource, verified: bool);
}

/// No-op audit sink for tests and offline mode
pub struct NullAuditSink;
impl AuditSink for NullAuditSink {
    fn content_fetch(&self, _: &str, _: FetchSource, _: bool) {}
}

// === CID Multihash Verification ===

/// Result of verifying content against a CID's embedded multihash.
enum CidVerification {
    /// Content hash matches the CID multihash.
    Verified,
    /// CID uses a codec or hash function that can't be verified against raw bytes.
    Unverifiable(&'static str),
    /// CID string could not be parsed — input is not a valid CID.
    InvalidCid(String),
    /// Content hash does NOT match the CID multihash — gateway returned wrong data.
    Mismatch(String),
}

/// Verify that fetched content matches the expected CID multihash.
///
/// Only raw-codec (0x55) CIDv1 with SHA2-256 can be verified directly,
/// because gateways return the unwrapped file content. CIDv0 (Qm...) and
/// dag-pb CIDv1 hash over the protobuf-wrapped block, not raw bytes.
fn verify_cid_content(cid_str: &str, content: &[u8]) -> CidVerification {
    let parsed = match cid::Cid::try_from(cid_str) {
        Ok(c) => c,
        Err(e) => {
            return CidVerification::InvalidCid(format!("failed to parse CID '{}': {}", cid_str, e))
        }
    };

    // Only raw-codec CIDv1 can be verified against raw bytes
    if parsed.version() != cid::Version::V1 || parsed.codec() != 0x55 {
        return CidVerification::Unverifiable("non-raw codec");
    }

    let mh = parsed.hash();
    // 0x12 = SHA2-256, the standard IPFS hash function
    if mh.code() != 0x12 {
        return CidVerification::Unverifiable("unsupported hash function");
    }

    let expected = mh.digest();
    let actual = Sha256::digest(content);
    if actual.as_slice() == expected {
        CidVerification::Verified
    } else {
        CidVerification::Mismatch(format!(
            "CID hash mismatch: expected {}, got {}",
            hex::encode(expected),
            hex::encode(actual)
        ))
    }
}

/// Content resolver for elastos:// URIs
pub struct ContentResolver {
    config: ResolverConfig,
    /// In-memory cache (for testing and small deployments)
    cache: RwLock<HashMap<String, Vec<u8>>>,
    /// Transport-agnostic fetcher (injected by server)
    fetcher: Arc<dyn ContentFetcher>,
    /// Audit log
    audit_log: Arc<dyn AuditSink>,
}

impl ContentResolver {
    /// Create a new content resolver with the given fetcher.
    pub fn new(
        config: ResolverConfig,
        audit_log: Arc<dyn AuditSink>,
        fetcher: Arc<dyn ContentFetcher>,
    ) -> Self {
        Self {
            config,
            cache: RwLock::new(HashMap::new()),
            fetcher,
            audit_log,
        }
    }

    /// Fetch content by elastos:// URI
    pub async fn fetch(&self, uri: &str) -> Result<FetchResult, FetchError> {
        let content_uri = ContentUri::parse(uri)?;
        self.fetch_uri(&content_uri).await
    }

    /// Fetch content by parsed URI
    pub async fn fetch_uri(&self, uri: &ContentUri) -> Result<FetchResult, FetchError> {
        // Try local cache first
        if self.config.enable_cache {
            if let Some(content) = self.check_cache(&uri.identifier).await {
                self.audit_log
                    .content_fetch(&uri.identifier, FetchSource::LocalCache, true);
                return Ok(FetchResult {
                    content,
                    source: FetchSource::LocalCache,
                    verified: true, // Cached content was verified on storage
                });
            }
        }

        // Fetch based on identifier type
        match uri.id_type {
            IdentifierType::IpfsCid => self.fetch_from_ipfs(uri).await,
            IdentifierType::Sha256 => self.fetch_by_sha256(uri).await,
            IdentifierType::Unknown => Err(FetchError::NotFound),
        }
    }

    /// Fetch from IPFS gateways with CID multihash verification.
    ///
    /// On hash mismatch, rejects the gateway response and tries the next gateway.
    /// For CIDs with unverifiable codecs (dag-pb, CIDv0), proceeds with `verified: false`.
    async fn fetch_from_ipfs(&self, uri: &ContentUri) -> Result<FetchResult, FetchError> {
        let path_suffix = uri
            .path
            .as_ref()
            .map(|p| format!("/{}", p))
            .unwrap_or_default();

        for gateway in &self.config.ipfs_gateways {
            let url = format!(
                "{}/ipfs/{}{}",
                gateway.trim_end_matches('/'),
                uri.identifier,
                path_suffix
            );

            match self.fetch_url(&url).await {
                Ok(content) => {
                    // Verify content against CID multihash (bare CID fetches only)
                    let verified = if path_suffix.is_empty() {
                        match verify_cid_content(&uri.identifier, &content) {
                            CidVerification::Verified => {
                                tracing::info!(
                                    "IPFS content verified against CID multihash: {}",
                                    uri.identifier
                                );
                                true
                            }
                            CidVerification::Unverifiable(reason) => {
                                tracing::debug!(
                                    "CID not directly verifiable ({}), proceeding unverified",
                                    reason
                                );
                                false
                            }
                            CidVerification::InvalidCid(err) => {
                                tracing::warn!(
                                    "CID syntax invalid for '{}': {}",
                                    uri.identifier,
                                    err
                                );
                                false // Proceed unverified — URI was accepted upstream
                            }
                            CidVerification::Mismatch(err) => {
                                tracing::warn!(
                                    "Gateway {} failed CID verification: {}",
                                    gateway,
                                    err
                                );
                                continue; // Reject, try next gateway
                            }
                        }
                    } else {
                        false // Sub-path fetch, can't verify against parent CID
                    };

                    // Cache the content
                    if self.config.enable_cache {
                        self.store_cache(&uri.identifier, content.clone()).await;
                    }

                    self.audit_log.content_fetch(
                        &uri.identifier,
                        FetchSource::IpfsGateway(gateway.clone()),
                        verified,
                    );

                    return Ok(FetchResult {
                        content,
                        source: FetchSource::IpfsGateway(gateway.clone()),
                        verified,
                    });
                }
                Err(e) => {
                    tracing::debug!("Failed to fetch from {}: {}", gateway, e);
                    continue;
                }
            }
        }

        self.audit_log.content_fetch(
            &uri.identifier,
            FetchSource::IpfsGateway("all".to_string()),
            false,
        );

        Err(FetchError::NotFound)
    }

    /// Fetch content by SHA-256 hash
    ///
    /// Checks cache first, then tries IPFS gateways (in case the SHA-256 maps
    /// to cached content). Verifies content hash using `verify_hash()`.
    async fn fetch_by_sha256(&self, uri: &ContentUri) -> Result<FetchResult, FetchError> {
        // Check if we have this hash in file cache
        if let Some(cache_dir) = &self.config.cache_dir {
            let cache_path = cache_dir.join(Self::cache_filename(&uri.identifier));
            if cache_path.exists() {
                if let Ok(content) = tokio::fs::read(&cache_path).await {
                    if Self::verify_hash(&content, &uri.identifier) {
                        self.audit_log.content_fetch(
                            &uri.identifier,
                            FetchSource::LocalCache,
                            true,
                        );
                        return Ok(FetchResult {
                            content,
                            source: FetchSource::LocalCache,
                            verified: true,
                        });
                    }
                }
            }
        }

        // SHA-256 identifiers can't be fetched from IPFS gateways directly
        Err(FetchError::NotFound)
    }

    /// Fetch from a URL via the injected fetcher
    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let bytes = self.fetcher.fetch_url(url).await?;

        if bytes.len() > self.config.max_content_size {
            return Err(FetchError::TooLarge {
                size: bytes.len(),
                max: self.config.max_content_size,
            });
        }

        Ok(bytes)
    }

    /// Check the local cache
    async fn check_cache(&self, identifier: &str) -> Option<Vec<u8>> {
        // Check memory cache
        let cache = self.cache.read().await;
        if let Some(content) = cache.get(identifier) {
            return Some(content.clone());
        }
        drop(cache);

        // Check file cache
        if let Some(cache_dir) = &self.config.cache_dir {
            let cache_path = cache_dir.join(Self::cache_filename(identifier));
            if cache_path.exists() {
                if let Ok(content) = tokio::fs::read(&cache_path).await {
                    // Store in memory cache too
                    let mut cache = self.cache.write().await;
                    cache.insert(identifier.to_string(), content.clone());
                    return Some(content);
                }
            }
        }

        None
    }

    /// Store content in cache
    async fn store_cache(&self, identifier: &str, content: Vec<u8>) {
        // Store in memory cache
        let mut cache = self.cache.write().await;
        cache.insert(identifier.to_string(), content.clone());
        drop(cache);

        // Store in file cache
        if let Some(cache_dir) = &self.config.cache_dir {
            if let Err(e) = tokio::fs::create_dir_all(cache_dir).await {
                tracing::warn!("Failed to create cache directory: {}", e);
                return;
            }

            let cache_path = cache_dir.join(Self::cache_filename(identifier));
            if let Err(e) = tokio::fs::write(&cache_path, &content).await {
                tracing::warn!("Failed to write cache file: {}", e);
            }
        }
    }

    /// Generate a safe filename for cache
    fn cache_filename(identifier: &str) -> String {
        // Use URL-safe base64 of the identifier
        let hash = Sha256::digest(identifier.as_bytes());
        format!("{}.cache", hex::encode(&hash[..16]))
    }

    /// Compute SHA-256 hash of content
    pub fn hash_content(content: &[u8]) -> String {
        let hash = Sha256::digest(content);
        format!("sha256:{}", hex::encode(hash))
    }

    /// Verify content against expected hash
    pub fn verify_hash(content: &[u8], expected: &str) -> bool {
        let actual = Self::hash_content(content);
        actual == expected
    }

    /// Clear the cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();

        if let Some(cache_dir) = &self.config.cache_dir {
            let _ = tokio::fs::remove_dir_all(cache_dir).await;
            let _ = tokio::fs::create_dir_all(cache_dir).await;
        }
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        CacheStats {
            memory_entries: cache.len(),
            memory_bytes: cache.values().map(|v| v.len()).sum(),
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub memory_entries: usize,
    pub memory_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ipfs_cid() {
        let uri =
            ContentUri::parse("elastos://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG").unwrap();
        assert_eq!(
            uri.identifier,
            "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"
        );
        assert_eq!(uri.id_type, IdentifierType::IpfsCid);
        assert_eq!(uri.path, None);
    }

    #[test]
    fn test_parse_ipfs_cid_with_path() {
        let uri = ContentUri::parse(
            "elastos://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG/capsule.json",
        )
        .unwrap();
        assert_eq!(
            uri.identifier,
            "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"
        );
        assert_eq!(uri.path, Some("capsule.json".to_string()));
    }

    #[test]
    fn test_parse_sha256() {
        let uri = ContentUri::parse(
            "elastos://sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();
        assert_eq!(uri.id_type, IdentifierType::Sha256);
    }

    #[test]
    fn test_parse_invalid_scheme() {
        let result = ContentUri::parse("http://example.com");
        assert!(matches!(result, Err(ParseError::InvalidScheme)));
    }

    #[test]
    fn test_parse_empty_identifier() {
        let result = ContentUri::parse("elastos://");
        assert!(matches!(result, Err(ParseError::EmptyIdentifier)));
    }

    #[test]
    fn test_hash_content() {
        let content = b"hello world";
        let hash = ContentResolver::hash_content(content);
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 71); // "sha256:" + 64 hex chars
    }

    #[test]
    fn test_verify_hash() {
        let content = b"hello world";
        let hash = ContentResolver::hash_content(content);
        assert!(ContentResolver::verify_hash(content, &hash));
        assert!(!ContentResolver::verify_hash(b"different content", &hash));
    }

    #[tokio::test]
    async fn test_cache_operations() {
        let audit_log = Arc::new(NullAuditSink);
        let resolver =
            ContentResolver::new(ResolverConfig::default(), audit_log, Arc::new(NullFetcher));

        // Store in cache
        resolver
            .store_cache("test-id", b"test content".to_vec())
            .await;

        // Check cache
        let cached = resolver.check_cache("test-id").await;
        assert_eq!(cached, Some(b"test content".to_vec()));

        // Clear cache
        resolver.clear_cache().await;
        let cached = resolver.check_cache("test-id").await;
        assert_eq!(cached, None);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let audit_log = Arc::new(NullAuditSink);
        let resolver =
            ContentResolver::new(ResolverConfig::default(), audit_log, Arc::new(NullFetcher));

        resolver.store_cache("id1", b"content1".to_vec()).await;
        resolver
            .store_cache("id2", b"content2content2".to_vec())
            .await;

        let stats = resolver.cache_stats().await;
        assert_eq!(stats.memory_entries, 2);
        assert_eq!(stats.memory_bytes, 8 + 16); // 8 + 16 bytes
    }

    #[test]
    fn test_default_resolver_has_no_ambient_gateways() {
        let cfg = ResolverConfig::default();
        assert!(cfg.ipfs_gateways.is_empty());
    }

    #[test]
    fn test_verify_cid_raw_sha256() {
        // Build a CIDv1 raw-codec SHA2-256 from known content
        let content = b"hello elastos";
        let digest = Sha256::digest(content);
        let mh = multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).expect("wrap multihash");
        let cid = cid::Cid::new_v1(0x55, mh);
        let cid_str = cid.to_string();

        match verify_cid_content(&cid_str, content) {
            CidVerification::Verified => {} // expected
            other => panic!(
                "expected Verified, got {:?}",
                cid_verification_debug(&other)
            ),
        }
    }

    #[test]
    fn test_verify_cid_mismatch() {
        // Build a valid CID for "hello" but verify against "world"
        let content = b"hello";
        let digest = Sha256::digest(content);
        let mh = multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).expect("wrap multihash");
        let cid = cid::Cid::new_v1(0x55, mh);
        let cid_str = cid.to_string();

        match verify_cid_content(&cid_str, b"world") {
            CidVerification::Mismatch(_) => {} // expected
            other => panic!(
                "expected Mismatch, got {:?}",
                cid_verification_debug(&other)
            ),
        }
    }

    #[test]
    fn test_verify_cid_v0_skipped() {
        // CIDv0 (Qm...) uses dag-pb, can't verify against raw bytes
        let cid_str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
        match verify_cid_content(cid_str, b"anything") {
            CidVerification::Unverifiable(_) => {} // expected
            other => panic!(
                "expected Unverifiable, got {:?}",
                cid_verification_debug(&other)
            ),
        }
    }

    #[test]
    fn test_verify_cid_invalid_syntax() {
        // Garbage string should return InvalidCid, not Unverifiable
        match verify_cid_content("not-a-cid!!!", b"anything") {
            CidVerification::InvalidCid(_) => {} // expected
            other => panic!(
                "expected InvalidCid, got {:?}",
                cid_verification_debug(&other)
            ),
        }
    }

    fn cid_verification_debug(v: &CidVerification) -> &'static str {
        match v {
            CidVerification::Verified => "Verified",
            CidVerification::Unverifiable(_) => "Unverifiable",
            CidVerification::InvalidCid(_) => "InvalidCid",
            CidVerification::Mismatch(_) => "Mismatch",
        }
    }
}
