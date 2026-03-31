//! Content-addressed identifier

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Content-addressed identifier for stored data
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentId(String);

impl ContentId {
    /// Create from raw hash string
    pub fn new(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    /// Create by hashing data (for local storage)
    pub fn from_data(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hasher.finalize();
        Self(format!("sha256:{}", hex::encode(hash)))
    }

    /// Get the raw ID string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to filesystem-safe name
    pub fn to_filename(&self) -> String {
        self.0.replace([':', '/'], "_")
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_id_from_data() {
        let data = b"hello world";
        let id = ContentId::from_data(data);
        assert!(id.as_str().starts_with("sha256:"));
    }

    #[test]
    fn test_content_id_deterministic() {
        let data = b"test data";
        let id1 = ContentId::from_data(data);
        let id2 = ContentId::from_data(data);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_to_filename() {
        let id = ContentId::new("sha256:abc123");
        assert_eq!(id.to_filename(), "sha256_abc123");
    }
}
