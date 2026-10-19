//! Capability token storage and revocation
//!
//! Manages:
//! - Global revocation epoch (for mass revocation)
//! - Per-token use counts (for use-limited tokens)
//! - Revocation list (for individual token revocation)

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::RwLock;

use super::token::TokenId;

/// The stable lock file orders epoch writers across Runtime processes.
struct EpochFileLock(fs::File);

impl EpochFileLock {
    fn acquire(path: &Path, initialize: bool) -> std::io::Result<(Self, bool)> {
        Self::acquire_inner(path, initialize).map_err(|error| {
            std::io::Error::new(error.kind(), format!(
                "Capability store refused: cannot acquire revocation epoch lock at {}: {error}. \
                 Stop all Runtime processes that share this data directory and preserve the capability store. \
                 Check directory ownership, permissions, and filesystem lock support. \
                 Preserve the epoch and any existing lock file. \
                 If only the lock file is missing and the stored epoch is valid, restart Runtime to recreate the lock. \
                 Otherwise repair access before restarting. \
                 If repair is uncertain, keep Runtime stopped and seek recovery help.",
                path.join("epoch.lock").display()
            ))
        })
    }

    fn acquire_inner(path: &Path, initialize: bool) -> std::io::Result<(Self, bool)> {
        let lock_path = path.join("epoch.lock");
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let (file, created) = if initialize {
            match options.create_new(true).open(&lock_path) {
                Ok(file) => (file, true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    (options.create_new(false).open(&lock_path)?, false)
                }
                Err(error) => return Err(error),
            }
        } else {
            (options.open(&lock_path)?, false)
        };
        file.lock()?;
        Ok((Self(file), created))
    }
}

impl Drop for EpochFileLock {
    fn drop(&mut self) {
        // Release at scope exit even if a concurrently spawned child briefly
        // inherited this open description before its exec closes the fd.
        let _ = self.0.unlock();
    }
}

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

    /// Get the last observed revocation epoch. Validation refreshes disk state.
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
    pub fn advance_epoch(&self) -> std::io::Result<u64> {
        self.epoch_transaction(
            |current| {
                current
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("capability epoch exhausted"))
            },
            |epoch| self.write_epoch(epoch),
        )
    }

    /// Load an already-persisted epoch during initialization.
    ///
    /// A lower value cannot restore previously revoked authority. The only
    /// Runtime caller is construction-time loading; use `advance_epoch` for
    /// revocation, which persists the new value before publishing it.
    pub fn set_epoch(&self, epoch: u64) {
        let _epoch_writer = self
            .epoch_writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.epoch.fetch_max(epoch, Ordering::SeqCst);
    }

    /// Serialize the durable snapshot, update and write across instances and
    /// processes. A stale instance always starts from the durable maximum.
    fn epoch_transaction(
        &self,
        update: impl FnOnce(u64) -> std::io::Result<u64>,
        writer: impl FnOnce(u64) -> std::io::Result<()>,
    ) -> std::io::Result<u64> {
        let _epoch_writer = self
            .epoch_writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_lock = self
            .storage_path
            .as_ref()
            .map(|path| EpochFileLock::acquire(path, false))
            .transpose()?;
        if let Some(path) = &self.storage_path {
            self.epoch
                .fetch_max(Self::read_epoch(path)?, Ordering::SeqCst);
        }
        let next = update(self.current_epoch())?;
        if let Err(error) = writer(next) {
            // Rename can succeed before directory synchronization fails. Keep
            // the advanced state while reporting the durability error.
            if let Some(path) = &self.storage_path {
                if let Ok(persisted) = Self::read_epoch(path) {
                    self.epoch.fetch_max(persisted, Ordering::SeqCst);
                }
            }
            return Err(error);
        }
        self.epoch.fetch_max(next, Ordering::SeqCst);
        Ok(next)
    }

    fn read_epoch(path: &Path) -> std::io::Result<u64> {
        fs::read_to_string(path.join("epoch"))?
            .trim()
            .parse()
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid capability epoch")
            })
    }

    fn write_epoch(&self, epoch: u64) -> std::io::Result<()> {
        if let Some(path) = &self.storage_path {
            let epoch_path = path.join("epoch");
            let tmp_path = path.join("epoch.tmp");
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut staged = options.open(&tmp_path)?;
            staged.write_all(epoch.to_string().as_bytes())?;
            staged.sync_all()?;
            fs::rename(&tmp_path, &epoch_path)?;
            #[cfg(unix)]
            fs::File::open(path).and_then(|directory| directory.sync_all()).map_err(|error| {
                std::io::Error::new(error.kind(), format!(
                    "Capability epoch was written, but directory synchronization failed; durability is uncertain: {error}"
                ))
            })?;
        }
        Ok(())
    }

    /// Check if a token's epoch is valid
    pub fn is_epoch_valid(&self, token_epoch: u64) -> bool {
        if let Some(path) = &self.storage_path {
            let Ok(persisted) = Self::read_epoch(path) else {
                return false;
            };
            self.epoch.fetch_max(persisted, Ordering::SeqCst);
        }
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
            // Only the creator of the stable lock can initialize an absent
            // epoch. An interrupted initialization or removed epoch requires
            // explicit repair instead of silently restoring epoch zero.
            {
                let (_file_lock, created) = EpochFileLock::acquire(path, true)?;
                let epoch = match Self::read_epoch(path) {
                    Ok(epoch) => epoch,
                    Err(error) if created && error.kind() == std::io::ErrorKind::NotFound => {
                        self.write_epoch(0)?;
                        0
                    }
                    Err(error) => {
                        return Err(std::io::Error::new(error.kind(), format!(
                            "Capability store startup refused: cannot load revocation epoch at {}: {error}. \
                             Stop all Runtime processes that share this data directory and preserve the capability store. \
                             Restore a valid unsigned 64-bit epoch at or above the latest committed epoch established from trusted valid state. \
                             If that value cannot be established, keep Runtime stopped and seek recovery help.",
                            path.join("epoch").display()
                        )));
                    }
                };
                self.set_epoch(epoch);
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
                    .map(|_| store.advance_epoch().unwrap())
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
            if blocked_path == "epoch" {
                fs::remove_file(directory.path().join("epoch")).unwrap();
            }
            fs::create_dir(directory.path().join(blocked_path)).unwrap();
            assert!(store.advance_epoch().is_err());
            assert_eq!(store.current_epoch(), 0);
            // A synchronous writer guard must not make this async API !Send.
            assert_send(store.persist());
            assert!(store.persist().await.is_err());
            assert_eq!(store.current_epoch(), 0);
            fs::remove_dir(directory.path().join(blocked_path)).unwrap();
            if blocked_path == "epoch" {
                fs::write(directory.path().join("epoch"), "0").unwrap();
            }
            assert_eq!(store.advance_epoch().unwrap(), 1);
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
        assert!(store.advance_epoch().is_err());
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

    #[tokio::test]
    async fn test_independent_epoch_writers_preserve_every_advance() {
        use std::sync::{Arc, Barrier};
        const WRITERS: usize = 4;
        const ADVANCES: usize = 4;
        let directory = tempfile::tempdir().unwrap();
        let mut stores = Vec::new();
        for _ in 0..WRITERS {
            stores.push(
                CapabilityStore::with_persistence(directory.path())
                    .await
                    .unwrap(),
            );
        }
        let barrier = Arc::new(Barrier::new(WRITERS));
        let handles = stores
            .into_iter()
            .map(|store| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..ADVANCES)
                        .map(|_| store.advance_epoch().unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let mut epochs = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        epochs.sort_unstable();
        assert_eq!(
            epochs,
            (1..=(WRITERS * ADVANCES) as u64).collect::<Vec<_>>()
        );
        let reloaded = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        assert_eq!(reloaded.current_epoch(), (WRITERS * ADVANCES) as u64);
    }

    #[test]
    fn test_epoch_file_lock_is_process_wide() {
        const CHILD_PATH: &str = "ELASTOS_EPOCH_LOCK_TEST_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            assert!(
                matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)),
                "a second process entered an active epoch transaction"
            );
            println!("EPOCH_FILE_LOCK_CHILD_VERIFIED");
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = runtime
            .block_on(CapabilityStore::with_persistence(directory.path()))
            .unwrap();
        store
            .epoch_transaction(
                |current| {
                    let result = std::process::Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "capability::store::tests::test_epoch_file_lock_is_process_wide",
                            "--nocapture",
                        ])
                        .env(CHILD_PATH, directory.path().join("epoch.lock"))
                        .output()
                        .unwrap();
                    assert!(
                        result.status.success(),
                        "cross-process lock probe failed: {}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                    let output = String::from_utf8_lossy(&result.stdout);
                    assert!(
                        output.contains("running 1 test")
                            && output.contains("EPOCH_FILE_LOCK_CHILD_VERIFIED"),
                        "the exact child lock probe did not execute: {output}"
                    );
                    Ok(current)
                },
                |_| Ok(()),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn test_external_epoch_advance_retires_old_tokens_in_a_running_store() {
        use crate::capability::manager::ValidationError;
        use crate::capability::{Action, CapabilityManager, ResourceId, TokenConstraints};
        use crate::primitives::{audit::AuditLog, metrics::MetricsManager};
        use std::sync::Arc;

        let directory = tempfile::tempdir().unwrap();
        let running = Arc::new(
            CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap(),
        );
        let manager = CapabilityManager::new(
            running.clone(),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        );
        let resource = ResourceId::new("localhost://Users/self/Documents/test");
        let tokens = ["first", "second"].map(|capsule| {
            manager.grant(
                capsule,
                resource.clone(),
                Action::Read,
                TokenConstraints::default(),
                None,
            )
        });
        assert_ne!(tokens[0].id, tokens[1].id);
        let external = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        assert!(running.is_epoch_valid(0));
        assert_eq!(external.advance_epoch().unwrap(), 1);
        for token in tokens {
            assert!(
                matches!(
                    manager
                        .validate(&token, &token.capsule, Action::Read, &resource, None)
                        .await,
                    Err(ValidationError::TokenRevoked)
                ),
                "a running store must reject tokens from the earlier durable epoch"
            );
        }
        assert!(running.is_epoch_valid(1));
        assert_eq!(running.current_epoch(), 1);
    }

    #[tokio::test]
    async fn test_epoch_lock_failure_preserves_state_and_explains_recovery() {
        for missing in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let store = CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap();
            assert_eq!(store.advance_epoch().unwrap(), 1);
            let epoch_path = directory.path().join("epoch");
            let lock_path = directory.path().join("epoch.lock");
            fs::remove_file(&lock_path).unwrap();
            if !missing {
                fs::create_dir(&lock_path).unwrap();
            }
            let expected_kind = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .unwrap_err()
                .kind();
            let mut errors = vec![
                store.advance_epoch().unwrap_err(),
                store.persist().await.unwrap_err(),
            ];
            if !missing {
                errors.push(
                    CapabilityStore::with_persistence(directory.path())
                        .await
                        .err()
                        .expect("startup must refuse an unusable epoch lock"),
                );
            }
            for error in errors {
                assert_eq!(error.kind(), expected_kind);
                let message = error.to_string();
                assert!(
                    message.contains("cannot acquire revocation epoch lock"),
                    "epoch lock failure needs context: {message}"
                );
                assert!(
                    message.contains(&lock_path.display().to_string()),
                    "{message}"
                );
                assert!(message.contains("Stop all Runtime processes"), "{message}");
                assert!(
                    message.contains("preserve the capability store"),
                    "{message}"
                );
                assert!(
                    message.contains("ownership, permissions, and filesystem lock support"),
                    "{message}"
                );
                assert!(
                    message.contains("Preserve the epoch and any existing lock file"),
                    "{message}"
                );
            }
            assert_eq!(store.current_epoch(), 1);
            assert_eq!(fs::read_to_string(&epoch_path).unwrap(), "1");
            assert_eq!(lock_path.exists(), !missing);
            if missing {
                drop(store);
                let restarted = CapabilityStore::with_persistence(directory.path())
                    .await
                    .unwrap();
                assert!(lock_path.is_file());
                assert_eq!(fs::read_to_string(&epoch_path).unwrap(), "1");
                assert_eq!(restarted.current_epoch(), 1);
                assert!(!restarted.is_epoch_valid(0));
                assert_eq!(restarted.advance_epoch().unwrap(), 2);
            }
        }
    }

    #[tokio::test]
    async fn test_missing_or_invalid_epoch_fails_closed_until_repaired() {
        for (contents, missing_lock, repaired_epoch) in [
            (None, false, 7_u64),
            (Some("invalid"), false, 7),
            (None, false, 9),
            (Some("invalid"), false, 9),
            (Some(""), true, 7),
            (Some(""), true, 9),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap();
            for _ in 0..7 {
                store.advance_epoch().unwrap();
            }
            let epoch_path = directory.path().join("epoch");
            let trusted_epoch: u64 = fs::read_to_string(&epoch_path).unwrap().parse().unwrap();
            if let Some(contents) = contents {
                fs::write(&epoch_path, contents).unwrap();
            } else {
                fs::remove_file(&epoch_path).unwrap();
            }
            if missing_lock {
                // Older stores can have an empty epoch without a lock file.
                fs::remove_file(directory.path().join("epoch.lock")).unwrap();
            }
            assert!(!store.is_epoch_valid(7));
            assert!(store.advance_epoch().is_err());
            assert!(store.persist().await.is_err());
            let error = CapabilityStore::with_persistence(directory.path())
                .await
                .err()
                .expect("startup must refuse unreadable durable state");
            assert_eq!(
                error.kind(),
                if contents.is_none() {
                    std::io::ErrorKind::NotFound
                } else {
                    std::io::ErrorKind::InvalidData
                }
            );
            let message = error.to_string();
            assert!(
                message.contains("Capability store startup refused"),
                "{message}"
            );
            assert!(
                message.contains(&epoch_path.display().to_string()),
                "{message}"
            );
            assert!(message.contains("Stop all Runtime processes"), "{message}");
            assert!(
                message.contains("preserve the capability store"),
                "{message}"
            );
            assert!(
                message.contains("Restore a valid unsigned 64-bit epoch at or above the latest committed epoch established from trusted valid state"),
                "{message}"
            );
            assert!(
                message.contains("keep Runtime stopped and seek recovery help"),
                "{message}"
            );
            if let Some(contents) = contents {
                assert_eq!(fs::read_to_string(&epoch_path).unwrap(), contents);
            } else {
                assert!(!epoch_path.exists());
            }
            assert!(directory.path().join("epoch.lock").is_file());
            assert_eq!(store.current_epoch(), 7);
            drop(store);

            // The fixture's latest trusted epoch is the lower bound for repair.
            assert!(repaired_epoch >= trusted_epoch);
            fs::write(&epoch_path, repaired_epoch.to_string()).unwrap();
            let repaired = CapabilityStore::with_persistence(directory.path())
                .await
                .unwrap();
            assert_eq!(repaired.current_epoch(), repaired_epoch);
            for revoked_epoch in 0..repaired_epoch {
                assert!(!repaired.is_epoch_valid(revoked_epoch));
            }
            assert!(repaired.is_epoch_valid(repaired_epoch));
            assert_eq!(repaired.advance_epoch().unwrap(), repaired_epoch + 1);
        }
    }

    #[tokio::test]
    async fn test_interrupted_epoch_initialization_requires_explicit_repair() {
        let directory = tempfile::tempdir().unwrap();
        fs::File::create(directory.path().join("epoch.lock")).unwrap();
        assert!(
            CapabilityStore::with_persistence(directory.path())
                .await
                .is_err(),
            "interrupted initialization must require explicit repair"
        );
        assert!(!directory.path().join("epoch").exists());
    }

    #[tokio::test]
    async fn test_post_write_error_keeps_the_observed_epoch_advanced() {
        let directory = tempfile::tempdir().unwrap();
        let store = CapabilityStore::with_persistence(directory.path())
            .await
            .unwrap();
        let error = store
            .epoch_transaction(
                |current| Ok(current + 1),
                |next| {
                    store.write_epoch(next)?;
                    Err(std::io::Error::other("fixture: durability is uncertain"))
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("durability is uncertain"));
        assert_eq!(
            store.current_epoch(),
            1,
            "a post-write error must retain the observed advanced epoch"
        );
        assert_eq!(CapabilityStore::read_epoch(directory.path()).unwrap(), 1);
        store.persist().await.unwrap();
        assert_eq!(CapabilityStore::read_epoch(directory.path()).unwrap(), 1);
    }

    #[test]
    fn test_epoch() {
        let store = CapabilityStore::new();

        assert_eq!(store.current_epoch(), 0);

        let new_epoch = store.advance_epoch().unwrap();
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
            store.advance_epoch().unwrap();
            store.advance_epoch().unwrap();

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
