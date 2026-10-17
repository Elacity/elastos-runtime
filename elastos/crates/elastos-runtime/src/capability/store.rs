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
use std::sync::Mutex;
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

    /// Serializes every epoch snapshot, persistent write and memory update.
    epoch_writer: Mutex<()>,

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
            epoch_writer: Mutex::new(()),
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
            epoch_writer: Mutex::new(()),
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
        match self.epoch_transaction(
            |current| {
                current
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("capability epoch exhausted"))
            },
            |epoch| self.write_epoch(epoch),
        ) {
            Ok(epoch) => epoch,
            Err(error) => {
                tracing::error!("CRITICAL: Capability epoch advance aborted: {}", error);
                self.current_epoch()
            }
        }
    }

    /// Load an already-persisted epoch during initialization.
    ///
    /// A lower value cannot restore previously revoked authority. The only
    /// Runtime caller is construction-time loading; use `advance_epoch` for
    /// revocation, which persists the new value before publishing it.
    pub fn set_epoch(&self, epoch: u64) {
        self.epoch_transaction(|current| Ok(current.max(epoch)), |_| Ok(()))
            .expect("initializing an epoch cannot fail");
    }

    /// All epoch writers share this synchronous transaction. The update and
    /// writer run under one lock; memory is published only after writer success.
    fn epoch_transaction(
        &self,
        update: impl FnOnce(u64) -> std::io::Result<u64>,
        writer: impl FnOnce(u64) -> std::io::Result<()>,
    ) -> std::io::Result<u64> {
        let _epoch_writer = self
            .epoch_writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = update(self.current_epoch())?;
        writer(next)?;
        self.epoch.store(next, Ordering::SeqCst);
        Ok(next)
    }

    fn write_epoch(&self, epoch: u64) -> std::io::Result<()> {
        if let Some(path) = &self.storage_path {
            let epoch_path = path.join("epoch");
            let tmp_path = path.join("epoch.tmp");
            fs::write(&tmp_path, epoch.to_string())?;
            fs::rename(&tmp_path, &epoch_path)?;
        }
        Ok(())
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
        if self.storage_path.is_some() {
            // The synchronous transaction releases its guard before the await.
            self.epoch_transaction(Ok, |epoch| self.write_epoch(epoch))?;

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

    fn assert_epoch_writer_held(store: &CapabilityStore) {
        // This is a deterministic lock-coverage probe, not a timing assertion.
        let blocked = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    matches!(
                        store.epoch_writer.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    )
                })
                .join()
                .unwrap()
        });
        assert!(blocked, "another epoch writer could enter the transaction");
    }

    #[tokio::test]
    async fn test_epoch_transaction_serializes_snapshot_write_and_publication() {
        let directory = tempfile::tempdir().unwrap();
        let store = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        for advance in [true, false] {
            let before = store.current_epoch();
            let result = store
                .epoch_transaction(
                    |current| {
                        assert_epoch_writer_held(&store);
                        assert_eq!(current, before);
                        Ok(if advance { current + 1 } else { current })
                    },
                    |next| {
                        assert_epoch_writer_held(&store);
                        assert_eq!(store.current_epoch(), before);
                        store.write_epoch(next)?;
                        assert_eq!(
                            fs::read_to_string(directory.path().join("epoch"))?,
                            next.to_string()
                        );
                        assert_eq!(
                            store.current_epoch(),
                            before,
                            "publish only after writer success"
                        );
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(store.current_epoch(), result);
            assert!(store.epoch_writer.try_lock().is_ok());
        }
    }

    #[tokio::test]
    async fn test_concurrent_epoch_advance_and_snapshot_preserve_persisted_epoch() {
        use std::sync::{Arc, Barrier};
        const WRITERS: usize = 8;
        const ADVANCES: usize = 8;
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(WRITERS + 2));
        let mut handles = Vec::new();
        for _ in 0..WRITERS {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                (0..ADVANCES)
                    .map(|_| store.advance_epoch())
                    .collect::<Vec<_>>()
            }));
        }
        let snapshot_store = Arc::clone(&store);
        let snapshot_barrier = Arc::clone(&barrier);
        let snapshots = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            snapshot_barrier.wait();
            for _ in 0..WRITERS * ADVANCES {
                runtime.block_on(snapshot_store.persist()).unwrap();
            }
        });
        barrier.wait();
        let mut epochs = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        snapshots.join().unwrap();
        epochs.sort_unstable();
        let final_epoch = (WRITERS * ADVANCES) as u64;
        assert_eq!(epochs, (1..=final_epoch).collect::<Vec<_>>());
        assert_eq!(store.current_epoch(), final_epoch);
        assert_eq!(
            fs::read_to_string(directory.path().join("epoch")).unwrap(),
            final_epoch.to_string()
        );
        let reloaded = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        assert_eq!(reloaded.current_epoch(), final_epoch);
    }

    #[tokio::test]
    async fn test_epoch_write_and_rename_failures_do_not_publish_memory() {
        fn assert_send<T: Send>(_: T) {}
        for blocked_path in ["epoch.tmp", "epoch"] {
            let directory = tempfile::tempdir().unwrap();
            let store = CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap();
            fs::create_dir(directory.path().join(blocked_path)).unwrap();
            assert_eq!(store.advance_epoch(), 0);
            assert_eq!(store.current_epoch(), 0);
            // A synchronous writer guard must not make this async API !Send.
            assert_send(store.persist());
            assert!(store.persist().await.is_err());
            assert_eq!(store.current_epoch(), 0);
            fs::remove_dir(directory.path().join(blocked_path)).unwrap();
            assert_eq!(store.advance_epoch(), 1);
            let reloaded = CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap();
            assert_eq!(reloaded.current_epoch(), 1);
        }
    }

    #[tokio::test]
    async fn test_epoch_initialization_is_monotonic_and_overflow_stays_unchanged() {
        let memory = CapabilityStore::new();
        memory.set_epoch(7);
        memory.set_epoch(3);
        assert_eq!(memory.current_epoch(), 7);
        assert!(!memory.is_epoch_valid(6));

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("epoch"), u64::MAX.to_string()).unwrap();
        let store = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        assert_eq!(store.advance_epoch(), u64::MAX);
        assert_eq!(store.current_epoch(), u64::MAX);
        assert_eq!(
            fs::read_to_string(directory.path().join("epoch")).unwrap(),
            u64::MAX.to_string()
        );
        store.persist().await.unwrap();
        let reloaded = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        assert_eq!(reloaded.current_epoch(), u64::MAX);
    }

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
