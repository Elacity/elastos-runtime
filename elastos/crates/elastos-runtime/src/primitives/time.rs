//! Secure time source for ElastOS
//!
//! Provides trusted timestamps that cannot be manipulated by capsules.
//! The `SecureTimestamp` type itself lives in `elastos-common` (shared across crates).
//! This module provides `SecureTimeSource` — the runtime's persistence manager
//! for the monotonic counter.
use std::fs;
use std::path::Path;

// Re-export the timestamp type from elastos-common
pub use elastos_common::timestamp::CLOCK_SKEW_TOLERANCE_SECS;
pub use elastos_common::SecureTimestamp;

/// Secure time source manager
///
/// Handles initialization and persistence of the monotonic counter.
pub struct SecureTimeSource {
    counter_path: Option<std::path::PathBuf>,
}

impl SecureTimeSource {
    /// Create a new time source without persistence (in-memory only)
    pub fn new() -> Self {
        Self { counter_path: None }
    }

    /// Create a time source with persistence to the given path.
    ///
    /// RECOVERY SEMANTICS: an unreadable or unparseable counter file restarts the monotonic
    /// counter from zero (loudly — a warning names the file), the same as a first run. Within one
    /// process timestamps stay strictly monotonic either way; what a corrupt file costs is
    /// CROSS-RESTART ordering against records stamped before the corruption. The audit chain does
    /// not depend on that (its ordering is the hash chain's `seq`, not the timestamp counter).
    pub fn with_persistence(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Load existing counter if file exists
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match content.trim().parse::<u64>() {
                    // Start from saved value + 1 to ensure monotonicity
                    Ok(value) => SecureTimestamp::init_counter(value + 1),
                    Err(e) => tracing::warn!(
                        "monotonic counter file {} is unparseable ({e}) — restarting the \
                         counter from zero (cross-restart ordering lost)",
                        path.display()
                    ),
                },
                Err(e) => tracing::warn!(
                    "monotonic counter file {} is unreadable ({e}) — restarting the counter \
                     from zero (cross-restart ordering lost)",
                    path.display()
                ),
            }
        }

        Ok(Self {
            counter_path: Some(path),
        })
    }

    /// Get current secure timestamp
    pub fn now(&self) -> SecureTimestamp {
        SecureTimestamp::now()
    }

    /// Persist the current counter value
    pub fn persist(&self) -> std::io::Result<()> {
        if let Some(path) = &self.counter_path {
            let value = SecureTimestamp::current_counter();
            fs::write(path, value.to_string())?;
        }
        Ok(())
    }

    /// Get the current monotonic counter value
    pub fn current_sequence(&self) -> u64 {
        SecureTimestamp::current_counter()
    }
}

impl Default for SecureTimeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SecureTimeSource {
    fn drop(&mut self) {
        // Try to persist on drop, ignore errors
        let _ = self.persist();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_source() {
        let source = SecureTimeSource::new();
        let ts1 = source.now();
        let ts2 = source.now();

        assert!(ts2.monotonic_seq > ts1.monotonic_seq);
    }
}
