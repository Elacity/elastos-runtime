//! Namespace persistence and content resolution
//!
//! Handles loading, saving, and caching of namespaces and their content.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::resolver::ContentResolver;

use crate::namespace::{ContentId, Namespace, NamespaceEntry, NamespaceError};

const LOCAL_NAMESPACE_ROOT_URI: &str = "localhost://Public";

/// Cache entry for content
#[derive(Debug)]
struct CacheEntry {
    /// The cached content
    content: Vec<u8>,
    /// Size in bytes
    size: u64,
}

/// Stores and retrieves namespaces, handles content caching
pub struct NamespaceStore {
    /// Local storage path for namespace data
    storage_path: PathBuf,

    /// Content resolver for fetching remote content
    content_resolver: Arc<ContentResolver>,

    /// Audit log for security events
    audit_log: Arc<dyn NamespaceAuditSink>,

    /// Current loaded namespace (per-owner)
    namespaces: RwLock<HashMap<String, Namespace>>,

    /// Content cache (CID -> content)
    content_cache: RwLock<HashMap<String, CacheEntry>>,

    /// Cache size limit in bytes (default 100MB)
    cache_limit: u64,
}

impl NamespaceStore {
    /// Create a new namespace store
    pub fn new(
        storage_path: PathBuf,
        content_resolver: Arc<ContentResolver>,
        audit_log: Arc<dyn NamespaceAuditSink>,
    ) -> Self {
        Self {
            storage_path,
            content_resolver,
            audit_log,
            namespaces: RwLock::new(HashMap::new()),
            content_cache: RwLock::new(HashMap::new()),
            cache_limit: 100 * 1024 * 1024, // 100MB
        }
    }

    /// Set the cache size limit
    pub fn with_cache_limit(mut self, limit: u64) -> Self {
        self.cache_limit = limit;
        self
    }

    /// Get the storage path for a namespace
    fn namespace_path(&self, owner: &str) -> PathBuf {
        self.storage_path.join(format!("{}.namespace.json", owner))
    }

    /// Load a namespace for a user (from local storage)
    pub async fn load(&self, owner: &str) -> Result<Namespace, NamespaceError> {
        // Check in-memory cache first
        {
            let namespaces = self.namespaces.read().await;
            if let Some(ns) = namespaces.get(owner) {
                return Ok(ns.clone());
            }
        }

        // Try loading from disk
        let path = self.namespace_path(owner);
        if path.exists() {
            let json = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| NamespaceError::IoError(e.to_string()))?;

            let namespace = Namespace::from_json(&json)?;

            // Cache in memory
            let mut namespaces = self.namespaces.write().await;
            namespaces.insert(owner.to_string(), namespace.clone());

            self.audit_log.namespace_loaded(owner);

            return Ok(namespace);
        }

        Err(NamespaceError::NotFound(format!("namespace for {}", owner)))
    }

    /// Load or create a namespace for a user
    pub async fn load_or_create(&self, owner: &str) -> Result<Namespace, NamespaceError> {
        match self.load(owner).await {
            Ok(ns) => Ok(ns),
            Err(NamespaceError::NotFound(_)) => {
                let namespace = Namespace::with_owner_hex(owner)?;

                // Save the new namespace
                self.save(&namespace).await?;

                self.audit_log.namespace_created(owner);

                Ok(namespace)
            }
            Err(e) => Err(e),
        }
    }

    /// Save a namespace (to local storage)
    pub async fn save(&self, namespace: &Namespace) -> Result<ContentId, NamespaceError> {
        // Ensure storage directory exists
        tokio::fs::create_dir_all(&self.storage_path)
            .await
            .map_err(|e| NamespaceError::IoError(e.to_string()))?;

        // Serialize to JSON
        let json = namespace.to_json()?;

        // Write to disk
        let path = self.namespace_path(&namespace.owner);
        tokio::fs::write(&path, &json)
            .await
            .map_err(|e| NamespaceError::IoError(e.to_string()))?;

        // Update in-memory cache
        let mut namespaces = self.namespaces.write().await;
        namespaces.insert(namespace.owner.clone(), namespace.clone());

        // Compute and return the namespace CID
        let cid = namespace.to_cid()?;

        self.audit_log
            .namespace_saved(&namespace.owner, &cid.to_string());

        Ok(cid)
    }

    /// Get the current namespace for an owner (from cache)
    pub async fn current(&self, owner: &str) -> Option<Namespace> {
        let namespaces = self.namespaces.read().await;
        namespaces.get(owner).cloned()
    }

    /// Check if content is cached locally
    pub async fn is_cached(&self, cid: &ContentId) -> bool {
        let cache = self.content_cache.read().await;
        cache.contains_key(cid.raw())
    }

    /// Get cached content size
    pub async fn cached_size(&self, cid: &ContentId) -> Option<u64> {
        let cache = self.content_cache.read().await;
        cache.get(cid.raw()).map(|e| e.size)
    }

    /// Read content by CID (from cache or fetch)
    pub async fn read_content(&self, cid: &ContentId) -> Result<Vec<u8>, NamespaceError> {
        // Check cache first
        {
            let cache = self.content_cache.read().await;
            if let Some(entry) = cache.get(cid.raw()) {
                return Ok(entry.content.clone());
            }
        }

        // Fetch from content resolver
        let uri = format!("elastos://{}", cid.raw());
        let result = self
            .content_resolver
            .fetch(&uri)
            .await
            .map_err(|e| NamespaceError::FetchError(e.to_string()))?;

        // Cache the content
        self.cache_content(cid, result.content.clone()).await;

        Ok(result.content)
    }

    /// Store content and return its CID
    pub async fn store_content(&self, content: &[u8]) -> Result<ContentId, NamespaceError> {
        let cid = ContentId::from_content(content);

        // Cache locally
        self.cache_content(&cid, content.to_vec()).await;

        // TODO: In Phase 7, optionally publish to IPFS

        Ok(cid)
    }

    /// Cache content locally
    async fn cache_content(&self, cid: &ContentId, content: Vec<u8>) {
        let size = content.len() as u64;

        // Check if we need to evict
        self.maybe_evict_cache(size).await;

        let mut cache = self.content_cache.write().await;
        cache.insert(cid.raw().to_string(), CacheEntry { content, size });
    }

    /// Evict cache entries if needed to make room
    async fn maybe_evict_cache(&self, needed: u64) {
        let mut cache = self.content_cache.write().await;

        let current_size: u64 = cache.values().map(|e| e.size).sum();
        if current_size + needed <= self.cache_limit {
            return;
        }

        // Simple LRU-ish eviction: remove entries until we have space
        // In production, we'd use a proper LRU cache
        let to_remove: u64 = (current_size + needed).saturating_sub(self.cache_limit);
        let mut removed: u64 = 0;
        let mut keys_to_remove = Vec::new();

        for (key, entry) in cache.iter() {
            if removed >= to_remove {
                break;
            }
            keys_to_remove.push(key.clone());
            removed += entry.size;
        }

        for key in keys_to_remove {
            cache.remove(&key);
        }
    }

    /// Prefetch content by CIDs (warm the cache)
    pub async fn prefetch(&self, cids: &[ContentId]) -> PrefetchResult {
        let mut fetched = 0;
        let mut failed = 0;
        let mut errors = Vec::new();

        for cid in cids {
            if self.is_cached(cid).await {
                continue;
            }

            match self.read_content(cid).await {
                Ok(_) => fetched += 1,
                Err(e) => {
                    failed += 1;
                    errors.push((cid.clone(), e.to_string()));
                }
            }
        }

        PrefetchResult {
            fetched,
            failed,
            errors,
        }
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> CacheStats {
        let cache = self.content_cache.read().await;
        let entry_count = cache.len();
        let total_bytes: u64 = cache.values().map(|e| e.size).sum();

        CacheStats {
            entry_count,
            total_bytes,
            limit_bytes: self.cache_limit,
        }
    }

    /// Clear the content cache
    pub async fn clear_cache(&self) {
        let mut cache = self.content_cache.write().await;
        cache.clear();
    }

    /// Resolve a path in a namespace and get the content
    pub async fn read_path(&self, owner: &str, path: &str) -> Result<ReadResult, NamespaceError> {
        let namespace = self.load(owner).await?;

        let entry = namespace
            .resolve(path)
            .ok_or_else(|| NamespaceError::NotFound(path.to_string()))?;

        match entry {
            NamespaceEntry::File {
                cid,
                size,
                content_type,
                ..
            } => {
                let cached = self.is_cached(cid).await;
                let content = self.read_content(cid).await?;

                Ok(ReadResult {
                    content,
                    cid: cid.clone(),
                    size: *size,
                    content_type: content_type.clone(),
                    was_cached: cached,
                })
            }
            NamespaceEntry::Directory { .. } => Err(NamespaceError::NotAFile(path.to_string())),
        }
    }

    /// Write content to a path in a namespace
    pub async fn write_path(
        &self,
        owner: &str,
        path: &str,
        content: &[u8],
        content_type: Option<String>,
    ) -> Result<WriteResult, NamespaceError> {
        // Store the content and get CID
        let cid = self.store_content(content).await?;
        let size = content.len() as u64;

        // Update namespace
        let mut namespace = self.load_or_create(owner).await?;

        let entry = NamespaceEntry::file(cid.clone(), size, content_type);
        namespace.put(path, entry)?;

        // Save updated namespace
        let namespace_cid = self.save(&namespace).await?;

        Ok(WriteResult {
            path: path.to_string(),
            cid,
            size,
            namespace_cid,
        })
    }

    /// Delete a path from a namespace
    pub async fn delete_path(
        &self,
        owner: &str,
        path: &str,
    ) -> Result<DeleteResult, NamespaceError> {
        let mut namespace = self.load(owner).await?;

        namespace.remove(path)?;

        let namespace_cid = self.save(&namespace).await?;

        Ok(DeleteResult {
            path: path.to_string(),
            namespace_cid,
        })
    }

    /// List entries at a path
    pub async fn list_path(
        &self,
        owner: &str,
        path: &str,
    ) -> Result<Vec<EntryInfo>, NamespaceError> {
        let namespace = self.load(owner).await?;

        let entries = namespace.list(path)?;

        let mut result = Vec::new();
        for (name, entry) in entries {
            let full_path = if path.is_empty() || path == "/" {
                format!("{}/{}", LOCAL_NAMESPACE_ROOT_URI, name)
            } else {
                format!(
                    "{}/{}/{}",
                    LOCAL_NAMESPACE_ROOT_URI,
                    path.trim_matches('/'),
                    name
                )
            };

            let (cid, size, content_type) = match entry {
                NamespaceEntry::File {
                    cid,
                    size,
                    content_type,
                    ..
                } => (Some(cid.clone()), *size, content_type.clone()),
                NamespaceEntry::Directory { .. } => (None, entry.size(), None),
            };

            let cached = if let Some(ref c) = cid {
                self.is_cached(c).await
            } else {
                true // Directories are always "cached"
            };

            result.push(EntryInfo {
                name: name.to_string(),
                path: full_path,
                entry_type: if entry.is_file() {
                    "file".into()
                } else {
                    "directory".into()
                },
                cid,
                size,
                content_type,
                modified_at: entry.modified_at().unix_secs,
                cached,
            });
        }

        Ok(result)
    }

    /// Get namespace status
    pub async fn namespace_status(&self, owner: &str) -> Result<NamespaceStatus, NamespaceError> {
        let namespace = self.load(owner).await?;
        let namespace_cid = namespace.to_cid()?;

        // Calculate cached size by checking each file
        let cached_size = self.calculate_cached_size(&namespace.root).await;

        Ok(NamespaceStatus {
            owner: namespace.owner.clone(),
            namespace_cid,
            entry_count: namespace.entry_count(),
            total_size: namespace.total_size(),
            cached_size,
            last_modified: namespace.modified_at.unix_secs,
            signed: namespace.signature.is_some(),
        })
    }

    /// Calculate the size of cached content in a namespace entry tree
    async fn calculate_cached_size(&self, entry: &NamespaceEntry) -> u64 {
        match entry {
            NamespaceEntry::File { cid, size, .. } => {
                if self.is_cached(cid).await {
                    *size
                } else {
                    0
                }
            }
            NamespaceEntry::Directory { children, .. } => {
                let mut total = 0u64;
                for child in children.values() {
                    // Use Box::pin for recursive async
                    total += Box::pin(self.calculate_cached_size(child)).await;
                }
                total
            }
        }
    }
}

/// Result of reading content
#[derive(Debug)]
pub struct ReadResult {
    /// The content bytes
    pub content: Vec<u8>,
    /// Content ID
    pub cid: ContentId,
    /// Size in bytes
    pub size: u64,
    /// Content type (MIME)
    pub content_type: Option<String>,
    /// Whether content was in cache (vs fetched)
    pub was_cached: bool,
}

/// Result of writing content
#[derive(Debug)]
pub struct WriteResult {
    /// Path that was written
    pub path: String,
    /// Content ID of the new content
    pub cid: ContentId,
    /// Size in bytes
    pub size: u64,
    /// New namespace CID after the write
    pub namespace_cid: ContentId,
}

/// Result of deleting a path
#[derive(Debug)]
pub struct DeleteResult {
    /// Path that was deleted
    pub path: String,
    /// New namespace CID after deletion
    pub namespace_cid: ContentId,
}

/// Information about a namespace entry
#[derive(Debug, Clone)]
pub struct EntryInfo {
    /// Entry name
    pub name: String,
    /// Full path (for example `localhost://Public/...`)
    pub path: String,
    /// Type: "file" or "directory"
    pub entry_type: String,
    /// Content ID (for files)
    pub cid: Option<ContentId>,
    /// Size in bytes
    pub size: u64,
    /// Content type (for files)
    pub content_type: Option<String>,
    /// Modification timestamp (unix secs)
    pub modified_at: u64,
    /// Whether content is cached locally
    pub cached: bool,
}

/// Result of prefetching content
#[derive(Debug)]
pub struct PrefetchResult {
    /// Number of CIDs successfully fetched
    pub fetched: usize,
    /// Number of CIDs that failed to fetch
    pub failed: usize,
    /// Errors for failed fetches
    pub errors: Vec<(ContentId, String)>,
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of cached entries
    pub entry_count: usize,
    /// Total bytes cached
    pub total_bytes: u64,
    /// Cache size limit
    pub limit_bytes: u64,
}

/// Namespace status information
#[derive(Debug, Clone)]
pub struct NamespaceStatus {
    /// Owner's public key (hex)
    pub owner: String,
    /// Namespace content ID
    pub namespace_cid: ContentId,
    /// Total number of entries
    pub entry_count: u64,
    /// Total size of all content
    pub total_size: u64,
    /// Size of locally cached content
    pub cached_size: u64,
    /// Last modification timestamp
    pub last_modified: u64,
    /// Whether namespace is signed
    pub signed: bool,
}

/// Trait for namespace-specific audit events
pub trait NamespaceAuditSink: Send + Sync {
    fn namespace_loaded(&self, owner: &str);
    fn namespace_created(&self, owner: &str);
    fn namespace_saved(&self, owner: &str, cid: &str);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{NullFetcher, ResolverConfig};
    use tempfile::tempdir;

    struct TestAuditSink;
    impl crate::resolver::AuditSink for TestAuditSink {
        fn content_fetch(&self, _: &str, _: crate::resolver::FetchSource, _: bool) {}
    }
    impl NamespaceAuditSink for TestAuditSink {
        fn namespace_loaded(&self, _: &str) {}
        fn namespace_created(&self, _: &str) {}
        fn namespace_saved(&self, _: &str, _: &str) {}
    }

    async fn create_test_store() -> (NamespaceStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let audit_sink: Arc<TestAuditSink> = Arc::new(TestAuditSink);
        let content_resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            audit_sink.clone() as Arc<dyn crate::resolver::AuditSink>,
            Arc::new(NullFetcher),
        ));

        let store = NamespaceStore::new(
            dir.path().to_path_buf(),
            content_resolver,
            audit_sink as Arc<dyn NamespaceAuditSink>,
        );

        (store, dir)
    }

    #[tokio::test]
    async fn test_store_create_and_load() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        // Create namespace
        let ns = store.load_or_create(owner).await.unwrap();
        assert_eq!(ns.owner, owner);

        // Load it back
        let ns2 = store.load(owner).await.unwrap();
        assert_eq!(ns2.owner, owner);
    }

    #[tokio::test]
    async fn test_store_write_and_read() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        // Write content
        let content = b"hello world";
        let result = store
            .write_path(owner, "test.txt", content, Some("text/plain".into()))
            .await
            .unwrap();

        assert_eq!(result.size, content.len() as u64);
        assert!(result.cid.is_sha256());

        // Read it back
        let read_result = store.read_path(owner, "test.txt").await.unwrap();
        assert_eq!(read_result.content, content);
        assert_eq!(read_result.content_type, Some("text/plain".into()));
    }

    #[tokio::test]
    async fn test_store_list_path() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        store
            .write_path(owner, "photos/a.jpg", b"aaa", None)
            .await
            .unwrap();
        store
            .write_path(owner, "photos/b.jpg", b"bbb", None)
            .await
            .unwrap();

        let entries = store.list_path(owner, "photos").await.unwrap();
        assert_eq!(entries.len(), 2);

        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a.jpg"));
        assert!(names.contains(&"b.jpg"));
    }

    #[tokio::test]
    async fn test_store_delete_path() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        store
            .write_path(owner, "test.txt", b"content", None)
            .await
            .unwrap();

        // Verify it exists
        let result = store.read_path(owner, "test.txt").await;
        assert!(result.is_ok());

        // Delete it
        store.delete_path(owner, "test.txt").await.unwrap();

        // Verify it's gone
        let result = store.read_path(owner, "test.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_cache_stats() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        store
            .write_path(owner, "a.txt", b"hello", None)
            .await
            .unwrap();
        store
            .write_path(owner, "b.txt", b"world!", None)
            .await
            .unwrap();

        let stats = store.cache_stats().await;
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.total_bytes, 11); // "hello" + "world!"
    }

    #[tokio::test]
    async fn test_store_namespace_status() {
        let (store, _dir) = create_test_store().await;

        let owner = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        store
            .write_path(owner, "a.txt", b"hello", None)
            .await
            .unwrap();
        store
            .write_path(owner, "dir/b.txt", b"world!", None)
            .await
            .unwrap();

        let status = store.namespace_status(owner).await.unwrap();
        assert_eq!(status.owner, owner);
        assert_eq!(status.entry_count, 2);
        assert_eq!(status.total_size, 11);
        assert_eq!(status.cached_size, 11); // All content is cached
        assert!(!status.signed);
    }

    #[tokio::test]
    async fn test_store_is_cached() {
        let (store, _dir) = create_test_store().await;

        let content = b"test content";
        let cid = store.store_content(content).await.unwrap();

        assert!(store.is_cached(&cid).await);

        let unknown_cid = ContentId::from_content(b"unknown");
        assert!(!store.is_cached(&unknown_cid).await);
    }
}
