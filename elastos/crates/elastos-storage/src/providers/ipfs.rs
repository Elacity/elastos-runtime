//! IPFS storage provider

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::cache::ContentCache;
use crate::{ContentId, StorageProvider};
use elastos_common::{ElastosError, Result};

/// IPFS storage provider
///
/// Stores and retrieves content from IPFS via the HTTP API.
/// Uses a local cache to avoid repeated fetches.
pub struct IpfsProvider {
    /// IPFS API endpoint (e.g., "http://localhost:5001")
    api_url: String,
    /// HTTP client
    client: reqwest::Client,
    /// Local cache for fetched content
    cache: Arc<ContentCache>,
    /// Whether to use gateway mode (read-only, no add)
    gateway_mode: bool,
}

impl IpfsProvider {
    /// Create a new IPFS provider connected to a local node
    pub fn new(api_url: impl Into<String>, cache: Arc<ContentCache>) -> Self {
        Self {
            api_url: api_url.into(),
            client: reqwest::Client::new(),
            cache,
            gateway_mode: false,
        }
    }

    /// Create a provider that uses a public gateway (read-only)
    pub fn with_gateway(gateway_url: impl Into<String>, cache: Arc<ContentCache>) -> Self {
        Self {
            api_url: gateway_url.into(),
            client: reqwest::Client::new(),
            cache,
            gateway_mode: true,
        }
    }

    /// Check if the IPFS node is reachable
    pub async fn is_available(&self) -> bool {
        if self.gateway_mode {
            // For gateways, try to fetch a known CID
            return true; // Assume gateway is available
        }

        let url = format!("{}/api/v0/id", self.api_url);
        self.client.post(&url).send().await.is_ok()
    }

    /// Normalize a content identifier to its base CID form.
    fn normalize_cid(cid: &str) -> &str {
        cid.strip_prefix("elastos://").unwrap_or(cid)
    }
}

#[derive(Deserialize)]
struct AddResponse {
    #[serde(rename = "Hash")]
    hash: String,
}

#[derive(Deserialize)]
struct StatResponse {
    #[serde(rename = "Size")]
    size: u64,
}

#[async_trait]
impl StorageProvider for IpfsProvider {
    async fn put(&self, data: &[u8]) -> Result<ContentId> {
        if self.gateway_mode {
            return Err(ElastosError::Storage(
                "Cannot add content via gateway (read-only)".into(),
            ));
        }

        // Create multipart form with the file data
        let part = reqwest::multipart::Part::bytes(data.to_vec()).file_name("file");

        let form = reqwest::multipart::Form::new().part("file", part);

        let url = format!("{}/api/v0/add?pin=true", self.api_url);

        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ElastosError::Storage(format!("IPFS add failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(ElastosError::Storage(format!(
                "IPFS add failed ({}): {}",
                status, text
            )));
        }

        let result: AddResponse = response
            .json()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to parse IPFS response: {}", e)))?;

        let cid = ContentId::new(&result.hash);

        // Cache locally for faster retrieval
        self.cache.put(&cid, data).await?;

        tracing::info!("Added content to IPFS: {}", cid);

        Ok(cid)
    }

    async fn get(&self, id: &ContentId) -> Result<Vec<u8>> {
        let cid = Self::normalize_cid(id.as_str());

        // Check cache first
        if let Some(data) = self.cache.get(id).await? {
            tracing::debug!("Cache hit for {}", cid);
            return Ok(data);
        }

        tracing::debug!("Cache miss for {}, fetching from IPFS", cid);

        // Fetch from IPFS
        let url = if self.gateway_mode {
            // Gateway URL format: https://gateway.example.com/ipfs/Qm...
            format!("{}/ipfs/{}", self.api_url, cid)
        } else {
            // API URL format
            format!("{}/api/v0/cat?arg={}", self.api_url, cid)
        };

        let response = if self.gateway_mode {
            self.client.get(&url).send().await
        } else {
            self.client.post(&url).send().await
        }
        .map_err(|e| ElastosError::Storage(format!("IPFS cat failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ElastosError::CapsuleNotFound(format!(
                    "Content not found in IPFS: {}",
                    cid
                )));
            }
            let text = response.text().await.unwrap_or_default();
            return Err(ElastosError::Storage(format!(
                "IPFS cat failed ({}): {}",
                status, text
            )));
        }

        let data = response
            .bytes()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to read IPFS response: {}", e)))?
            .to_vec();

        // Cache for future use
        self.cache.put(id, &data).await?;

        Ok(data)
    }

    async fn exists(&self, id: &ContentId) -> Result<bool> {
        // Check cache first
        if self.cache.exists(id).await? {
            return Ok(true);
        }

        let cid = Self::normalize_cid(id.as_str());

        if self.gateway_mode {
            // For gateway, try a HEAD request
            let url = format!("{}/ipfs/{}", self.api_url, cid);
            let response = self.client.head(&url).send().await;
            return Ok(response.is_ok() && response.unwrap().status().is_success());
        }

        // For local node, use stat
        let url = format!("{}/api/v0/block/stat?arg={}", self.api_url, cid);
        let response = self.client.post(&url).send().await;

        Ok(response.is_ok() && response.unwrap().status().is_success())
    }

    async fn delete(&self, id: &ContentId) -> Result<()> {
        // IPFS content is immutable, we can only remove from local cache
        // and unpin from the local node
        self.cache.delete(id).await?;

        if !self.gateway_mode {
            let cid = Self::normalize_cid(id.as_str());
            let url = format!("{}/api/v0/pin/rm?arg={}", self.api_url, cid);

            // Unpinning may fail if not pinned, that's okay
            let _ = self.client.post(&url).send().await;

            tracing::info!("Unpinned content from IPFS: {}", cid);
        }

        Ok(())
    }

    async fn list(&self, prefix: Option<&str>) -> Result<Vec<ContentId>> {
        // IPFS doesn't have a global list operation
        // We can only list what's in our local cache
        self.cache.list(prefix).await
    }

    async fn size(&self, id: &ContentId) -> Result<u64> {
        // Check cache first
        if let Some(size) = self.cache.size(id).await? {
            return Ok(size);
        }

        let cid = Self::normalize_cid(id.as_str());

        if self.gateway_mode {
            // For gateway, we'd need to fetch the whole content to know size
            // Instead, return an error suggesting to fetch first
            return Err(ElastosError::Storage(
                "Cannot get size from gateway without fetching".into(),
            ));
        }

        let url = format!("{}/api/v0/block/stat?arg={}", self.api_url, cid);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| ElastosError::Storage(format!("IPFS stat failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "Content not found in IPFS: {}",
                cid
            )));
        }

        let stat: StatResponse = response
            .json()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to parse IPFS stat: {}", e)))?;

        Ok(stat.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // These tests require a running IPFS node
    // Run with: IPFS_TEST=1 cargo test ipfs -- --ignored

    async fn create_test_provider() -> Option<(IpfsProvider, tempfile::TempDir)> {
        if std::env::var("IPFS_TEST").is_err() {
            return None;
        }

        let dir = tempdir().unwrap();
        let cache = Arc::new(
            ContentCache::new(dir.path().to_path_buf(), 100)
                .await
                .unwrap(),
        );
        let provider = IpfsProvider::new("http://localhost:5001", cache);

        if !provider.is_available().await {
            return None;
        }

        Some((provider, dir))
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS node
    async fn test_ipfs_put_get() {
        let Some((provider, _dir)) = create_test_provider().await else {
            println!("Skipping test - IPFS not available");
            return;
        };

        let data = b"Hello, IPFS!";
        let cid = provider.put(data).await.unwrap();

        println!("Stored with CID: {}", cid);

        let retrieved = provider.get(&cid).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS node
    async fn test_ipfs_exists() {
        let Some((provider, _dir)) = create_test_provider().await else {
            return;
        };

        let data = b"Test exists";
        let cid = provider.put(data).await.unwrap();

        assert!(provider.exists(&cid).await.unwrap());

        let fake_cid = ContentId::new("QmFakeCidThatDoesNotExist1234567890");
        assert!(!provider.exists(&fake_cid).await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_integration() {
        let dir = tempdir().unwrap();
        let cache = Arc::new(
            ContentCache::new(dir.path().to_path_buf(), 100)
                .await
                .unwrap(),
        );

        // Create provider (will fail IPFS calls but cache should work)
        let provider = IpfsProvider::new("http://localhost:5001", cache.clone());

        // Pre-populate cache
        let cid = ContentId::new("QmTestCid123");
        cache.put(&cid, b"cached data").await.unwrap();

        // Should get from cache even if IPFS is down
        let data = provider.get(&cid).await.unwrap();
        assert_eq!(data, b"cached data");
    }
}
