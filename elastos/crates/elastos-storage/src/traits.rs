//! Storage provider trait

use async_trait::async_trait;
use elastos_common::Result;

use crate::ContentId;

/// Abstract storage provider interface
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Store content, returns content-addressed ID
    async fn put(&self, data: &[u8]) -> Result<ContentId>;

    /// Retrieve content by ID
    async fn get(&self, id: &ContentId) -> Result<Vec<u8>>;

    /// Check if content exists
    async fn exists(&self, id: &ContentId) -> Result<bool>;

    /// Delete content
    async fn delete(&self, id: &ContentId) -> Result<()>;

    /// List contents with optional prefix filter
    async fn list(&self, prefix: Option<&str>) -> Result<Vec<ContentId>>;

    /// Get content size without fetching
    async fn size(&self, id: &ContentId) -> Result<u64>;
}
