//! Loopback-only IPFS gateway fetcher.
//!
//! The trusted server path may still use a loopback HTTP adapter here, but it
//! must not silently widen into arbitrary public URLs or generic local web
//! fetching. This adapter exists only for loopback `/ipfs/...` gateway reads.

use elastos_runtime::content::{ContentFetcher, FetchError};

use crate::local_http::validate_loopback_http_url;

/// Loopback `/ipfs/...` content fetcher using reqwest.
pub struct LoopbackIpfsGatewayFetcher {
    client: reqwest::Client,
}

impl LoopbackIpfsGatewayFetcher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for LoopbackIpfsGatewayFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ContentFetcher for LoopbackIpfsGatewayFetcher {
    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let parsed =
            validate_loopback_http_url(url).map_err(|e| FetchError::Network(e.to_string()))?;
        let path = parsed.path();
        if !path.starts_with("/ipfs/") || path == "/ipfs/" {
            return Err(FetchError::Network(format!(
                "loopback content fetcher only accepts /ipfs/<cid> URLs, got '{}'",
                url
            )));
        }

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| FetchError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(FetchError::NotFound);
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| FetchError::Network(e.to_string()))?;

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::LoopbackIpfsGatewayFetcher;
    use elastos_runtime::content::{ContentFetcher, FetchError};

    #[tokio::test]
    async fn rejects_non_loopback_http_urls() {
        let fetcher = LoopbackIpfsGatewayFetcher::new();
        let err = fetcher
            .fetch_url("https://example.com/ipfs/QmExample")
            .await
            .unwrap_err();

        assert!(
            matches!(err, FetchError::Network(message) if message.contains("non-loopback HTTP URL not allowed"))
        );
    }

    #[tokio::test]
    async fn rejects_non_ipfs_loopback_urls() {
        let fetcher = LoopbackIpfsGatewayFetcher::new();
        let err = fetcher
            .fetch_url("http://127.0.0.1:8080/not-ipfs/QmExample")
            .await
            .unwrap_err();

        assert!(
            matches!(err, FetchError::Network(message) if message.contains("only accepts /ipfs/<cid> URLs"))
        );
    }
}
