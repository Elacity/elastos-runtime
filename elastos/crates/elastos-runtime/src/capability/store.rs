//! Capability token storage and revocation
//!
//! Manages:
//! - Global revocation epoch (for mass revocation)
//! - Per-token use counts (for use-limited tokens)
//! - Revocation list (for individual token revocation)

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

use super::token::TokenId;

/// Capability store - manages token state
///
/// Owns the revocation epoch as an instance field (not process-global),
/// which makes it testable without `serial_test` and ready for
/// multi-runtime scenarios.
pub struct CapabilityStore {
    /// Revocation epoch — all tokens with epoch < this value are rejected.
    epoch: AtomicU64,

    /// Per-token use counts
    use_counts: RwLock<HashMap<TokenId, u32>>,

    /// Individually revoked token IDs
    revoked_tokens: RwLock<HashSet<TokenId>>,

    /// Path for persistence (optional)
    storage_path: Option<PathBuf>,
}

impl CapabilityStore {
    /// Create a new in-memory capability store
    pub fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            use_counts: RwLock::new(HashMap::new()),
            revoked_tokens: RwLock::new(HashSet::new()),
            storage_path: None,
        }
    }

    /// Create a capability store with persistence
    pub async fn with_persistence(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Create directory if needed
        fs::create_dir_all(&path)?;

        let store = Self {
            epoch: AtomicU64::new(0),
            use_counts: RwLock::new(HashMap::new()),
            revoked_tokens: RwLock::new(HashSet::new()),
            storage_path: Some(path.clone()),
        };

        // Load persisted state
        store.load_state().await?;

        Ok(store)
    }

    /// Get the current revocation epoch
    pub fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Advance the epoch (mass revocation)
    ///
    /// All tokens with epoch < new_epoch will be rejected.
    /// Persists BEFORE updating in-memory to prevent crash-revive:
    /// if the process dies after persist but before in-memory update,
    /// the next startup loads the higher epoch and tokens stay revoked.
    /// Returns the new epoch value.
    pub fn advance_epoch(&self) -> u64 {
        let new_epoch = self.epoch.load(Ordering::SeqCst) + 1;

        // Persist FIRST — crash-safe ordering
        if let Some(path) = &self.storage_path {
            let epoch_path = path.join("epoch");
            let tmp_path = path.join("epoch.tmp");
            if let Err(e) = fs::write(&tmp_path, new_epoch.to_string())
                .and_then(|_| fs::rename(&tmp_path, &epoch_path))
            {
                tracing::error!(
                    "CRITICAL: Failed to persist epoch to {}: {}. Epoch advance aborted.",
                    epoch_path.display(),
                    e
                );
                // Do NOT advance in-memory if persistence failed —
                // better to fail the revocation than to have in-memory
                // and on-disk diverge.
                return self.epoch.load(Ordering::SeqCst);
            }
        }

        // Only update in-memory after successful persistence
        self.epoch.store(new_epoch, Ordering::SeqCst);
        new_epoch
    }

    /// Set the epoch to a specific value (for initialization/loading)
    pub fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::SeqCst);
    }

    /// Check if a token's epoch is valid
    pub fn is_epoch_valid(&self, token_epoch: u64) -> bool {
        token_epoch >= self.current_epoch()
    }

    /// Get the use count for a token
    pub async fn get_use_count(&self, token_id: &TokenId) -> u32 {
        let counts = self.use_counts.read().await;
        counts.get(token_id).copied().unwrap_or(0)
    }

    /// Increment the use count for a token
    ///
    /// Returns the new use count.
    pub async fn increment_use_count(&self, token_id: &TokenId) -> u32 {
        let mut counts = self.use_counts.write().await;
        let count = counts.entry(*token_id).or_insert(0);
        *count += 1;
        *count
    }

    /// Check if a token has exceeded its use limit
    pub async fn has_exceeded_uses(&self, token_id: &TokenId, max_uses: u32) -> bool {
        self.get_use_count(token_id).await >= max_uses
    }

    /// Atomically check-and-increment: if current count < max_uses, increment and
    /// return Ok(new_count). Otherwise return Err(current_count).
    ///
    /// This prevents the TOCTOU race in separate read-then-write use-count checks.
    pub async fn try_use_token(&self, token_id: &TokenId, max_uses: u32) -> Result<u32, u32> {
        let mut counts = self.use_counts.write().await;
        let count = counts.entry(*token_id).or_insert(0);
        if *count >= max_uses {
            return Err(*count);
        }
        *count += 1;
        Ok(*count)
    }

    /// Revoke a specific token
    pub async fn revoke_token(&self, token_id: TokenId) {
        {
            let mut revoked = self.revoked_tokens.write().await;
            revoked.insert(token_id);
        }
        // Persist if configured (lock is released before this call)
        self.persist_revoked_tokens().await;
    }

    /// Check if a specific token is revoked
    pub async fn is_token_revoked(&self, token_id: &TokenId) -> bool {
        let revoked = self.revoked_tokens.read().await;
        revoked.contains(token_id)
    }

    /// Get all revoked token IDs
    pub async fn get_revoked_tokens(&self) -> Vec<TokenId> {
        let revoked = self.revoked_tokens.read().await;
        revoked.iter().copied().collect()
    }

    /// Clear old use counts (tokens that are no longer valid)
    ///
    /// Call periodically to prevent unbounded growth.
    pub async fn cleanup_use_counts(&self, valid_tokens: &HashSet<TokenId>) {
        let mut counts = self.use_counts.write().await;
        counts.retain(|id, _| valid_tokens.contains(id));
    }

    /// Persist state to disk
    pub async fn persist(&self) -> std::io::Result<()> {
        if let Some(path) = &self.storage_path {
            // Persist epoch
            let epoch_path = path.join("epoch");
            fs::write(epoch_path, self.current_epoch().to_string())?;

            // Persist revoked tokens
            self.persist_revoked_tokens().await;
        }
        Ok(())
    }

    /// Load state from disk
    async fn load_state(&self) -> std::io::Result<()> {
        if let Some(path) = &self.storage_path {
            // Load epoch
            let epoch_path = path.join("epoch");
            if epoch_path.exists() {
                if let Ok(content) = fs::read_to_string(&epoch_path) {
                    if let Ok(epoch) = content.trim().parse::<u64>() {
                        self.set_epoch(epoch);
                    }
                }
            }

            // Load revoked tokens
            let revoked_path = path.join("revoked_tokens");
            if revoked_path.exists() {
                if let Ok(content) = fs::read_to_string(&revoked_path) {
                    let mut revoked = self.revoked_tokens.write().await;
                    for line in content.lines() {
                        if let Ok(bytes) = hex::decode(line.trim()) {
                            if bytes.len() == 16 {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(&bytes);
                                revoked.insert(TokenId(arr));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Persist revoked tokens to disk (atomic write via temp + rename)
    async fn persist_revoked_tokens(&self) {
        if let Some(path) = &self.storage_path {
            let revoked_path = path.join("revoked_tokens");
            let tmp_path = path.join("revoked_tokens.tmp");
            let revoked = self.revoked_tokens.read().await;
            let content: String = revoked
                .iter()
                .map(|id| hex::encode(id.0))
                .collect::<Vec<_>>()
                .join("\n");
            if let Err(e) =
                fs::write(&tmp_path, &content).and_then(|_| fs::rename(&tmp_path, &revoked_path))
            {
                tracing::error!(
                    "CRITICAL: Failed to persist revoked tokens to {}: {}",
                    revoked_path.display(),
                    e
                );
            }
        }
    }
}

impl Default for CapabilityStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch() {
        let store = CapabilityStore::new();

        assert_eq!(store.current_epoch(), 0);

        let new_epoch = store.advance_epoch();
        assert_eq!(new_epoch, 1);
        assert_eq!(store.current_epoch(), 1);

        assert!(store.is_epoch_valid(1));
        assert!(!store.is_epoch_valid(0));
    }

    #[tokio::test]
    async fn test_use_counts() {
        let store = CapabilityStore::new();
        let token_id = TokenId::new();

        assert_eq!(store.get_use_count(&token_id).await, 0);

        store.increment_use_count(&token_id).await;
        assert_eq!(store.get_use_count(&token_id).await, 1);

        store.increment_use_count(&token_id).await;
        assert_eq!(store.get_use_count(&token_id).await, 2);

        assert!(!store.has_exceeded_uses(&token_id, 3).await);
        assert!(store.has_exceeded_uses(&token_id, 2).await);
    }

    #[tokio::test]
    async fn test_token_revocation() {
        let store = CapabilityStore::new();
        let token_id = TokenId::new();

        assert!(!store.is_token_revoked(&token_id).await);

        store.revoke_token(token_id).await;
        assert!(store.is_token_revoked(&token_id).await);
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create store and add some state
        {
            let store = CapabilityStore::with_persistence(temp_dir.path())
                .await
                .unwrap();
            store.advance_epoch();
            store.advance_epoch();

            let token_id = TokenId::from_bytes([1u8; 16]);
            store.revoke_token(token_id).await;
            store.persist().await.unwrap();
        }

        // Load in new store
        {
            let store = CapabilityStore::with_persistence(temp_dir.path())
                .await
                .unwrap();
            assert_eq!(store.current_epoch(), 2);

            let token_id = TokenId::from_bytes([1u8; 16]);
            assert!(store.is_token_revoked(&token_id).await);
        }
    }
}
