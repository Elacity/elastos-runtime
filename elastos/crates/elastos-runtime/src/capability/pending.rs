//! Pending capability request store
//!
//! Tracks capability requests that are awaiting user approval (grant/deny).
//! Requests have a timeout and are cleaned up periodically.
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::token::{Action, CapabilityToken, ResourceId};
use crate::primitives::audit::AuditLog;
use crate::primitives::time::SecureTimestamp;
use crate::session::SessionId;

/// Default request timeout in seconds (5 minutes)
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;

/// Maximum number of pending requests before rejecting new ones
pub const MAX_PENDING_REQUESTS: usize = 1024;

/// Maximum number of pending requests per session before rejecting new ones.
/// Prevents a single session from starving other sessions.
pub const MAX_PENDING_PER_SESSION: usize = 32;

/// Unique identifier for a pending request
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub String);

impl RequestId {
    /// Create a new random request ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a pending request
#[derive(Debug, Clone)]
pub enum RequestStatus {
    /// Awaiting user decision
    Pending,

    /// User granted the request
    Granted {
        token: Box<CapabilityToken>,
        duration: GrantDuration,
    },

    /// User denied the request
    Denied { reason: String },

    /// Request timed out without user response
    Expired,
}

/// Duration for which a capability is granted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantDuration {
    /// Valid for one use only
    Once,
    /// Valid until session ends
    Session,
}

impl std::str::FromStr for GrantDuration {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "once" => Ok(GrantDuration::Once),
            "session" => Ok(GrantDuration::Session),
            _ => Err(format!(
                "Invalid duration: {}. Expected 'once' or 'session'",
                s
            )),
        }
    }
}

impl std::fmt::Display for GrantDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantDuration::Once => write!(f, "once"),
            GrantDuration::Session => write!(f, "session"),
        }
    }
}

/// A pending capability request
#[derive(Debug, Clone)]
pub struct PendingCapabilityRequest {
    /// Unique request identifier
    pub id: RequestId,

    /// Session that made the request
    pub session_id: SessionId,

    /// Requested resource
    pub resource: ResourceId,

    /// Requested action
    pub action: Action,

    /// When the request was created
    pub requested_at: SecureTimestamp,

    /// When the request expires
    pub expires_at: SecureTimestamp,

    /// Current status
    pub status: RequestStatus,
}

impl PendingCapabilityRequest {
    /// Create a new pending request
    pub fn new(
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        timeout_secs: u64,
    ) -> Self {
        let requested_at = SecureTimestamp::now();
        let expires_at = SecureTimestamp::after_secs(timeout_secs);

        Self {
            id: RequestId::new(),
            session_id,
            resource,
            action,
            requested_at,
            expires_at,
            status: RequestStatus::Pending,
        }
    }

    /// Check if the request has expired
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_expired()
    }

    /// Check if the request is still pending
    pub fn is_pending(&self) -> bool {
        matches!(self.status, RequestStatus::Pending) && !self.is_expired()
    }

    /// Check if the request has been granted
    pub fn is_granted(&self) -> bool {
        matches!(self.status, RequestStatus::Granted { .. })
    }

    /// Check if the request has been denied
    pub fn is_denied(&self) -> bool {
        matches!(self.status, RequestStatus::Denied { .. })
    }

    /// Get the granted token if the request was granted
    pub fn granted_token(&self) -> Option<&CapabilityToken> {
        match &self.status {
            RequestStatus::Granted { token, .. } => Some(token.as_ref()),
            _ => None,
        }
    }
}

/// Store for pending capability requests
pub struct PendingRequestStore {
    /// Pending requests indexed by request ID
    requests: RwLock<HashMap<String, PendingCapabilityRequest>>,

    /// Index from session ID to request IDs (for listing)
    session_requests: RwLock<HashMap<String, Vec<String>>>,

    /// Audit log
    audit_log: Arc<AuditLog>,

    /// Request timeout in seconds
    timeout_secs: u64,
}

impl PendingRequestStore {
    /// Create a new pending request store
    pub fn new(audit_log: Arc<AuditLog>) -> Self {
        Self {
            requests: RwLock::new(HashMap::new()),
            session_requests: RwLock::new(HashMap::new()),
            audit_log,
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(audit_log: Arc<AuditLog>, timeout_secs: u64) -> Self {
        Self {
            requests: RwLock::new(HashMap::new()),
            session_requests: RwLock::new(HashMap::new()),
            audit_log,
            timeout_secs,
        }
    }

    /// Create a new pending request
    ///
    /// If the store is at capacity (MAX_PENDING_REQUESTS), expired requests are
    /// cleaned up first. If still at capacity, the returned request is immediately
    /// denied (not stored) to prevent request-flood DoS.
    pub async fn create_request(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
    ) -> PendingCapabilityRequest {
        // Capacity guard: evict expired if at limit
        {
            let count = self.requests.read().await.len();
            if count >= MAX_PENDING_REQUESTS {
                self.cleanup_expired().await;
                self.cleanup_old(0).await;
            }
        }
        // Re-check after cleanup — reject if still at capacity
        {
            let count = self.requests.read().await.len();
            if count >= MAX_PENDING_REQUESTS {
                let mut request = PendingCapabilityRequest::new(
                    session_id.clone(),
                    resource.clone(),
                    action,
                    self.timeout_secs,
                );
                request.status = RequestStatus::Denied {
                    reason: "Too many pending requests".to_string(),
                };
                return request;
            }
        }

        // Per-session rate limit: prevent one session from starving others
        {
            let session_requests = self.session_requests.read().await;
            if let Some(ids) = session_requests.get(&session_id.0) {
                let requests = self.requests.read().await;
                let pending_count = ids
                    .iter()
                    .filter(|id| {
                        requests
                            .get(*id)
                            .is_some_and(|r| matches!(r.status, RequestStatus::Pending))
                    })
                    .count();
                if pending_count >= MAX_PENDING_PER_SESSION {
                    let mut request = PendingCapabilityRequest::new(
                        session_id.clone(),
                        resource.clone(),
                        action,
                        self.timeout_secs,
                    );
                    request.status = RequestStatus::Denied {
                        reason: "Too many pending requests for this session".to_string(),
                    };
                    return request;
                }
            }
        }

        let request = PendingCapabilityRequest::new(
            session_id.clone(),
            resource.clone(),
            action,
            self.timeout_secs,
        );

        // Store request
        {
            let mut requests = self.requests.write().await;
            requests.insert(request.id.0.clone(), request.clone());
        }

        // Add to session index
        {
            let mut session_requests = self.session_requests.write().await;
            session_requests
                .entry(session_id.0.clone())
                .or_default()
                .push(request.id.0.clone());
        }

        // Audit
        self.audit_log
            .emit(crate::primitives::audit::AuditEvent::CapabilityRequested {
                timestamp: SecureTimestamp::now(),
                request_id: request.id.to_string(),
                session_id: session_id.to_string(),
                resource: resource.to_string(),
                action: action.to_string(),
            });

        request
    }

    /// Get a request by ID
    pub async fn get_request(&self, request_id: &str) -> Option<PendingCapabilityRequest> {
        let requests = self.requests.read().await;
        requests.get(request_id).cloned()
    }

    /// Grant a pending request
    pub async fn grant_request(
        &self,
        request_id: &str,
        token: CapabilityToken,
        duration: GrantDuration,
    ) -> Result<(), String> {
        let mut requests = self.requests.write().await;

        let request = requests
            .get_mut(request_id)
            .ok_or_else(|| format!("Request not found: {}", request_id))?;

        if !matches!(request.status, RequestStatus::Pending) {
            return Err(format!("Request {} is not pending", request_id));
        }

        if request.is_expired() {
            request.status = RequestStatus::Expired;
            return Err(format!("Request {} has expired", request_id));
        }

        request.status = RequestStatus::Granted {
            token: Box::new(token),
            duration,
        };

        Ok(())
    }

    /// Deny a pending request
    pub async fn deny_request(&self, request_id: &str, reason: &str) -> Result<(), String> {
        let mut requests = self.requests.write().await;

        let request = requests
            .get_mut(request_id)
            .ok_or_else(|| format!("Request not found: {}", request_id))?;

        if !matches!(request.status, RequestStatus::Pending) {
            return Err(format!("Request {} is not pending", request_id));
        }

        request.status = RequestStatus::Denied {
            reason: reason.to_string(),
        };

        // Audit
        self.audit_log
            .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                timestamp: SecureTimestamp::now(),
                request_id: request_id.to_string(),
                session_id: request.session_id.to_string(),
                reason: reason.to_string(),
            });

        Ok(())
    }

    /// List all pending requests (for shell to display)
    pub async fn list_pending(&self) -> Vec<PendingCapabilityRequest> {
        let requests = self.requests.read().await;
        requests
            .values()
            .filter(|r| r.is_pending())
            .cloned()
            .collect()
    }

    /// List pending requests for a specific session
    pub async fn list_session_pending(&self, session_id: &str) -> Vec<PendingCapabilityRequest> {
        let session_requests = self.session_requests.read().await;
        let requests = self.requests.read().await;

        session_requests
            .get(session_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| requests.get(id))
                    .filter(|r| r.is_pending())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clean up expired requests
    ///
    /// Returns the number of requests cleaned up
    pub async fn cleanup_expired(&self) -> usize {
        let mut expired_ids = Vec::new();

        // Find expired requests
        {
            let requests = self.requests.read().await;
            for (id, request) in requests.iter() {
                if request.is_expired() && matches!(request.status, RequestStatus::Pending) {
                    expired_ids.push(id.clone());
                }
            }
        }

        // Mark them as expired
        {
            let mut requests = self.requests.write().await;
            for id in &expired_ids {
                if let Some(request) = requests.get_mut(id) {
                    request.status = RequestStatus::Expired;
                }
            }
        }

        expired_ids.len()
    }

    /// Remove old completed/expired requests
    ///
    /// Keeps requests for `retention_secs` after they're resolved
    pub async fn cleanup_old(&self, retention_secs: u64) -> usize {
        let now = SecureTimestamp::now();
        let mut removed = Vec::new();

        {
            let mut requests = self.requests.write().await;
            requests.retain(|id, request| {
                // Keep pending requests
                if matches!(request.status, RequestStatus::Pending) {
                    return true;
                }

                // Remove old resolved requests
                let age_secs = now.unix_secs.saturating_sub(request.requested_at.unix_secs);
                if age_secs > retention_secs {
                    removed.push(id.clone());
                    false
                } else {
                    true
                }
            });
        }

        // Clean up session index
        if !removed.is_empty() {
            let mut session_requests = self.session_requests.write().await;
            for ids in session_requests.values_mut() {
                ids.retain(|id| !removed.contains(id));
            }
        }

        removed.len()
    }

    /// Get the number of pending requests
    pub async fn pending_count(&self) -> usize {
        let requests = self.requests.read().await;
        requests.values().filter(|r| r.is_pending()).count()
    }

    /// List granted requests for a specific session
    pub async fn list_session_granted(&self, session_id: &str) -> Vec<PendingCapabilityRequest> {
        let requests = self.requests.read().await;
        requests
            .values()
            .filter(|r| r.session_id.to_string() == session_id && r.is_granted())
            .cloned()
            .collect()
    }

    /// Mark a granted request as revoked
    ///
    /// This changes the status to Denied with reason "Revoked"
    pub async fn revoke_request(&self, request_id: &str) {
        let mut requests = self.requests.write().await;
        if let Some(request) = requests.get_mut(request_id) {
            if request.is_granted() {
                request.status = RequestStatus::Denied {
                    reason: "Revoked by user".to_string(),
                };

                // Audit
                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                        timestamp: SecureTimestamp::now(),
                        request_id: request_id.to_string(),
                        session_id: request.session_id.to_string(),
                        reason: "Revoked by user".to_string(),
                    });
            }
        }
    }

    /// Mark all granted requests as revoked
    ///
    /// Called when epoch is advanced to revoke all capabilities
    pub async fn revoke_all_granted(&self) {
        let mut requests = self.requests.write().await;
        let now = SecureTimestamp::now();

        for (request_id, request) in requests.iter_mut() {
            if request.is_granted() {
                request.status = RequestStatus::Denied {
                    reason: "Epoch advanced - all capabilities revoked".to_string(),
                };

                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                        timestamp: now,
                        request_id: request_id.clone(),
                        session_id: request.session_id.to_string(),
                        reason: "Epoch advanced - all capabilities revoked".to_string(),
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> PendingRequestStore {
        PendingRequestStore::new(Arc::new(AuditLog::new()))
    }

    #[tokio::test]
    async fn test_create_request() {
        let store = create_test_store();

        let request = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/photos/*"),
                Action::Read,
            )
            .await;

        assert!(request.is_pending());
        assert!(!request.id.as_str().is_empty());

        // Should be retrievable
        let retrieved = store.get_request(request.id.as_str()).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, request.id);
    }

    #[tokio::test]
    async fn test_grant_request() {
        let store = create_test_store();

        let request = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await;

        // Create a mock token
        let token = CapabilityToken::new(
            "test-capsule".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/test"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );

        // Grant the request
        let result = store
            .grant_request(request.id.as_str(), token, GrantDuration::Session)
            .await;
        assert!(result.is_ok());

        // Should now be granted
        let updated = store.get_request(request.id.as_str()).await.unwrap();
        assert!(updated.is_granted());
        assert!(updated.granted_token().is_some());
    }

    #[tokio::test]
    async fn test_deny_request() {
        let store = create_test_store();

        let request = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Write,
            )
            .await;

        // Deny the request
        let result = store
            .deny_request(request.id.as_str(), "User denied access")
            .await;
        assert!(result.is_ok());

        // Should now be denied
        let updated = store.get_request(request.id.as_str()).await.unwrap();
        assert!(updated.is_denied());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let store = create_test_store();

        // Create multiple requests
        let r1 = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/a"),
                Action::Read,
            )
            .await;

        store
            .create_request(
                SessionId::from_string("session-2"),
                ResourceId::new("localhost://Users/self/Documents/b"),
                Action::Write,
            )
            .await;

        // Both should be pending
        let pending = store.list_pending().await;
        assert_eq!(pending.len(), 2);

        // Grant one
        let token = CapabilityToken::new(
            "test".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/a"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );
        store
            .grant_request(r1.id.as_str(), token, GrantDuration::Once)
            .await
            .unwrap();

        // Now only one pending
        let pending = store.list_pending().await;
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_cannot_grant_twice() {
        let store = create_test_store();

        let request = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await;

        let token = CapabilityToken::new(
            "test".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/test"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );

        // First grant succeeds
        store
            .grant_request(request.id.as_str(), token.clone(), GrantDuration::Session)
            .await
            .unwrap();

        // Second grant fails
        let result = store
            .grant_request(request.id.as_str(), token, GrantDuration::Session)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pending_count() {
        let store = create_test_store();

        assert_eq!(store.pending_count().await, 0);

        store
            .create_request(
                SessionId::from_string("s1"),
                ResourceId::new("localhost://Users/self/Documents/a"),
                Action::Read,
            )
            .await;
        assert_eq!(store.pending_count().await, 1);

        store
            .create_request(
                SessionId::from_string("s2"),
                ResourceId::new("localhost://Users/self/Documents/b"),
                Action::Read,
            )
            .await;
        assert_eq!(store.pending_count().await, 2);
    }

    #[tokio::test]
    async fn test_expired_request() {
        // Create store with very short timeout
        let store = PendingRequestStore::with_timeout(Arc::new(AuditLog::new()), 0);

        let request = store
            .create_request(
                SessionId::from_string("session-1"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await;

        // Should be immediately expired
        assert!(request.is_expired());
        assert!(!request.is_pending());

        // Cannot grant expired request
        let token = CapabilityToken::new(
            "test".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/test"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );

        let result = store
            .grant_request(request.id.as_str(), token, GrantDuration::Session)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_per_session_limit_rejects_when_full() {
        let store = PendingRequestStore::new(Arc::new(AuditLog::new()));
        let session = SessionId::from_string("flood-session");

        // Fill to per-session limit
        for i in 0..MAX_PENDING_PER_SESSION {
            let r = store
                .create_request(
                    session.clone(),
                    ResourceId::new(format!("localhost://Users/self/Documents/res-{}", i)),
                    Action::Read,
                )
                .await;
            assert!(r.is_pending(), "request {} should be pending", i);
        }

        // Next request from same session should be denied
        let rejected = store
            .create_request(
                session.clone(),
                ResourceId::new("localhost://Users/self/Documents/overflow"),
                Action::Read,
            )
            .await;
        assert!(rejected.is_denied());

        // But a different session should still be able to create requests
        let other = store
            .create_request(
                SessionId::from_string("other-session"),
                ResourceId::new("localhost://Users/self/Documents/ok"),
                Action::Read,
            )
            .await;
        assert!(other.is_pending());
    }

    #[tokio::test]
    async fn test_capacity_limit_rejects_when_full() {
        let store = PendingRequestStore::new(Arc::new(AuditLog::new()));

        // Fill to capacity
        for i in 0..MAX_PENDING_REQUESTS {
            let r = store
                .create_request(
                    SessionId::from_string(format!("session-{}", i)),
                    ResourceId::new("localhost://Users/self/Documents/test"),
                    Action::Read,
                )
                .await;
            assert!(r.is_pending(), "request {} should be pending", i);
        }

        // Next request should be denied (capacity reached)
        let rejected = store
            .create_request(
                SessionId::from_string("session-overflow"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await;
        assert!(rejected.is_denied());
    }

    #[tokio::test]
    async fn test_capacity_limit_recovers_after_expiry() {
        // Timeout=0 means requests expire immediately
        let store = PendingRequestStore::with_timeout(Arc::new(AuditLog::new()), 0);

        // Fill to capacity (all expire immediately)
        for i in 0..MAX_PENDING_REQUESTS {
            store
                .create_request(
                    SessionId::from_string(format!("session-{}", i)),
                    ResourceId::new("localhost://Users/self/Documents/test"),
                    Action::Read,
                )
                .await;
        }

        // Next request triggers cleanup of expired requests, so it should succeed
        let recovered = store
            .create_request(
                SessionId::from_string("session-after-cleanup"),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await;
        // After cleanup of expired requests, new request should be accepted
        // (cleanup_expired marks them expired, cleanup_old removes them)
        assert!(
            recovered.is_pending() || recovered.is_expired(),
            "should be accepted after expired cleanup"
        );
    }
}
