//! Per-capsule metrics tracking
//!
//! Runtime tracks resource usage per capsule for:
//! 1. Rate limiting (prevent DoS)
//! 2. Resource quotas (fair sharing)
//! 3. Audit and monitoring
//!
//! Phase 3: Track only, no enforcement
//! Later: Add configurable limits and enforcement

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// Metrics for a single capsule
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapsuleMetrics {
    /// Number of capability requests in current period
    pub capability_requests: u64,

    /// Number of messages sent in current period
    pub messages_sent: u64,

    /// Number of messages received in current period
    pub messages_received: u64,

    /// Bytes allocated (WASM linear memory)
    pub memory_bytes: u64,

    /// Total capability uses (lifetime)
    pub total_capability_uses: u64,

    /// Total bytes read
    pub total_bytes_read: u64,

    /// Total bytes written
    pub total_bytes_written: u64,

    /// Number of errors/failures
    pub error_count: u64,

    /// Time capsule was started (Unix timestamp)
    pub started_at: u64,

    /// CPU time used in milliseconds (if trackable)
    pub cpu_time_ms: u64,
}

impl CapsuleMetrics {
    /// Create new metrics with current timestamp
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Self {
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            ..Default::default()
        }
    }

    /// Record a capability request
    pub fn record_capability_request(&mut self) {
        self.capability_requests += 1;
    }

    /// Record a capability use
    pub fn record_capability_use(&mut self) {
        self.total_capability_uses += 1;
    }

    /// Record a message sent
    pub fn record_message_sent(&mut self) {
        self.messages_sent += 1;
    }

    /// Record a message received
    pub fn record_message_received(&mut self) {
        self.messages_received += 1;
    }

    /// Record bytes read
    pub fn record_bytes_read(&mut self, bytes: u64) {
        self.total_bytes_read += bytes;
    }

    /// Record bytes written
    pub fn record_bytes_written(&mut self, bytes: u64) {
        self.total_bytes_written += bytes;
    }

    /// Record an error
    pub fn record_error(&mut self) {
        self.error_count += 1;
    }

    /// Update memory usage
    pub fn set_memory_bytes(&mut self, bytes: u64) {
        self.memory_bytes = bytes;
    }

    /// Reset per-period counters (for rate limiting windows)
    pub fn reset_period_counters(&mut self) {
        self.capability_requests = 0;
        self.messages_sent = 0;
        self.messages_received = 0;
    }
}

/// Resource limits for rate limiting (Phase 3: not enforced, just configured)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Max capability requests per minute
    pub max_capability_requests_per_min: u32,

    /// Max messages per second
    pub max_messages_per_sec: u32,

    /// Max memory in bytes
    pub max_memory_bytes: u64,

    /// Max storage in bytes
    pub max_storage_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_capability_requests_per_min: 1000,
            max_messages_per_sec: 100,
            max_memory_bytes: 256 * 1024 * 1024,   // 256 MB
            max_storage_bytes: 1024 * 1024 * 1024, // 1 GB
        }
    }
}

/// Metrics manager for all capsules
pub struct MetricsManager {
    /// Per-capsule metrics
    metrics: Arc<RwLock<HashMap<String, CapsuleMetrics>>>,

    /// Default resource limits
    default_limits: ResourceLimits,

    /// Per-capsule limits overrides
    limits: Arc<RwLock<HashMap<String, ResourceLimits>>>,

    /// When the current rate-limiting period started
    period_start: Arc<RwLock<Instant>>,

    /// Rate limit period duration
    period_duration: Duration,
}

impl MetricsManager {
    /// Create a new metrics manager
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            default_limits: ResourceLimits::default(),
            limits: Arc::new(RwLock::new(HashMap::new())),
            period_start: Arc::new(RwLock::new(Instant::now())),
            period_duration: Duration::from_secs(60),
        }
    }

    fn metrics_read(&self) -> RwLockReadGuard<'_, HashMap<String, CapsuleMetrics>> {
        self.metrics.read().unwrap_or_else(|poisoned| {
            tracing::warn!("metrics registry lock poisoned; recovering read access");
            poisoned.into_inner()
        })
    }

    fn metrics_write(&self) -> RwLockWriteGuard<'_, HashMap<String, CapsuleMetrics>> {
        self.metrics.write().unwrap_or_else(|poisoned| {
            tracing::warn!("metrics registry lock poisoned; recovering write access");
            poisoned.into_inner()
        })
    }

    fn limits_read(&self) -> RwLockReadGuard<'_, HashMap<String, ResourceLimits>> {
        self.limits.read().unwrap_or_else(|poisoned| {
            tracing::warn!("metrics limits lock poisoned; recovering read access");
            poisoned.into_inner()
        })
    }

    fn limits_write(&self) -> RwLockWriteGuard<'_, HashMap<String, ResourceLimits>> {
        self.limits.write().unwrap_or_else(|poisoned| {
            tracing::warn!("metrics limits lock poisoned; recovering write access");
            poisoned.into_inner()
        })
    }

    fn period_start_write(&self) -> RwLockWriteGuard<'_, Instant> {
        self.period_start.write().unwrap_or_else(|poisoned| {
            tracing::warn!("metrics period lock poisoned; recovering write access");
            poisoned.into_inner()
        })
    }

    /// Start tracking a new capsule
    pub fn start_capsule(&self, capsule_id: &str) {
        let mut metrics = self.metrics_write();
        metrics.insert(capsule_id.to_string(), CapsuleMetrics::new());
    }

    /// Stop tracking a capsule
    pub fn stop_capsule(&self, capsule_id: &str) -> Option<CapsuleMetrics> {
        let mut metrics = self.metrics_write();
        metrics.remove(capsule_id)
    }

    /// Get metrics for a capsule
    pub fn get_metrics(&self, capsule_id: &str) -> Option<CapsuleMetrics> {
        let metrics = self.metrics_read();
        metrics.get(capsule_id).cloned()
    }

    /// Get all metrics
    pub fn get_all_metrics(&self) -> HashMap<String, CapsuleMetrics> {
        let metrics = self.metrics_read();
        metrics.clone()
    }

    /// Record a capability request for a capsule
    pub fn record_capability_request(&self, capsule_id: &str) {
        self.check_period_reset();
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_capability_request();
        }
    }

    /// Record a capability use for a capsule
    pub fn record_capability_use(&self, capsule_id: &str) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_capability_use();
        }
    }

    /// Record a message sent
    pub fn record_message_sent(&self, capsule_id: &str) {
        self.check_period_reset();
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_message_sent();
        }
    }

    /// Record a message received
    pub fn record_message_received(&self, capsule_id: &str) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_message_received();
        }
    }

    /// Record bytes read
    pub fn record_bytes_read(&self, capsule_id: &str, bytes: u64) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_bytes_read(bytes);
        }
    }

    /// Record bytes written
    pub fn record_bytes_written(&self, capsule_id: &str, bytes: u64) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_bytes_written(bytes);
        }
    }

    /// Record an error
    pub fn record_error(&self, capsule_id: &str) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.record_error();
        }
    }

    /// Update memory usage
    pub fn set_memory_bytes(&self, capsule_id: &str, bytes: u64) {
        let mut metrics = self.metrics_write();
        if let Some(m) = metrics.get_mut(capsule_id) {
            m.set_memory_bytes(bytes);
        }
    }

    /// Set resource limits for a specific capsule
    pub fn set_limits(&self, capsule_id: &str, limits: ResourceLimits) {
        let mut lim = self.limits_write();
        lim.insert(capsule_id.to_string(), limits);
    }

    /// Get resource limits for a capsule
    pub fn get_limits(&self, capsule_id: &str) -> ResourceLimits {
        let lim = self.limits_read();
        lim.get(capsule_id)
            .cloned()
            .unwrap_or_else(|| self.default_limits.clone())
    }

    /// Check if rate limit would be exceeded (Phase 3: advisory only)
    ///
    /// Returns true if the action would exceed limits.
    /// Phase 3: Just returns the check result, doesn't block.
    /// Later: Will actually enforce limits.
    pub fn would_exceed_capability_limit(&self, capsule_id: &str) -> bool {
        let metrics = self.metrics_read();
        let limits = self.get_limits(capsule_id);

        if let Some(m) = metrics.get(capsule_id) {
            m.capability_requests >= limits.max_capability_requests_per_min as u64
        } else {
            false
        }
    }

    /// Check if message rate limit would be exceeded
    pub fn would_exceed_message_limit(&self, capsule_id: &str) -> bool {
        let metrics = self.metrics_read();
        let limits = self.get_limits(capsule_id);

        if let Some(m) = metrics.get(capsule_id) {
            // Convert per-second limit to per-minute for comparison
            m.messages_sent >= (limits.max_messages_per_sec as u64 * 60)
        } else {
            false
        }
    }

    /// Check and reset period counters if period has elapsed
    fn check_period_reset(&self) {
        let mut period_start = self.period_start_write();
        if period_start.elapsed() >= self.period_duration {
            *period_start = Instant::now();

            // Reset all period counters
            let mut metrics = self.metrics_write();
            for m in metrics.values_mut() {
                m.reset_period_counters();
            }
        }
    }
}

impl Default for MetricsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capsule_metrics() {
        let mut metrics = CapsuleMetrics::new();

        metrics.record_capability_request();
        metrics.record_capability_request();
        assert_eq!(metrics.capability_requests, 2);

        metrics.record_capability_use();
        assert_eq!(metrics.total_capability_uses, 1);

        metrics.record_bytes_read(1024);
        assert_eq!(metrics.total_bytes_read, 1024);
    }

    #[test]
    fn test_metrics_manager() {
        let manager = MetricsManager::new();

        manager.start_capsule("cap-1");
        manager.record_capability_request("cap-1");
        manager.record_capability_request("cap-1");

        let metrics = manager.get_metrics("cap-1").unwrap();
        assert_eq!(metrics.capability_requests, 2);

        manager.stop_capsule("cap-1");
        assert!(manager.get_metrics("cap-1").is_none());
    }

    #[test]
    fn test_rate_limit_check() {
        let manager = MetricsManager::new();

        // Set very low limit
        manager.set_limits(
            "cap-1",
            ResourceLimits {
                max_capability_requests_per_min: 2,
                ..Default::default()
            },
        );

        manager.start_capsule("cap-1");
        manager.record_capability_request("cap-1");
        assert!(!manager.would_exceed_capability_limit("cap-1"));

        manager.record_capability_request("cap-1");
        assert!(manager.would_exceed_capability_limit("cap-1"));
    }
}
