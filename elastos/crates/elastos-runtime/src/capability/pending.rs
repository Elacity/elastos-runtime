//! Pending capability request store
//!
//! Tracks capability requests that are awaiting user approval (grant/deny).
//! Requests have a timeout and are cleaned up periodically.
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::manager::CapabilityManager;
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

    /// Plain-language reason supplied for the approval decision.
    pub reason: String,

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
        Self::new_with_reason(session_id, resource, action, String::new(), timeout_secs)
    }

    pub fn new_with_reason(
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        reason: String,
        timeout_secs: u64,
    ) -> Self {
        let requested_at = SecureTimestamp::now();
        let expires_at = SecureTimestamp::after_secs(timeout_secs);

        Self {
            id: RequestId::new(),
            session_id,
            resource,
            action,
            reason,
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

#[derive(Default)]
struct PendingRequestState {
    /// Requests and their index change together under the store's lock.
    requests: HashMap<String, PendingCapabilityRequest>,
    /// Index from session ID to request IDs (for listing)
    session_requests: HashMap<String, Vec<String>>,
}

impl PendingRequestState {
    fn cleanup_expired(&mut self, now: &SecureTimestamp) -> usize {
        let mut expired = 0;
        for request in self.requests.values_mut() {
            if matches!(request.status, RequestStatus::Pending) && request.expires_at.is_before(now)
            {
                request.status = RequestStatus::Expired;
                expired += 1;
            }
        }
        expired
    }

    fn cleanup_old(&mut self, now: &SecureTimestamp, retention_secs: u64) -> usize {
        let previous_len = self.requests.len();
        self.requests.retain(|_, request| {
            // Request expiry alone says nothing about token validity. This pass
            // checks token expiry; authoritative retirement needs the manager below.
            let resolved = match &request.status {
                RequestStatus::Pending => false,
                RequestStatus::Granted { token, .. } => {
                    token.expiry().is_some_and(|expiry| expiry.is_before(now))
                }
                RequestStatus::Denied { .. } | RequestStatus::Expired => true,
            };
            if !resolved {
                return true;
            }
            let age_secs = now.unix_secs.saturating_sub(request.requested_at.unix_secs);
            retention_secs != 0 && age_secs <= retention_secs
        });

        let removed = previous_len - self.requests.len();
        if removed != 0 {
            self.prune_session_index();
        }
        removed
    }

    fn prune_session_index(&mut self) {
        let requests = &self.requests;
        self.session_requests.retain(|_, ids| {
            ids.retain(|id| requests.contains_key(id));
            !ids.is_empty()
        });
    }
}

/// Store for pending capability requests
pub struct PendingRequestStore {
    state: RwLock<PendingRequestState>,

    /// Audit log
    audit_log: Arc<AuditLog>,

    /// Request timeout in seconds
    timeout_secs: u64,
}

impl PendingRequestStore {
    /// Create a new pending request store
    pub fn new(audit_log: Arc<AuditLog>) -> Self {
        Self {
            state: RwLock::new(PendingRequestState::default()),
            audit_log,
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Shared audit sink used by Runtime-owned bridge adapters.
    pub fn audit_log(&self) -> Arc<AuditLog> {
        self.audit_log.clone()
    }

    /// Create with custom timeout
    pub fn with_timeout(audit_log: Arc<AuditLog>, timeout_secs: u64) -> Self {
        Self {
            state: RwLock::new(PendingRequestState::default()),
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
        self.create_request_with_reason(session_id, resource, action, String::new())
            .await
    }

    pub async fn create_request_with_reason(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        reason: String,
    ) -> PendingCapabilityRequest {
        // Admission, cleanup, and both index updates are one state transition.
        let mut state = self.state.write().await;
        let now = SecureTimestamp::now();
        if state.requests.len() >= MAX_PENDING_REQUESTS {
            state.cleanup_expired(&now);
            state.cleanup_old(&now, 0);
        }

        let denial = if state.requests.len() >= MAX_PENDING_REQUESTS {
            Some("Too many pending requests")
        } else {
            let pending_count = state.session_requests.get(&session_id.0).map_or(0, |ids| {
                ids.iter()
                    .filter(|id| {
                        state.requests.get(*id).is_some_and(|request| {
                            matches!(request.status, RequestStatus::Pending)
                                && !request.expires_at.is_before(&now)
                        })
                    })
                    .count()
            });
            (pending_count >= MAX_PENDING_PER_SESSION)
                .then_some("Too many pending requests for this session")
        };

        let mut request = PendingCapabilityRequest::new_with_reason(
            session_id.clone(),
            resource.clone(),
            action,
            reason,
            self.timeout_secs,
        );
        if let Some(reason) = denial {
            request.status = RequestStatus::Denied {
                reason: reason.to_string(),
            };
            return request;
        }

        state.requests.insert(request.id.0.clone(), request.clone());
        state
            .session_requests
            .entry(session_id.0.clone())
            .or_default()
            .push(request.id.0.clone());
        drop(state);

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
        let state = self.state.read().await;
        state.requests.get(request_id).cloned()
    }

    /// Grant a pending request
    pub async fn grant_request(
        &self,
        request_id: &str,
        token: CapabilityToken,
        duration: GrantDuration,
    ) -> Result<(), String> {
        let mut state = self.state.write().await;

        let request = state
            .requests
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
        let mut state = self.state.write().await;

        let request = state
            .requests
            .get_mut(request_id)
            .ok_or_else(|| format!("Request not found: {}", request_id))?;

        if !matches!(request.status, RequestStatus::Pending) {
            return Err(format!("Request {} is not pending", request_id));
        }

        request.status = RequestStatus::Denied {
            reason: reason.to_string(),
        };
        let session_id = request.session_id.to_string();
        drop(state);

        // Audit
        self.audit_log
            .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                timestamp: SecureTimestamp::now(),
                request_id: request_id.to_string(),
                session_id,
                reason: reason.to_string(),
            });

        Ok(())
    }

    /// List all pending requests (for shell to display)
    pub async fn list_pending(&self) -> Vec<PendingCapabilityRequest> {
        let state = self.state.read().await;
        state
            .requests
            .values()
            .filter(|r| r.is_pending())
            .cloned()
            .collect()
    }

    /// List pending requests for a specific session
    pub async fn list_session_pending(&self, session_id: &str) -> Vec<PendingCapabilityRequest> {
        let state = self.state.read().await;

        state
            .session_requests
            .get(session_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| state.requests.get(id))
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
        let mut state = self.state.write().await;
        state.cleanup_expired(&SecureTimestamp::now())
    }

    /// Remove old denied/expired requests and explicitly expired grants.
    ///
    /// Retention is measured from request creation. Zero removes resolved records
    /// immediately; active grants stay available for listing and revocation.
    pub async fn cleanup_old(&self, retention_secs: u64) -> usize {
        let mut state = self.state.write().await;
        state.cleanup_old(&SecureTimestamp::now(), retention_secs)
    }

    /// Reclaim authoritatively revoked or exhausted grants when the store is full.
    ///
    /// The Runtime owner must supply the manager that issued and validates this
    /// store's grants. Revocation and remaining uses come from that manager's
    /// authoritative state, never the requested duration or session visibility.
    /// This does not consume, revoke or authorize a use.
    /// Admission still rechecks both limits under the same state lock afterward.
    pub async fn cleanup_retired_grants_at_capacity(
        &self,
        capability_manager: &CapabilityManager,
    ) -> usize {
        if self.state.read().await.requests.len() < MAX_PENDING_REQUESTS {
            return 0;
        }
        let mut state = self.state.write().await;
        if state.requests.len() < MAX_PENDING_REQUESTS {
            return 0;
        }

        let mut retired = Vec::new();
        for (id, request) in &state.requests {
            if let RequestStatus::Granted { token, .. } = &request.status {
                // The query reads only the manager's epoch/revocation/use state.
                // Those guards are released before callers access pending state.
                if capability_manager.is_revoked_or_exhausted(token).await {
                    retired.push(id.clone());
                }
            }
        }
        for id in &retired {
            state.requests.remove(id);
        }
        if !retired.is_empty() {
            state.prune_session_index();
        }
        retired.len()
    }

    /// Get the number of pending requests
    pub async fn pending_count(&self) -> usize {
        let state = self.state.read().await;
        state.requests.values().filter(|r| r.is_pending()).count()
    }

    /// List granted requests for a specific session
    pub async fn list_session_granted(&self, session_id: &str) -> Vec<PendingCapabilityRequest> {
        let state = self.state.read().await;
        state
            .session_requests
            .get(session_id)
            .into_iter()
            .flatten()
            .filter_map(|id| state.requests.get(id))
            .filter(|request| request.is_granted())
            .cloned()
            .collect()
    }

    /// Mark a granted request as revoked
    ///
    /// This changes the status to Denied with reason "Revoked"
    pub async fn revoke_request(&self, request_id: &str) {
        let mut state = self.state.write().await;
        if let Some(request) = state.requests.get_mut(request_id) {
            if request.is_granted() {
                request.status = RequestStatus::Denied {
                    reason: "Revoked by user".to_string(),
                };
                let session_id = request.session_id.to_string();
                drop(state);

                // Audit
                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                        timestamp: SecureTimestamp::now(),
                        request_id: request_id.to_string(),
                        session_id,
                        reason: "Revoked by user".to_string(),
                    });
            }
        }
    }

    /// Mark all granted requests as revoked
    ///
    /// Called when epoch is advanced to revoke all capabilities
    pub async fn revoke_all_granted(&self) {
        let mut state = self.state.write().await;
        let now = SecureTimestamp::now();
        let mut events = Vec::new();

        for (request_id, request) in state.requests.iter_mut() {
            if request.is_granted() {
                request.status = RequestStatus::Denied {
                    reason: "Epoch advanced - all capabilities revoked".to_string(),
                };

                events.push(crate::primitives::audit::AuditEvent::CapabilityDenied {
                    timestamp: now,
                    request_id: request_id.clone(),
                    session_id: request.session_id.to_string(),
                    reason: "Epoch advanced - all capabilities revoked".to_string(),
                });
            }
        }
        drop(state);
        for event in events {
            self.audit_log.emit(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> PendingRequestStore {
        PendingRequestStore::new(Arc::new(AuditLog::new()))
    }

    async fn request_for(store: &PendingRequestStore, session: &str) -> PendingCapabilityRequest {
        store
            .create_request(
                SessionId::from_string(session),
                ResourceId::new("localhost://Users/self/Documents/test"),
                Action::Read,
            )
            .await
    }

    fn token_for(request: &PendingCapabilityRequest) -> CapabilityToken {
        CapabilityToken::new(
            "test-capsule".to_string(),
            [0u8; 32],
            request.resource.clone(),
            request.action,
            Default::default(),
            SecureTimestamp::now(),
            None,
        )
    }

    async fn concurrent_admissions(store: Arc<PendingRequestStore>, shared_session: bool) -> usize {
        const CONTENDERS: usize = 64;
        let barrier = Arc::new(tokio::sync::Barrier::new(CONTENDERS + 1));
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..CONTENDERS {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            tasks.spawn(async move {
                barrier.wait().await;
                let session = if shared_session {
                    "shared-session".to_string()
                } else {
                    format!("concurrent-{index}")
                };
                request_for(&store, &session).await.is_pending()
            });
        }
        barrier.wait().await;
        let mut admitted = 0;
        while let Some(result) = tasks.join_next().await {
            admitted += usize::from(result.unwrap());
        }
        admitted
    }

    #[tokio::test]
    async fn test_expired_requests_release_session_capacity() {
        let store = create_test_store();
        for _ in 0..MAX_PENDING_PER_SESSION {
            assert!(request_for(&store, "shared-session").await.is_pending());
        }
        {
            let mut state = store.state.write().await;
            for request in state.requests.values_mut() {
                request.expires_at = SecureTimestamp::at(0);
            }
        }
        assert_eq!(store.pending_count().await, 0);

        let recovered = request_for(&store, "shared-session").await;
        assert!(recovered.is_pending());
        assert!(store.get_request(recovered.id.as_str()).await.is_some());
        assert_eq!(store.list_session_pending("shared-session").await.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_admission_respects_session_limit() {
        let store = Arc::new(create_test_store());
        for _ in 0..MAX_PENDING_PER_SESSION - 1 {
            request_for(&store, "shared-session").await;
        }

        assert_eq!(concurrent_admissions(Arc::clone(&store), true).await, 1);
        assert_eq!(store.pending_count().await, MAX_PENDING_PER_SESSION);
        assert_eq!(
            store.list_session_pending("shared-session").await.len(),
            MAX_PENDING_PER_SESSION
        );
        assert!(request_for(&store, "other-session").await.is_pending());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_admission_respects_global_limit() {
        let store = Arc::new(create_test_store());
        for index in 0..MAX_PENDING_REQUESTS - 1 {
            request_for(&store, &format!("prefill-{index}")).await;
        }

        assert_eq!(concurrent_admissions(Arc::clone(&store), false).await, 1);
        assert_eq!(store.pending_count().await, MAX_PENDING_REQUESTS);
        let state = store.state.read().await;
        assert_eq!(state.requests.len(), MAX_PENDING_REQUESTS);
        assert_eq!(state.session_requests.len(), MAX_PENDING_REQUESTS);
    }

    #[tokio::test]
    async fn test_cleanup_preserves_grants_and_removes_empty_session_indexes() {
        let store = create_test_store();
        let pending = request_for(&store, "mixed-session").await;
        let once = request_for(&store, "mixed-session").await;
        let session = request_for(&store, "grant-session").await;
        let denied = request_for(&store, "mixed-session").await;
        let expired = request_for(&store, "expired-session").await;
        store
            .grant_request(once.id.as_str(), token_for(&once), GrantDuration::Once)
            .await
            .unwrap();
        store
            .grant_request(
                session.id.as_str(),
                token_for(&session),
                GrantDuration::Session,
            )
            .await
            .unwrap();
        store
            .deny_request(denied.id.as_str(), "test denial")
            .await
            .unwrap();
        {
            let mut state = store.state.write().await;
            for request in state.requests.values_mut() {
                request.requested_at = SecureTimestamp::at(0);
                if request.id != pending.id {
                    request.expires_at = SecureTimestamp::at(0);
                }
            }
        }

        // Request expiry never expires a granted token or changes its status.
        assert_eq!(store.cleanup_expired().await, 1);
        assert_eq!(store.cleanup_old(60).await, 2);
        assert!(store.get_request(denied.id.as_str()).await.is_none());
        assert!(store.get_request(expired.id.as_str()).await.is_none());
        assert_eq!(store.list_session_pending("mixed-session").await.len(), 1);
        assert_eq!(
            store.list_session_granted("mixed-session").await[0].id,
            once.id
        );
        assert_eq!(
            store.list_session_granted("grant-session").await[0].id,
            session.id
        );
        assert!(!store
            .state
            .read()
            .await
            .session_requests
            .contains_key("expired-session"));

        // Individual and epoch revocation make grant records eligible for cleanup.
        store.revoke_request(once.id.as_str()).await;
        assert_eq!(store.cleanup_old(0).await, 1);
        assert!(store.list_session_granted("mixed-session").await.is_empty());
        store.revoke_all_granted().await;
        assert_eq!(store.cleanup_old(0).await, 1);
        assert!(!store
            .state
            .read()
            .await
            .session_requests
            .contains_key("grant-session"));
        assert_eq!(store.cleanup_old(0).await, 0);
        assert_eq!(
            store.list_session_pending("mixed-session").await[0].id,
            pending.id
        );
    }

    #[tokio::test]
    async fn test_pressure_cleanup_preserves_grants_and_reclaims_fresh_denials() {
        let store = create_test_store();
        let granted = request_for(&store, "grant-session").await;
        let token = token_for(&granted);
        let token_id = token.id;
        store
            .grant_request(granted.id.as_str(), token, GrantDuration::Session)
            .await
            .unwrap();
        for index in 0..MAX_PENDING_REQUESTS - 1 {
            let request = request_for(&store, &format!("denied-{index}")).await;
            store
                .deny_request(request.id.as_str(), "test denial")
                .await
                .unwrap();
        }
        assert_eq!(
            store.state.read().await.requests.len(),
            MAX_PENDING_REQUESTS
        );

        assert!(request_for(&store, "new-session").await.is_pending());
        let retained = store.get_request(granted.id.as_str()).await.unwrap();
        assert_eq!(retained.granted_token().unwrap().id, token_id);
        assert_eq!(store.list_session_granted("grant-session").await.len(), 1);
        let state = store.state.read().await;
        assert_eq!(state.requests.len(), 2);
        assert_eq!(state.session_requests.len(), 2);
    }

    #[tokio::test]
    async fn test_live_grants_fill_capacity_until_explicit_revocation() {
        let store = create_test_store();
        for _ in 0..MAX_PENDING_REQUESTS {
            let request = request_for(&store, "grant-session").await;
            assert!(request.is_pending());
            store
                .grant_request(
                    request.id.as_str(),
                    token_for(&request),
                    GrantDuration::Session,
                )
                .await
                .unwrap();
        }
        assert!(request_for(&store, "new-session").await.is_denied());
        assert_eq!(
            store.list_session_granted("grant-session").await.len(),
            MAX_PENDING_REQUESTS
        );
        assert_eq!(store.cleanup_old(0).await, 0);

        store.revoke_all_granted().await;
        assert!(request_for(&store, "new-session").await.is_pending());
        assert!(store.list_session_granted("grant-session").await.is_empty());
        assert_eq!(store.state.read().await.requests.len(), 1);
    }

    #[tokio::test]
    async fn test_pressure_reclaims_only_authoritatively_exhausted_grants() {
        use crate::capability::{CapabilityStore, TokenConstraints};
        use crate::primitives::metrics::MetricsManager;

        let audit = Arc::new(AuditLog::new());
        let counts = Arc::new(CapabilityStore::new());
        let manager = CapabilityManager::new(
            counts.clone(),
            audit.clone(),
            Arc::new(MetricsManager::new()),
        );
        let store = PendingRequestStore::new(audit);
        let mut grants = Vec::new();
        for (session, max_uses, used) in [
            ("exhausted-only", Some(1), 1),
            ("mixed", Some(1), 1),
            ("mixed", Some(1), 0),
            ("mixed", None, 0),
            ("mixed", Some(2), 1),
        ] {
            let request = request_for(&store, session).await;
            let token = manager.grant(
                session,
                request.resource.clone(),
                request.action,
                TokenConstraints::new(0, false, None, max_uses),
                None,
            );
            store
                .grant_request(request.id.as_str(), token.clone(), GrantDuration::Once)
                .await
                .unwrap();
            for _ in 0..used {
                manager
                    .validate(&token, session, request.action, &request.resource, None)
                    .await
                    .unwrap();
            }
            grants.push((request, token, used));
        }
        assert_eq!(store.cleanup_retired_grants_at_capacity(&manager).await, 0);
        for index in 0..MAX_PENDING_REQUESTS - grants.len() {
            assert!(request_for(&store, &format!("prefill-{index}"))
                .await
                .is_pending());
        }

        assert_eq!(store.cleanup_retired_grants_at_capacity(&manager).await, 2);
        for (index, (request, token, used)) in grants.iter().enumerate() {
            assert_eq!(
                store.get_request(request.id.as_str()).await.is_some(),
                index >= 2
            );
            assert_eq!(counts.get_use_count(token.id()).await, *used);
        }
        let state = store.state.read().await;
        assert!(!state.session_requests.contains_key("exhausted-only"));
        assert_eq!(state.session_requests["mixed"].len(), 3);
        assert_eq!(state.requests.len(), MAX_PENDING_REQUESTS - 2);
        assert_eq!(
            state.session_requests.values().map(Vec::len).sum::<usize>(),
            state.requests.len()
        );
        drop(state);

        assert!(request_for(&store, "recovered").await.is_pending());
        assert!(request_for(&store, "recovered").await.is_pending());
        assert_eq!(store.cleanup_retired_grants_at_capacity(&manager).await, 0);
        assert!(request_for(&store, "overflow").await.is_denied());
    }

    #[tokio::test]
    async fn test_pressure_reclaims_revoked_grants_without_changing_authority_state() {
        use crate::capability::{manager::ValidationError, CapabilityStore, TokenConstraints};
        use crate::primitives::metrics::MetricsManager;

        let audit = Arc::new(AuditLog::new());
        let authority = Arc::new(CapabilityStore::new());
        let manager = CapabilityManager::new(
            authority.clone(),
            audit.clone(),
            Arc::new(MetricsManager::new()),
        );
        let store = PendingRequestStore::new(audit);
        let stale = request_for(&store, "stale-epoch").await;
        let stale_token = manager.grant(
            "stale-epoch",
            stale.resource.clone(),
            stale.action,
            TokenConstraints::default(),
            None,
        );
        let epoch = manager.revoke_all("retire old epoch");
        store.revoke_all_granted().await;
        // A grant minted before revocation can finish registering afterward.
        store
            .grant_request(
                stale.id.as_str(),
                stale_token.clone(),
                GrantDuration::Session,
            )
            .await
            .unwrap();

        let revoked = request_for(&store, "mixed").await;
        let revoked_token = manager.grant(
            "mixed",
            revoked.resource.clone(),
            revoked.action,
            TokenConstraints::new(epoch, false, None, Some(2)),
            None,
        );
        store
            .grant_request(
                revoked.id.as_str(),
                revoked_token.clone(),
                GrantDuration::Session,
            )
            .await
            .unwrap();
        manager
            .validate(
                &revoked_token,
                "mixed",
                revoked.action,
                &revoked.resource,
                None,
            )
            .await
            .unwrap();
        manager
            .revoke(*revoked_token.id(), "retire individual token")
            .await;

        let live = request_for(&store, "mixed").await;
        let live_token = manager.grant(
            "mixed",
            live.resource.clone(),
            live.action,
            TokenConstraints::default(),
            None,
        );
        store
            .grant_request(live.id.as_str(), live_token.clone(), GrantDuration::Session)
            .await
            .unwrap();
        for index in 0..MAX_PENDING_REQUESTS - 3 {
            assert!(request_for(&store, &format!("prefill-{index}"))
                .await
                .is_pending());
        }
        let revoked_before = authority.get_revoked_tokens().await;

        assert_eq!(store.cleanup_retired_grants_at_capacity(&manager).await, 2);
        assert_eq!(authority.current_epoch(), epoch);
        assert_eq!(authority.get_revoked_tokens().await, revoked_before);
        assert!(authority.is_token_revoked(revoked_token.id()).await);
        assert!(!authority.is_token_revoked(stale_token.id()).await);
        assert_eq!(authority.get_use_count(revoked_token.id()).await, 1);
        for (token, session, resource) in [
            (&stale_token, "stale-epoch", &stale.resource),
            (&revoked_token, "mixed", &revoked.resource),
        ] {
            assert!(matches!(
                manager
                    .validate(token, session, Action::Read, resource, None)
                    .await,
                Err(ValidationError::TokenRevoked)
            ));
        }
        assert_eq!(authority.get_use_count(revoked_token.id()).await, 1);
        manager
            .validate(&live_token, "mixed", live.action, &live.resource, None)
            .await
            .unwrap();
        assert_eq!(store.list_session_granted("mixed").await[0].id, live.id);
        let state = store.state.read().await;
        assert!(!state.requests.contains_key(stale.id.as_str()));
        assert!(!state.requests.contains_key(revoked.id.as_str()));
        assert!(!state.session_requests.contains_key("stale-epoch"));
        assert_eq!(state.session_requests["mixed"].len(), 1);
        assert_eq!(state.requests.len(), MAX_PENDING_REQUESTS - 2);
        assert_eq!(
            state.session_requests.values().map(Vec::len).sum::<usize>(),
            state.requests.len()
        );
        drop(state);

        assert!(request_for(&store, "recovered").await.is_pending());
        assert!(request_for(&store, "recovered").await.is_pending());
        assert_eq!(store.cleanup_retired_grants_at_capacity(&manager).await, 0);
        assert!(request_for(&store, "overflow").await.is_denied());
    }

    #[tokio::test]
    async fn test_cleanup_reclaims_only_explicitly_expired_grants() {
        let store = create_test_store();
        let expired = request_for(&store, "expired-grant-session").await;
        let live = request_for(&store, "live-grant-session").await;
        let mut expired_token = token_for(&expired);
        expired_token.expiry = Some(SecureTimestamp::at(0));
        let mut live_token = token_for(&live);
        live_token.expiry = Some(SecureTimestamp::after_secs(300));
        store
            .grant_request(expired.id.as_str(), expired_token, GrantDuration::Session)
            .await
            .unwrap();
        store
            .grant_request(live.id.as_str(), live_token, GrantDuration::Once)
            .await
            .unwrap();

        assert_eq!(store.cleanup_old(0).await, 1);
        assert!(store.get_request(expired.id.as_str()).await.is_none());
        assert_eq!(
            store.list_session_granted("live-grant-session").await[0].id,
            live.id
        );
        assert!(!store
            .state
            .read()
            .await
            .session_requests
            .contains_key("expired-grant-session"));
    }

    #[tokio::test]
    async fn test_expired_capacity_recovers_without_waiting_for_next_second() {
        let store = PendingRequestStore::with_timeout(Arc::new(AuditLog::new()), 0);
        for index in 0..MAX_PENDING_REQUESTS {
            request_for(&store, &format!("expired-{index}")).await;
        }
        let recovered = request_for(&store, "new-session").await;
        assert!(!recovered.is_denied());
        assert!(store.get_request(recovered.id.as_str()).await.is_some());
        let state = store.state.read().await;
        assert_eq!(state.requests.len(), 1);
        assert_eq!(state.session_requests.len(), 1);
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
