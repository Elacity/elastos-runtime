//! Content cache with LRU eviction

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;

use lru::LruCache;
use tokio::fs;

use crate::ContentId;
use elastos_common::Result;

/// Content cache with LRU eviction
pub struct ContentCache {
    /// Directory for cached content
    cache_dir: PathBuf,
    /// Maximum cache size in bytes
    max_size: u64,
    /// Current cache size
    current_size: Mutex<u64>,
    /// LRU tracking for eviction (maps ContentId to size)
    lru: Mutex<LruCache<String, u64>>,
}

impl ContentCache {
    /// Create a new content cache
    pub async fn new(cache_dir: PathBuf, max_size_mb: u64) -> Result<Self> {
        fs::create_dir_all(&cache_dir).await?;

        let cache = Self {
            cache_dir,
            max_size: max_size_mb * 1024 * 1024,
            current_size: Mutex::new(0),
            lru: Mutex::new(LruCache::new(NonZeroUsize::new(10000).unwrap())),
        };

        // Scan existing cache
        cache.scan_existing().await?;

        Ok(cache)
    }

    /// Scan existing cache directory and populate LRU
    async fn scan_existing(&self) -> Result<()> {
        let mut entries = match fs::read_dir(&self.cache_dir).await {
            Ok(e) => e,
            Err(_) => return Ok(()), // Directory doesn't exist yet
        };

        let mut total_size = 0u64;
        let mut cache_entries = Vec::new();

        // Collect entries first without holding the lock
        while let Some(entry) = entries.next_entry().await? {
            if let Ok(metadata) = entry.metadata().await {
                let size = metadata.len();
                let name = entry.file_name().to_string_lossy().to_string();
                cache_entries.push((name, size));
                total_size += size;
            }
        }

        // Now populate the LRU with the lock held (no await points)
        {
            let mut lru = self.lru.lock().unwrap();
            for (name, size) in cache_entries {
                lru.put(name, size);
            }
        }
        *self.current_size.lock().unwrap() = total_size;

        tracing::info!(
            "Cache initialized: {} bytes in {} items",
            total_size,
            self.lru.lock().unwrap().len()
        );

        Ok(())
    }

    /// Store content in cache
    pub async fn put(&self, id: &ContentId, data: &[u8]) -> Result<()> {
        let size = data.len() as u64;

        // Evict if necessary
        self.ensure_space(size).await?;

        let path = self.content_path(id);
        fs::write(&path, data).await?;

        let mut current = self.current_size.lock().unwrap();
        *current += size;

        let mut lru = self.lru.lock().unwrap();
        lru.put(id.to_filename(), size);

        Ok(())
    }

    /// Get content from cache
    pub async fn get(&self, id: &ContentId) -> Result<Option<Vec<u8>>> {
        let path = self.content_path(id);

        if path.exists() {
            // Touch LRU to mark as recently used
            {
                let mut lru = self.lru.lock().unwrap();
                lru.get(&id.to_filename());
            }

            let data = fs::read(&path).await?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    /// Check if content exists in cache
    pub async fn exists(&self, id: &ContentId) -> Result<bool> {
        let path = self.content_path(id);
        Ok(path.exists())
    }

    /// Delete content from cache
    pub async fn delete(&self, id: &ContentId) -> Result<()> {
        let path = self.content_path(id);

        if path.exists() {
            let metadata = fs::metadata(&path).await?;
            let size = metadata.len();

            fs::remove_file(&path).await?;

            let mut current = self.current_size.lock().unwrap();
            *current = current.saturating_sub(size);

            let mut lru = self.lru.lock().unwrap();
            lru.pop(&id.to_filename());
        }

        Ok(())
    }

    /// List all content IDs in cache
    pub async fn list(&self, prefix: Option<&str>) -> Result<Vec<ContentId>> {
        let mut entries = fs::read_dir(&self.cache_dir).await?;
        let mut ids = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            // Convert filename back to content ID format
            let id_str = name.replace("_", ":");

            if let Some(p) = prefix {
                if id_str.starts_with(p) {
                    ids.push(ContentId::new(id_str));
                }
            } else {
                ids.push(ContentId::new(id_str));
            }
        }

        Ok(ids)
    }

    /// Get size of content if it exists in cache
    pub async fn size(&self, id: &ContentId) -> Result<Option<u64>> {
        let path = self.content_path(id);

        if path.exists() {
            let metadata = fs::metadata(&path).await?;
            Ok(Some(metadata.len()))
        } else {
            Ok(None)
        }
    }

    /// Get current cache size in bytes
    pub fn current_size(&self) -> u64 {
        *self.current_size.lock().unwrap()
    }

    /// Get maximum cache size in bytes
    pub fn max_size(&self) -> u64 {
        self.max_size
    }

    /// Ensure there's enough space for new content
    async fn ensure_space(&self, needed: u64) -> Result<()> {
        loop {
            let current = *self.current_size.lock().unwrap();

            if current + needed <= self.max_size {
                break;
            }

            // Need to evict
            let to_evict = {
                let mut lru = self.lru.lock().unwrap();
                lru.pop_lru()
            };

            if let Some((name, size)) = to_evict {
                let path = self.cache_dir.join(&name);
                if path.exists() {
                    fs::remove_file(&path).await?;
                    tracing::debug!("Evicted {} ({} bytes) from cache", name, size);
                }

                let mut current = self.current_size.lock().unwrap();
                *current = current.saturating_sub(size);
            } else {
                // Nothing left to evict
                break;
            }
        }

        Ok(())
    }

    /// Get the filesystem path for a content ID
    fn content_path(&self, id: &ContentId) -> PathBuf {
        self.cache_dir.join(id.to_filename())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_cache_put_get() {
        let dir = tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf(), 10)
            .await
            .unwrap();

        let id = ContentId::from_data(b"test data");
        cache.put(&id, b"test data").await.unwrap();

        let data = cache.get(&id).await.unwrap();
        assert_eq!(data, Some(b"test data".to_vec()));
    }

    #[tokio::test]
    async fn test_cache_exists() {
        let dir = tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf(), 10)
            .await
            .unwrap();

        let id = ContentId::from_data(b"test");
        assert!(!cache.exists(&id).await.unwrap());

        cache.put(&id, b"test").await.unwrap();
        assert!(cache.exists(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_delete() {
        let dir = tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf(), 10)
            .await
            .unwrap();

        let id = ContentId::from_data(b"test");
        cache.put(&id, b"test").await.unwrap();
        assert!(cache.exists(&id).await.unwrap());

        cache.delete(&id).await.unwrap();
        assert!(!cache.exists(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let dir = tempdir().unwrap();
        // 1MB max cache
        let cache = ContentCache::new(dir.path().to_path_buf(), 1)
            .await
            .unwrap();

        // Add items until we exceed capacity
        let data = vec![0u8; 500 * 1024]; // 500KB each

        let id1 = ContentId::new("sha256:1111");
        let id2 = ContentId::new("sha256:2222");
        let id3 = ContentId::new("sha256:3333");

        cache.put(&id1, &data).await.unwrap();
        cache.put(&id2, &data).await.unwrap();

        // Both should exist
        assert!(cache.exists(&id1).await.unwrap());
        assert!(cache.exists(&id2).await.unwrap());

        // Adding a third should evict the first (LRU)
        cache.put(&id3, &data).await.unwrap();

        // id1 should be evicted
        assert!(!cache.exists(&id1).await.unwrap());
        assert!(cache.exists(&id2).await.unwrap());
        assert!(cache.exists(&id3).await.unwrap());
    }
}
