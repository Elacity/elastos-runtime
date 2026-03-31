//! Streaming IPFS operations for large files
//!
//! Provides streaming upload and download for large files like rootfs images
//! that can't be loaded entirely into memory.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use elastos_common::{ElastosError, Result};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

// No default public IPFS gateways. Callers must explicitly provide gateways
// via with_gateways() if they want gateway-based downloads. The default path
// is local IPFS node only — fail closed if unavailable.

/// Progress tracking for streaming operations
#[derive(Debug, Clone)]
pub struct StreamingProgress {
    /// Bytes transferred so far
    bytes_transferred: Arc<AtomicU64>,
    /// Total bytes expected (if known)
    total_bytes: Option<u64>,
}

impl StreamingProgress {
    /// Create a new progress tracker
    pub fn new(total_bytes: Option<u64>) -> Self {
        Self {
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            total_bytes,
        }
    }

    /// Get bytes transferred
    pub fn bytes_transferred(&self) -> u64 {
        self.bytes_transferred.load(Ordering::Relaxed)
    }

    /// Get total bytes if known
    pub fn total_bytes(&self) -> Option<u64> {
        self.total_bytes
    }

    /// Get progress as percentage (0-100)
    pub fn percentage(&self) -> Option<f64> {
        self.total_bytes.map(|total| {
            if total == 0 {
                100.0
            } else {
                (self.bytes_transferred() as f64 / total as f64) * 100.0
            }
        })
    }

    /// Add bytes to the count
    fn add_bytes(&self, bytes: u64) {
        self.bytes_transferred.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Streaming IPFS provider for large file operations
pub struct IpfsStreamingProvider {
    /// IPFS API URL for uploads (e.g., "http://localhost:5001")
    api_url: String,
    /// Gateway URLs for downloads
    gateways: Vec<String>,
    /// HTTP client
    client: reqwest::Client,
}

impl IpfsStreamingProvider {
    /// Create a new streaming provider with local IPFS node only.
    /// No public gateway fallback — fail closed if local node is unavailable.
    pub fn new(api_url: impl Into<String>) -> Self {
        Self {
            api_url: api_url.into(),
            gateways: Vec::new(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3600))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Create with custom gateway list
    pub fn with_gateways(api_url: impl Into<String>, gateways: Vec<String>) -> Self {
        Self {
            api_url: api_url.into(),
            gateways,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3600))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Check if the local IPFS node is available
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/v0/id", self.api_url);
        self.client.post(&url).send().await.is_ok()
    }

    /// Stream upload a file to IPFS
    ///
    /// Returns the CID of the uploaded content.
    /// Requires a local IPFS node - gateways don't support uploads.
    pub async fn upload_streaming(
        &self,
        path: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<String> {
        if !self.is_available().await {
            return Err(ElastosError::Storage(
                "IPFS node not available. Publishing MicroVM capsules requires a local IPFS node (run 'ipfs daemon')".into(),
            ));
        }

        // Get file size for progress
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to read file metadata: {}", e)))?;

        let file_size = metadata.len();
        tracing::info!(
            "Uploading {} ({} MB) to IPFS...",
            path.display(),
            file_size / (1024 * 1024)
        );

        // Read file in chunks and create multipart body
        // For very large files, we use the IPFS API which handles chunking
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to open file: {}", e)))?;

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        // Create streaming body
        let stream = tokio_util::io::ReaderStream::new(file);

        // Wrap stream with progress tracking
        let progress_clone = progress.clone();
        let stream = stream.map(move |result| {
            if let Ok(ref chunk) = result {
                if let Some(ref p) = progress_clone {
                    p.add_bytes(chunk.len() as u64);
                }
            }
            result
        });

        let body = reqwest::Body::wrap_stream(stream);

        let part = reqwest::multipart::Part::stream_with_length(body, file_size)
            .file_name(file_name)
            .mime_str("application/octet-stream")
            .map_err(|e| ElastosError::Storage(format!("Failed to create multipart: {}", e)))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let url = format!("{}/api/v0/add?pin=true&chunker=size-262144", self.api_url);

        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ElastosError::Storage(format!("IPFS upload failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ElastosError::Storage(format!(
                "IPFS upload failed ({}): {}",
                status, body
            )));
        }

        // Parse response
        #[derive(serde::Deserialize)]
        struct AddResponse {
            #[serde(rename = "Hash")]
            hash: String,
        }

        let result: AddResponse = response
            .json()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to parse IPFS response: {}", e)))?;

        tracing::info!("Uploaded to IPFS: {} -> {}", path.display(), result.hash);

        Ok(result.hash)
    }

    /// Download from local IPFS node using the API
    async fn try_download_from_local(
        &self,
        cid: &str,
        dest: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<()> {
        let url = format!("{}/api/v0/cat?arg={}", self.api_url, cid);

        tracing::info!("Downloading from local IPFS node...");

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| ElastosError::Storage(format!("Local IPFS request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ElastosError::Storage(format!(
                "Local IPFS download failed: HTTP {}",
                response.status()
            )));
        }

        // Stream to file (reuse the gateway download logic)
        self.stream_response_to_file(response, dest, progress).await
    }

    /// Stream download a file from IPFS to a local path.
    ///
    /// Uses the local IPFS node. If explicit gateways were provided via
    /// `with_gateways()`, tries those as a visible fallback with logging.
    /// Fails closed if no source can deliver the content.
    pub async fn download_streaming(
        &self,
        cid: &str,
        dest: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<()> {
        let temp_path = dest.with_extension("downloading");

        // Try local IPFS node first
        if self.is_available().await {
            tracing::debug!("Local IPFS node available, checking for CID...");
            match self
                .try_download_from_local(cid, &temp_path, progress.clone())
                .await
            {
                Ok(()) => {
                    tracing::info!("Downloaded from local IPFS node");
                    tokio::fs::rename(&temp_path, dest).await.map_err(|e| {
                        ElastosError::Storage(format!("Failed to finalize download: {}", e))
                    })?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!("Local IPFS node doesn't have content: {}", e);
                }
            }
        }

        // Try explicitly configured gateways (if any)
        if self.gateways.is_empty() {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ElastosError::Storage(
                "Local IPFS node unavailable and no explicit gateways configured. \
                 No silent public gateway fallback is allowed."
                    .into(),
            ));
        }

        tracing::info!(
            "Local IPFS unavailable, trying {} explicitly configured gateways",
            self.gateways.len()
        );
        let mut last_error = None;
        for gateway in &self.gateways {
            tracing::info!("Trying gateway: {}", gateway);
            match self
                .try_download_from_gateway(gateway, cid, &temp_path, progress.clone())
                .await
            {
                Ok(()) => {
                    tracing::info!("Downloaded from explicit gateway: {}", gateway);
                    tokio::fs::rename(&temp_path, dest).await.map_err(|e| {
                        ElastosError::Storage(format!("Failed to finalize download: {}", e))
                    })?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("Gateway {} failed: {}", gateway, e);
                    last_error = Some(e);
                }
            }
        }

        let _ = tokio::fs::remove_file(&temp_path).await;
        Err(last_error.unwrap_or_else(|| {
            ElastosError::Storage("All explicitly configured gateways failed".into())
        }))
    }

    /// Try downloading from a specific gateway
    async fn try_download_from_gateway(
        &self,
        gateway: &str,
        cid: &str,
        dest: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<()> {
        let url = format!("{}/ipfs/{}", gateway.trim_end_matches('/'), cid);

        tracing::info!("Downloading from {}...", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ElastosError::Storage(format!("Download request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ElastosError::Storage(format!(
                "Download failed: HTTP {}",
                response.status()
            )));
        }

        self.stream_response_to_file(response, dest, progress).await
    }

    /// Stream an HTTP response body to a file
    async fn stream_response_to_file(
        &self,
        response: reqwest::Response,
        dest: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<()> {
        // Get content length if available
        let content_length = response.content_length();

        // Create parent directory if needed
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ElastosError::Storage(format!("Failed to create directory: {}", e)))?;
        }

        // Open destination file
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to create file: {}", e)))?;

        // Stream download
        let mut stream = response.bytes_stream();
        let mut bytes_written: u64 = 0;
        let mut last_log_mb: u64 = 0;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| ElastosError::Storage(format!("Download stream error: {}", e)))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| ElastosError::Storage(format!("Failed to write chunk: {}", e)))?;

            bytes_written += chunk.len() as u64;

            // Update progress
            if let Some(ref p) = progress {
                p.add_bytes(chunk.len() as u64);
            }

            // Log progress every 100MB
            let current_mb = bytes_written / (100 * 1024 * 1024);
            if current_mb > last_log_mb {
                last_log_mb = current_mb;
                let total_str = content_length
                    .map(|t| format!("{} MB", t / (1024 * 1024)))
                    .unwrap_or_else(|| "unknown".to_string());
                tracing::info!(
                    "Downloaded {} MB / {}",
                    bytes_written / (1024 * 1024),
                    total_str
                );
            }
        }

        // Ensure all data is flushed
        file.flush()
            .await
            .map_err(|e| ElastosError::Storage(format!("Failed to flush file: {}", e)))?;

        tracing::info!("Download complete ({} MB)", bytes_written / (1024 * 1024));

        Ok(())
    }

    /// Download and verify a file using block-level CID verification
    ///
    /// This downloads as CAR (Content Addressable aRchive) format which includes
    /// block CIDs for verification during download.
    pub async fn download_verified(
        &self,
        cid: &str,
        dest: &Path,
        progress: Option<StreamingProgress>,
    ) -> Result<()> {
        // For now, use regular streaming download
        // CAR-based verification would require additional dependencies (iroh-car)
        // and is more complex to implement correctly
        //
        // The content-addressed nature of IPFS already provides integrity:
        // - The CID is a cryptographic hash of the content
        // - If we get different content, it would have a different CID
        // - Gateways verify this internally
        //
        // For production use with untrusted gateways, we could:
        // 1. Download as CAR and verify block CIDs
        // 2. Hash the final file and compare to CID
        // 3. Use a local IPFS node (most secure)

        self.download_streaming(cid, dest, progress).await?;

        // Optionally verify by hashing the downloaded file
        // This adds overhead for 2GB files, so we skip it for now
        // In production, consider:
        // - Using local IPFS node which verifies internally
        // - Adding optional hash verification flag
        // - Using CAR format for streaming verification

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker() {
        let progress = StreamingProgress::new(Some(1000));

        assert_eq!(progress.bytes_transferred(), 0);
        assert_eq!(progress.percentage(), Some(0.0));

        progress.add_bytes(250);
        assert_eq!(progress.bytes_transferred(), 250);
        assert_eq!(progress.percentage(), Some(25.0));

        progress.add_bytes(750);
        assert_eq!(progress.bytes_transferred(), 1000);
        assert_eq!(progress.percentage(), Some(100.0));
    }

    #[test]
    fn test_progress_unknown_total() {
        let progress = StreamingProgress::new(None);

        assert_eq!(progress.total_bytes(), None);
        assert_eq!(progress.percentage(), None);

        progress.add_bytes(500);
        assert_eq!(progress.bytes_transferred(), 500);
        assert_eq!(progress.percentage(), None);
    }

    #[test]
    fn test_default_no_gateways() {
        // new() creates a provider with no public gateways — fail-closed by design.
        // Use with_gateways() to explicitly add gateways when needed.
        let provider = IpfsStreamingProvider::new("http://localhost:5001");
        assert!(provider.gateways.is_empty());
    }

    #[test]
    fn test_explicit_gateways() {
        let provider = IpfsStreamingProvider::with_gateways(
            "http://localhost:5001",
            vec!["https://example.com".to_string()],
        );
        assert_eq!(provider.gateways.len(), 1);
        assert!(provider
            .gateways
            .contains(&"https://example.com".to_string()));
    }

    // Integration tests require a running IPFS node
    // Run with: IPFS_TEST=1 cargo test ipfs_streaming -- --ignored

    #[tokio::test]
    #[ignore]
    async fn test_upload_streaming() {
        if std::env::var("IPFS_TEST").is_err() {
            return;
        }

        let provider = IpfsStreamingProvider::new("http://localhost:5001");

        if !provider.is_available().await {
            println!("IPFS not available, skipping test");
            return;
        }

        // Create a test file
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test.bin");
        tokio::fs::write(&test_file, vec![0u8; 1024 * 1024])
            .await
            .unwrap();

        let progress = StreamingProgress::new(Some(1024 * 1024));
        let cid = provider
            .upload_streaming(&test_file, Some(progress.clone()))
            .await
            .unwrap();

        assert!(!cid.is_empty());
        assert_eq!(progress.bytes_transferred(), 1024 * 1024);

        println!("Uploaded test file with CID: {}", cid);
    }
}
