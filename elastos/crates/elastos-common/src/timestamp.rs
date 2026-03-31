//! Secure timestamps for ElastOS
//!
//! Provides trusted timestamps using a combination of wall clock time
//! and a monotonic counter that always increases.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Global monotonic counter - survives within process, persisted across restarts
static MONOTONIC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Clock skew tolerance in seconds (5 minutes)
pub const CLOCK_SKEW_TOLERANCE_SECS: u64 = 300;

/// Secure timestamp - runtime-controlled, not raw system time
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecureTimestamp {
    /// Wall clock time (Unix seconds)
    pub unix_secs: u64,

    /// Monotonic counter (survives clock changes, always increases)
    pub monotonic_seq: u64,
}

impl SecureTimestamp {
    /// Create a new secure timestamp with current time
    pub fn now() -> Self {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let monotonic_seq = MONOTONIC_COUNTER.fetch_add(1, Ordering::SeqCst);

        Self {
            unix_secs,
            monotonic_seq,
        }
    }

    /// Create a timestamp for a specific time (for testing or expiry calculation)
    pub fn at(unix_secs: u64) -> Self {
        let monotonic_seq = MONOTONIC_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self {
            unix_secs,
            monotonic_seq,
        }
    }

    /// Create a timestamp in the future (for expiry)
    pub fn after_secs(secs: u64) -> Self {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + secs;
        let monotonic_seq = MONOTONIC_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self {
            unix_secs,
            monotonic_seq,
        }
    }

    /// Create a timestamp in the future (for expiry)
    pub fn after_mins(mins: u64) -> Self {
        Self::after_secs(mins * 60)
    }

    /// Create a timestamp in the future (for expiry)
    pub fn after_hours(hours: u64) -> Self {
        Self::after_secs(hours * 3600)
    }

    /// Check if this timestamp is before another (considering both fields)
    pub fn is_before(&self, other: &SecureTimestamp) -> bool {
        // Primary comparison is wall clock
        if self.unix_secs != other.unix_secs {
            return self.unix_secs < other.unix_secs;
        }
        // Tie-breaker is monotonic sequence
        self.monotonic_seq < other.monotonic_seq
    }

    /// Check if this timestamp is after another
    pub fn is_after(&self, other: &SecureTimestamp) -> bool {
        other.is_before(self)
    }

    /// Check if this timestamp is in the future (with clock skew tolerance)
    pub fn is_future(&self) -> bool {
        let now = Self::now();
        self.unix_secs > now.unix_secs + CLOCK_SKEW_TOLERANCE_SECS
    }

    /// Check if this timestamp has expired
    pub fn is_expired(&self) -> bool {
        let now = Self::now();
        self.is_before(&now)
    }

    /// Initialize the monotonic counter (called by runtime on startup)
    pub fn init_counter(value: u64) {
        MONOTONIC_COUNTER.store(value, Ordering::SeqCst);
    }

    /// Get current monotonic counter value (for persistence)
    pub fn current_counter() -> u64 {
        MONOTONIC_COUNTER.load(Ordering::SeqCst)
    }
}

impl Default for SecureTimestamp {
    fn default() -> Self {
        Self::now()
    }
}

impl std::fmt::Display for SecureTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.unix_secs, self.monotonic_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_creation() {
        let ts1 = SecureTimestamp::now();
        let ts2 = SecureTimestamp::now();

        // Monotonic sequence should increase
        assert!(ts2.monotonic_seq > ts1.monotonic_seq);

        // Unix time should be reasonable
        assert!(ts1.unix_secs > 1700000000); // After 2023
    }

    #[test]
    fn test_timestamp_ordering() {
        let ts1 = SecureTimestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let ts2 = SecureTimestamp::now();

        assert!(ts1.is_before(&ts2));
        assert!(ts2.is_after(&ts1));
    }

    #[test]
    fn test_timestamp_expiry() {
        let past = SecureTimestamp::at(1000);
        let future = SecureTimestamp::after_hours(1);

        assert!(past.is_expired());
        assert!(!future.is_expired());
    }

    #[test]
    fn test_future_detection() {
        let far_future = SecureTimestamp {
            unix_secs: SecureTimestamp::now().unix_secs + CLOCK_SKEW_TOLERANCE_SECS + 100,
            monotonic_seq: 0,
        };
        assert!(far_future.is_future());

        let near_future = SecureTimestamp {
            unix_secs: SecureTimestamp::now().unix_secs + 10,
            monotonic_seq: 0,
        };
        assert!(!near_future.is_future()); // Within tolerance
    }
}
