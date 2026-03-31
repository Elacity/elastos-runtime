//! Local filesystem storage provider

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

use elastos_common::{ElastosError, Result};

use crate::{ContentId, StorageProvider};

/// Storage provider that uses the local filesystem
pub struct LocalFSProvider {
    base_path: PathBuf,
}

impl LocalFSProvider {
    /// Create a new LocalFSProvider with the given base path
    pub async fn new(base_path: impl Into<PathBuf>) -> Result<Self> {
        let base_path = base_path.into();
        fs::create_dir_all(&base_path).await?;
        Ok(Self { base_path })
    }

    fn content_path(&self, id: &ContentId) -> PathBuf {
        self.base_path.join(id.to_filename())
    }
}

#[async_trait]
impl StorageProvider for LocalFSProvider {
    async fn put(&self, data: &[u8]) -> Result<ContentId> {
        let id = ContentId::from_data(data);
        let path = self.content_path(&id);
        fs::write(&path, data).await?;
        Ok(id)
    }

    async fn get(&self, id: &ContentId) -> Result<Vec<u8>> {
        let path = self.content_path(id);
        fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ElastosError::CapsuleNotFound(id.to_string())
            } else {
                ElastosError::Io(e)
            }
        })
    }

    async fn exists(&self, id: &ContentId) -> Result<bool> {
        let path = self.content_path(id);
        Ok(path.exists())
    }

    async fn delete(&self, id: &ContentId) -> Result<()> {
        let path = self.content_path(id);
        fs::remove_file(&path).await?;
        Ok(())
    }

    async fn list(&self, prefix: Option<&str>) -> Result<Vec<ContentId>> {
        let mut entries = fs::read_dir(&self.base_path).await?;
        let mut ids = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            // Convert filename back to content ID format
            let id = name.replace('_', ":");

            if let Some(p) = prefix {
                if id.starts_with(p) {
                    ids.push(ContentId::new(id));
                }
            } else {
                ids.push(ContentId::new(id));
            }
        }

        Ok(ids)
    }

    async fn size(&self, id: &ContentId) -> Result<u64> {
        let path = self.content_path(id);
        let metadata = fs::metadata(&path).await?;
        Ok(metadata.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_put_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let provider = LocalFSProvider::new(temp_dir.path()).await.unwrap();

        let data = b"hello world";
        let id = provider.put(data).await.unwrap();

        let retrieved = provider.get(&id).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let provider = LocalFSProvider::new(temp_dir.path()).await.unwrap();

        let data = b"test";
        let id = provider.put(data).await.unwrap();

        assert!(provider.exists(&id).await.unwrap());

        let nonexistent = ContentId::new("sha256:nonexistent");
        assert!(!provider.exists(&nonexistent).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let provider = LocalFSProvider::new(temp_dir.path()).await.unwrap();

        let data = b"to delete";
        let id = provider.put(data).await.unwrap();
        assert!(provider.exists(&id).await.unwrap());

        provider.delete(&id).await.unwrap();
        assert!(!provider.exists(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_list() {
        let temp_dir = TempDir::new().unwrap();
        let provider = LocalFSProvider::new(temp_dir.path()).await.unwrap();

        provider.put(b"one").await.unwrap();
        provider.put(b"two").await.unwrap();
        provider.put(b"three").await.unwrap();

        let all = provider.list(None).await.unwrap();
        assert_eq!(all.len(), 3);
    }
}
