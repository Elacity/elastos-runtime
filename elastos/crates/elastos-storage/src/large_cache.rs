//! Large file cache for rootfs images
//!
//! Provides LRU-based caching for large files (e.g., 2GB rootfs images)
//! with atomic writes and persistent index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use elastos_common::{ElastosError, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Default cache size limit: 50GB
const DEFAULT_CACHE_SIZE_BYTES: u64 = 50 * 1024 * 1024 * 1024;

/// Configuration for the large file cache
#[derive(Debug, Clone)]
pub struct LargeCacheConfig {
    /// Root directory for the cache
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes
    pub max_size_bytes: u64,
}

impl Default for LargeCacheConfig {
    fn default() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("elastos")
            .join("rootfs-cache");

        Self {
            cache_dir,
            max_size_bytes: DEFAULT_CACHE_SIZE_BYTES,
        }
    }
}

/// Entry in the cache index
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// CID of the content
    cid: String,
    /// Relative path within cache directory
    path: String,
    /// Size in bytes
    size: u64,
    /// Last access timestamp (Unix epoch seconds)
    last_accessed: u64,
}

/// Persistent cache index
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheIndex {
    /// Entries keyed by CID
    entries: HashMap<String, CacheEntry>,
    /// Total size of all cached files
    total_size: u64,
}

/// Large file cache with LRU eviction
pub struct LargeFileCache {
    config: LargeCacheConfig,
    index: RwLock<CacheIndex>,
}

impl LargeFileCache {
    /// Create a new large file cache
    pub async fn new(config: LargeCacheConfig) -> Result<Self> {
        // Create cache directory
        tokio::fs::create_dir_all(&config.cache_dir)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to create cache dir: {}", e)))?;

        // Load or create index
        let index = Self::load_index(&config.cache_dir).await?;

        Ok(Self {
            config,
            index: RwLock::new(index),
        })
    }

    /// Create with default configuration
    pub async fn with_defaults() -> Result<Self> {
        Self::new(LargeCacheConfig::default()).await
    }

    /// Load index from disk or create empty
    async fn load_index(cache_dir: &Path) -> Result<CacheIndex> {
        let index_path = cache_dir.join("index.json");

        if index_path.exists() {
            let data = tokio::fs::read_to_string(&index_path)
                .await
                .map_err(|e| ElastosError::Storage(format!("Failed to read cache index: {}", e)))?;

            serde_json::from_str(&data)
                .map_err(|e| ElastosError::Storage(format!("Failed to parse cache index: {}", e)))
        } else {
            Ok(CacheIndex::default())
        }
    }

    /// Save index to disk
    async fn save_index(&self) -> Result<()> {
        let index = self.index.read().await;
        let index_path = self.config.cache_dir.join("index.json");

        let data = serde_json::to_string_pretty(&*index).map_err(|e| {
            ElastosError::Storage(format!("Failed to serialize cache index: {}", e))
        })?;

        // Atomic write via temp file
        let temp_path = index_path.with_extension("json.tmp");
        tokio::fs::write(&temp_path, &data)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to write cache index: {}", e)))?;

        tokio::fs::rename(&temp_path, &index_path)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to rename cache index: {}", e)))?;

        Ok(())
    }

    /// Check if a CID is cached
    pub async fn contains(&self, cid: &str) -> bool {
        let index = self.index.read().await;
        if let Some(entry) = index.entries.get(cid) {
            // Verify file still exists
            let path = self.config.cache_dir.join(&entry.path);
            path.exists()
        } else {
            false
        }
    }

    /// Get the path to a cached file, updating access time
    pub async fn get(&self, cid: &str) -> Option<PathBuf> {
        let mut index = self.index.write().await;

        if let Some(entry) = index.entries.get_mut(cid) {
            let path = self.config.cache_dir.join(&entry.path);
            if path.exists() {
                // Update access time
                entry.last_accessed = current_timestamp();
                drop(index);
                let _ = self.save_index().await;
                return Some(path);
            }
        }

        None
    }

    /// Get the path where a CID should be cached (for writing)
    pub fn cache_path(&self, cid: &str) -> PathBuf {
        self.config.cache_dir.join(cid)
    }

    /// Get a temporary path for atomic download
    pub fn temp_path(&self, cid: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{}.downloading", cid))
    }

    /// Register a file in the cache after download
    ///
    /// This should be called after successfully downloading a file to the temp path
    /// and renaming it to the cache path.
    pub async fn register(&self, cid: &str, size: u64) -> Result<PathBuf> {
        let cache_path = self.cache_path(cid);
        let temp_path = self.temp_path(cid);

        // Move temp file to final location
        if temp_path.exists() {
            tokio::fs::rename(&temp_path, &cache_path)
                .await
                .map_err(|e| {
                    ElastosError::Storage(format!("Failed to finalize cached file: {}", e))
                })?;
        }

        // Verify the file exists
        if !cache_path.exists() {
            return Err(ElastosError::Storage(format!(
                "Cached file not found after registration: {}",
                cache_path.display()
            )));
        }

        // Evict old entries if needed
        self.evict_if_needed(size).await?;

        // Add to index
        let mut index = self.index.write().await;
        let entry = CacheEntry {
            cid: cid.to_string(),
            path: cid.to_string(),
            size,
            last_accessed: current_timestamp(),
        };

        index.total_size += size;
        index.entries.insert(cid.to_string(), entry);

        drop(index);
        self.save_index().await?;

        tracing::info!("Cached rootfs {} ({} MB)", cid, size / (1024 * 1024));

        Ok(cache_path)
    }

    /// Evict old entries to make room for new file
    async fn evict_if_needed(&self, new_size: u64) -> Result<()> {
        let mut index = self.index.write().await;

        // Check if eviction is needed
        if index.total_size + new_size <= self.config.max_size_bytes {
            return Ok(());
        }

        // Collect entries sorted by last_accessed (oldest first)
        let mut entries: Vec<_> = index.entries.values().cloned().collect();
        entries.sort_by_key(|e| e.last_accessed);

        let target_size = self.config.max_size_bytes.saturating_sub(new_size);
        let mut to_remove = Vec::new();

        for entry in entries {
            if index.total_size <= target_size {
                break;
            }

            to_remove.push(entry.cid.clone());
            index.total_size = index.total_size.saturating_sub(entry.size);
        }

        // Remove evicted entries
        for cid in &to_remove {
            if let Some(entry) = index.entries.remove(cid) {
                let path = self.config.cache_dir.join(&entry.path);
                if path.exists() {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::warn!("Failed to remove evicted cache file: {}", e);
                    } else {
                        tracing::info!("Evicted cached rootfs: {}", cid);
                    }
                }
            }
        }

        Ok(())
    }

    /// Remove a specific entry from the cache
    pub async fn remove(&self, cid: &str) -> Result<()> {
        let mut index = self.index.write().await;

        if let Some(entry) = index.entries.remove(cid) {
            index.total_size = index.total_size.saturating_sub(entry.size);

            let path = self.config.cache_dir.join(&entry.path);
            if path.exists() {
                tokio::fs::remove_file(&path).await.map_err(|e| {
                    ElastosError::Storage(format!("Failed to remove cache file: {}", e))
                })?;
            }

            drop(index);
            self.save_index().await?;
        }

        Ok(())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let index = self.index.read().await;
        CacheStats {
            entry_count: index.entries.len(),
            total_size: index.total_size,
            max_size: self.config.max_size_bytes,
        }
    }

    /// Get the cache directory path
    pub fn cache_dir(&self) -> &Path {
        &self.config.cache_dir
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entry_count: usize,
    pub total_size: u64,
    pub max_size: u64,
}

/// Get current Unix timestamp in seconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_large_cache_basic() {
        let temp = tempdir().unwrap();
        let config = LargeCacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size_bytes: 1024 * 1024 * 100, // 100MB
        };

        let cache = LargeFileCache::new(config).await.unwrap();

        // Initially empty
        assert!(!cache.contains("QmTest123").await);
        assert!(cache.get("QmTest123").await.is_none());
    }

    #[tokio::test]
    async fn test_large_cache_register() {
        let temp = tempdir().unwrap();
        let config = LargeCacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size_bytes: 1024 * 1024 * 100,
        };

        let cache = LargeFileCache::new(config).await.unwrap();

        let cid = "QmTestCid456";
        let test_data = b"test content for caching";

        // Write to temp path first
        let temp_path = cache.temp_path(cid);
        tokio::fs::write(&temp_path, test_data).await.unwrap();

        // Register the file
        let cached_path = cache.register(cid, test_data.len() as u64).await.unwrap();

        // Now it should be found
        assert!(cache.contains(cid).await);

        let retrieved_path = cache.get(cid).await.unwrap();
        assert_eq!(retrieved_path, cached_path);

        // Verify content
        let content = tokio::fs::read(&retrieved_path).await.unwrap();
        assert_eq!(content, test_data);
    }

    #[tokio::test]
    async fn test_large_cache_eviction() {
        let temp = tempdir().unwrap();
        let config = LargeCacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size_bytes: 300, // Very small for testing
        };

        let cache = LargeFileCache::new(config).await.unwrap();

        // Add first file (100 bytes)
        let cid1 = "QmFirst100";
        tokio::fs::write(cache.temp_path(cid1), vec![0u8; 100])
            .await
            .unwrap();
        cache.register(cid1, 100).await.unwrap();
        assert!(cache.contains(cid1).await);

        // Wait to ensure different timestamps (timestamps are in seconds)
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // Add second file (100 bytes)
        let cid2 = "QmSecond100";
        tokio::fs::write(cache.temp_path(cid2), vec![0u8; 100])
            .await
            .unwrap();
        cache.register(cid2, 100).await.unwrap();
        assert!(cache.contains(cid2).await);

        // Wait and access first to update its timestamp (making it newer than second)
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        cache.get(cid1).await;

        // Add third file (200 bytes) - should evict second (oldest access time)
        let cid3 = "QmThird200";
        tokio::fs::write(cache.temp_path(cid3), vec![0u8; 200])
            .await
            .unwrap();
        cache.register(cid3, 200).await.unwrap();

        // Third should exist (just added)
        assert!(cache.contains(cid3).await);
        // First should still exist (accessed more recently than second)
        assert!(cache.contains(cid1).await);
        // Second should be evicted (oldest access time)
        assert!(!cache.contains(cid2).await);
    }

    #[tokio::test]
    async fn test_large_cache_stats() {
        let temp = tempdir().unwrap();
        let config = LargeCacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size_bytes: 1024 * 1024,
        };

        let cache = LargeFileCache::new(config).await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.total_size, 0);

        // Add a file
        let cid = "QmStats";
        tokio::fs::write(cache.temp_path(cid), vec![0u8; 500])
            .await
            .unwrap();
        cache.register(cid, 500).await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(stats.entry_count, 1);
        assert_eq!(stats.total_size, 500);
    }
}
