//! Pending capability request store
//!
//! Tracks capability requests that are awaiting user approval (grant/deny).
//! Requests have a timeout and are cleaned up periodically.
use std::collections::{HashMap, HashSet};
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

/// Maximum outstanding requests and retained live grants per session.
/// A session keeps its live grant records within its own bounded share.
pub const MAX_PENDING_PER_SESSION: usize = 32;

/// Runtime-owned request scope. Session scopes end with their authenticated
/// connection; Component Bus scopes retain their existing capsule ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestScope {
    Session(SessionId),
    Capsule(String),
}

impl From<SessionId> for RequestScope {
    fn from(session: SessionId) -> Self {
        Self::Session(session)
    }
}

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

    /// Only authenticated Session scopes participate in registry retirement.
    scope: RequestScope,

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
            scope: RequestScope::Session(session_id.clone()),
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
    session_requests: HashMap<RequestScope, Vec<String>>,
    /// Present only when this store belongs to a capability-enabled registry.
    /// Ended IDs leave this set instead of accumulating as tombstones.
    active_sessions: Option<HashSet<String>>,
}

impl PendingRequestState {
    fn session_is_active(&self, session_id: &str) -> bool {
        self.active_sessions
            .as_ref()
            .is_none_or(|active| active.contains(session_id))
    }

    fn session_admission_count(&self, scope: &RequestScope, now: &SecureTimestamp) -> usize {
        self.session_requests.get(scope).map_or(0, |ids| {
            ids.iter()
                .filter(|id| {
                    self.requests
                        .get(*id)
                        .is_some_and(|request| match &request.status {
                            RequestStatus::Pending => !request.expires_at.is_before(now),
                            RequestStatus::Granted { .. } => true,
                            RequestStatus::Denied { .. } | RequestStatus::Expired => false,
                        })
                })
                .count()
        })
    }

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

    pub(crate) fn with_session_lifecycle(audit_log: Arc<AuditLog>) -> Self {
        Self {
            state: RwLock::new(PendingRequestState {
                active_sessions: Some(HashSet::new()),
                ..PendingRequestState::default()
            }),
            audit_log,
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    pub(crate) async fn register_session(&self, session_id: &str) {
        let mut state = self.state.write().await;
        if let Some(active) = &mut state.active_sessions {
            active.insert(session_id.to_string());
        }
    }

    pub(crate) async fn session_is_active(&self, session_id: &str) -> bool {
        self.state.read().await.session_is_active(session_id)
    }

    pub(crate) async fn deactivate_session(&self, session_id: &str) {
        let mut state = self.state.write().await;
        if let Some(active) = &mut state.active_sessions {
            active.remove(session_id);
        }
    }

    pub(crate) async fn retire_session(&self, manager: &CapabilityManager, session_id: &str) {
        self.retire_session_with(session_id, |token_id| async move {
            manager.revoke(token_id, "Session ended").await;
        })
        .await;
    }

    async fn retire_session_with<F, Fut>(&self, session_id: &str, mut revoke: F)
    where
        F: FnMut(super::token::TokenId) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let tokens = {
            let mut state = self.state.write().await;
            if let Some(active) = &mut state.active_sessions {
                active.remove(session_id);
            }
            state
                .requests
                .values()
                .filter(|request| {
                    matches!(&request.scope, RequestScope::Session(id) if id.as_str() == session_id)
                })
                .filter_map(|request| request.granted_token().map(|token| *token.id()))
                .collect::<Vec<_>>()
        };

        // Inactive scopes cannot acquire new requests or grants. Retain their
        // records until revocation finishes so cancellation can be retried.
        // The registry retains its inactive session entry until this completes.
        for token_id in tokens {
            revoke(token_id).await;
        }

        let mut state = self.state.write().await;
        state.requests.retain(|_, request| {
            !(matches!(&request.scope, RequestScope::Session(id) if id.as_str() == session_id))
        });
        state.prune_session_index();
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

    /// Admit a request through Runtime-owned pressure reconciliation.
    ///
    /// The manager must be the Runtime owner that issued this store's grants.
    /// Reconciliation never consumes or grants authority. The final capacity
    /// decision and insertion share one write lock after reconciliation.
    pub async fn admit_request(
        &self,
        capability_manager: &CapabilityManager,
        scope: impl Into<RequestScope>,
        resource: ResourceId,
        action: Action,
        reason: String,
    ) -> PendingCapabilityRequest {
        let scope = scope.into();
        self.cleanup_retired_grants_at_capacity(capability_manager, Some(&scope))
            .await;
        self.create_request_with_reason(scope, resource, action, reason)
            .await
    }

    #[cfg(test)]
    async fn create_request(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
    ) -> PendingCapabilityRequest {
        self.create_request_with_reason(session_id.into(), resource, action, String::new())
            .await
    }

    async fn create_request_with_reason(
        &self,
        scope: RequestScope,
        resource: ResourceId,
        action: Action,
        reason: String,
    ) -> PendingCapabilityRequest {
        let session_id = match &scope {
            RequestScope::Session(session_id) => session_id.clone(),
            RequestScope::Capsule(capsule_id) => SessionId(capsule_id.clone()),
        };
        // Admission, cleanup, and both index updates are one state transition.
        let mut state = self.state.write().await;
        let now = SecureTimestamp::now();
        if state.requests.len() >= MAX_PENDING_REQUESTS
            || state.session_admission_count(&scope, &now) >= MAX_PENDING_PER_SESSION
        {
            state.cleanup_expired(&now);
            state.cleanup_old(&now, 0);
        }

        let denial = if matches!(scope, RequestScope::Session(_))
            && !state.session_is_active(session_id.as_str())
        {
            Some("Session has ended")
        } else if state.requests.len() >= MAX_PENDING_REQUESTS {
            Some("Too many pending requests")
        } else {
            (state.session_admission_count(&scope, &now) >= MAX_PENDING_PER_SESSION)
                .then_some("Too many outstanding requests or grants for this session")
        };

        let mut request = PendingCapabilityRequest::new_with_reason(
            session_id.clone(),
            resource.clone(),
            action,
            reason,
            self.timeout_secs,
        );
        request.scope = scope.clone();
        if let Some(reason) = denial {
            request.status = RequestStatus::Denied {
                reason: reason.to_string(),
            };
            return request;
        }

        // Exercise the decision/insertion boundary in race tests. The writer
        // remains held across this yield, so contenders cannot share a slot.
        #[cfg(test)]
        tokio::task::yield_now().await;
        state.requests.insert(request.id.0.clone(), request.clone());
        state
            .session_requests
            .entry(scope)
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
            .get(request_id)
            .ok_or_else(|| format!("Request not found: {}", request_id))?;
        if matches!(request.scope, RequestScope::Session(_))
            && !state.session_is_active(request.session_id.as_str())
        {
            return Err("Session has ended".to_string());
        }

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
            .get(&RequestScope::Session(SessionId::from_string(session_id)))
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

    /// Reclaim retired grants under global or requesting-session pressure.
    async fn cleanup_retired_grants_at_capacity(
        &self,
        capability_manager: &CapabilityManager,
        scope: Option<&RequestScope>,
    ) -> usize {
        self.reclaim_retired_grants_with(scope, |token| async move {
            capability_manager.is_revoked_or_exhausted(&token).await
        })
        .await
    }

    async fn reclaim_retired_grants_with<F, Fut>(
        &self,
        scope: Option<&RequestScope>,
        mut is_retired: F,
    ) -> usize
    where
        F: FnMut(CapabilityToken) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let candidates = {
            let state = self.state.read().await;
            let global_pressure = state.requests.len() >= MAX_PENDING_REQUESTS;
            let session_pressure = scope.is_some_and(|scope| {
                state.session_admission_count(scope, &SecureTimestamp::now())
                    >= MAX_PENDING_PER_SESSION
            });
            if !global_pressure && !session_pressure {
                return 0;
            }
            state
                .requests
                .iter()
                .filter(|(_, request)| global_pressure || Some(&request.scope) == scope)
                .filter_map(|(id, request)| match &request.status {
                    RequestStatus::Granted { token, duration } => {
                        Some((id.clone(), token.as_ref().clone(), *duration))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // Manager queries can await without holding any pending-state guard.
        let mut retired = Vec::new();
        for (id, token, duration) in candidates {
            let token_id = *token.id();
            if is_retired(token).await {
                retired.push((id, token_id, duration));
            }
        }

        let mut state = self.state.write().await;
        let mut removed = 0;
        for (id, token_id, duration) in retired {
            // A granted record's fields are immutable until its status changes.
            // Recheck status, token identity and duration before removing it.
            let unchanged = state.requests.get(&id).is_some_and(|request| {
                matches!(&request.status, RequestStatus::Granted { token, duration: current_duration }
                    if *token.id() == token_id && *current_duration == duration)
            });
            if unchanged {
                state.requests.remove(&id);
                removed += 1;
            }
        }
        if removed != 0 {
            state.prune_session_index();
        }
        removed
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
            .get(&RequestScope::Session(SessionId::from_string(session_id)))
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

    /// Mark grants from epochs older than the persisted revocation epoch revoked.
    ///
    /// Called when epoch is advanced to revoke all capabilities
    pub async fn revoke_all_granted_before_epoch(&self, new_epoch: u64) {
        let mut state = self.state.write().await;
        let now = SecureTimestamp::now();
        let mut events = Vec::new();

        for (request_id, request) in state.requests.iter_mut() {
            if request
                .granted_token()
                .is_some_and(|token| token.constraints().epoch() < new_epoch)
            {
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
    use crate::capability::{CapabilityStore, TokenConstraints};
    use crate::primitives::metrics::MetricsManager;
    use crate::session::{SessionRegistry, SessionType};

    fn lifecycle_store() -> (
        SessionRegistry,
        Arc<PendingRequestStore>,
        Arc<CapabilityManager>,
    ) {
        let audit = Arc::new(AuditLog::new());
        let manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            audit.clone(),
            Arc::new(MetricsManager::new()),
        ));
        let (registry, pending) = SessionRegistry::with_capabilities(audit, manager.clone());
        (registry, pending, manager)
    }

    async fn lifecycle_grant(
        pending: &PendingRequestStore,
        manager: &CapabilityManager,
        scope: impl Into<RequestScope>,
    ) -> (PendingCapabilityRequest, CapabilityToken) {
        let request = pending
            .admit_request(
                manager,
                scope,
                ResourceId::new("localhost://Users/self/Documents/lifecycle"),
                Action::Read,
                "session lifecycle".to_string(),
            )
            .await;
        assert!(request.is_pending());
        let token = manager.grant(
            request.session_id.as_str(),
            request.resource.clone(),
            request.action,
            TokenConstraints::default(),
            None,
        );
        pending
            .grant_request(request.id.as_str(), token.clone(), GrantDuration::Session)
            .await
            .unwrap();
        (request, token)
    }

    #[tokio::test]
    async fn test_ended_sessions_release_full_global_grant_capacity() {
        let (registry, pending, manager) = lifecycle_store();
        let mut grants = Vec::new();
        for _ in 0..MAX_PENDING_REQUESTS / MAX_PENDING_PER_SESSION {
            let session = registry.create_session(SessionType::Capsule, None).await;
            for _ in 0..MAX_PENDING_PER_SESSION {
                grants.push(lifecycle_grant(&pending, &manager, session.id.clone()).await);
            }
            registry
                .get_session_mut(&session.token, |session| {
                    session.last_active = SecureTimestamp::at(0);
                })
                .await
                .unwrap();
        }
        assert_eq!(grants.len(), 32 * 32);
        assert_eq!(
            pending.state.read().await.requests.len(),
            MAX_PENDING_REQUESTS
        );
        let fresh = registry.create_session(SessionType::Capsule, None).await;
        assert!(request_for(&pending, fresh.id.as_str()).await.is_denied());

        assert_eq!(registry.cleanup_stale_sessions(600).await, 32);
        let admitted = pending
            .admit_request(
                &manager,
                fresh.id.clone(),
                ResourceId::new("localhost://Users/self/Documents/fresh"),
                Action::Read,
                String::new(),
            )
            .await;
        assert!(
            admitted.is_pending(),
            "ended sessions must release global grant capacity"
        );
        for (request, token) in grants {
            assert!(pending.get_request(request.id.as_str()).await.is_none());
            assert!(manager
                .validate(
                    &token,
                    request.session_id.as_str(),
                    request.action,
                    &request.resource,
                    None
                )
                .await
                .is_err());
        }
        let state = pending.state.read().await;
        assert_eq!(state.requests.len(), 1);
        assert_eq!(state.session_requests.len(), 1);
        assert_eq!(state.active_sessions.as_ref().unwrap().len(), 1);
        assert_eq!(
            state.session_requests[&RequestScope::Session(fresh.id.clone())],
            vec![admitted.id.0]
        );
    }

    #[tokio::test]
    async fn test_all_session_end_paths_retire_grants_and_preserve_active_sessions() {
        for mode in 0..4 {
            let (registry, pending, manager) = lifecycle_store();
            let ended = registry
                .create_session(SessionType::Capsule, Some("ended".to_string()))
                .await;
            let active = registry
                .create_session(SessionType::Capsule, Some("active".to_string()))
                .await;
            let (request, token) = lifecycle_grant(&pending, &manager, ended.id.clone()).await;
            let (live_request, live_token) =
                lifecycle_grant(&pending, &manager, active.id.clone()).await;
            match mode {
                0 => {
                    registry.invalidate_session(&ended.token).await.unwrap();
                }
                1 => {
                    registry.invalidate_vm_session("ended").await.unwrap();
                }
                2 => {
                    assert_eq!(
                        registry
                            .cleanup_dead_vm_sessions(&["active".to_string()])
                            .await,
                        1
                    );
                }
                _ => {
                    registry
                        .get_session_mut(&ended.token, |session| {
                            session.last_active = SecureTimestamp::at(0)
                        })
                        .await
                        .unwrap();
                    assert_eq!(registry.cleanup_stale_sessions(600).await, 1);
                }
            }
            assert!(registry.validate_token(&ended.token).await.is_none());
            assert!(registry.get_vm_token("ended").await.is_none());
            assert_eq!(registry.session_count().await, 1);
            assert!(pending.get_request(request.id.as_str()).await.is_none());
            assert!(request_for(&pending, ended.id.as_str()).await.is_denied());
            assert!(manager
                .validate(
                    &token,
                    ended.id.as_str(),
                    request.action,
                    &request.resource,
                    None
                )
                .await
                .is_err());
            assert!(registry.validate_token(&active.token).await.is_some());
            assert_eq!(
                pending.list_session_granted(active.id.as_str()).await.len(),
                1
            );
            manager
                .validate(
                    &live_token,
                    active.id.as_str(),
                    live_request.action,
                    &live_request.resource,
                    None,
                )
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn test_canceled_session_retirement_fails_closed_and_cleanup_retries() {
        use std::future::Future as _;
        use std::task::{Context, Waker};

        let (registry, pending, manager) = lifecycle_store();
        let session = registry
            .create_session(SessionType::Shell, Some("retiring".to_string()))
            .await;
        let (granted, token) = lifecycle_grant(&pending, &manager, session.id.clone()).await;
        let late = request_for(&pending, session.id.as_str()).await;
        let late_token = manager.grant(
            session.id.as_str(),
            late.resource.clone(),
            late.action,
            TokenConstraints::default(),
            None,
        );
        let mut retiring = Box::pin(tokio::task::unconstrained(
            pending.retire_session_with(session.id.as_str(), |_| std::future::pending::<()>()),
        ));
        assert!(retiring
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        drop(retiring);

        assert!(registry.validate_token(&session.token).await.is_none());
        assert!(!registry.is_shell(&session.token).await);
        assert!(!registry.has_vm_session("retiring").await);
        assert!(registry.get_vm_token("retiring").await.is_none());
        assert!(registry.list_sessions().await.is_empty());
        assert_eq!(registry.session_count().await, 0);
        assert!(registry
            .get_session_mut(&session.token, |_| ())
            .await
            .is_none());
        registry.touch_session(&session.token).await;
        assert!(
            request_for(&pending, session.id.as_str()).await.is_denied(),
            "ended session must reject late admission"
        );
        assert!(
            pending
                .grant_request(late.id.as_str(), late_token, GrantDuration::Session)
                .await
                .is_err(),
            "ended session must reject a late grant"
        );
        // The canceled operation preserves the identities needed for retry.
        assert!(pending.get_request(granted.id.as_str()).await.is_some());
        assert!(pending.get_request(late.id.as_str()).await.is_some());
        assert_eq!(registry.cleanup_stale_sessions(u64::MAX).await, 1);
        assert!(manager
            .validate(
                &token,
                session.id.as_str(),
                granted.action,
                &granted.resource,
                None
            )
            .await
            .is_err());
        let state = pending.state.read().await;
        assert!(state.requests.is_empty());
        assert!(state.session_requests.is_empty());
        assert!(state.active_sessions.as_ref().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_session_retirement_preserves_capsule_scope_with_same_owner_string() {
        let (registry, pending, manager) = lifecycle_store();
        let session = registry.create_session(SessionType::Capsule, None).await;
        let mut session_grants = Vec::new();
        for _ in 0..MAX_PENDING_PER_SESSION {
            session_grants.push(lifecycle_grant(&pending, &manager, session.id.clone()).await);
        }
        assert!(request_for(&pending, session.id.as_str()).await.is_denied());
        let (capsule_request, capsule_token) = lifecycle_grant(
            &pending,
            &manager,
            RequestScope::Capsule(session.id.to_string()),
        )
        .await;
        assert_eq!(
            pending
                .list_session_granted(session.id.as_str())
                .await
                .len(),
            MAX_PENDING_PER_SESSION
        );
        assert_eq!(pending.state.read().await.session_requests.len(), 2);
        registry.invalidate_session(&session.token).await.unwrap();
        for (session_request, session_token) in session_grants {
            assert!(pending
                .get_request(session_request.id.as_str())
                .await
                .is_none());
            assert!(manager
                .validate(
                    &session_token,
                    session.id.as_str(),
                    session_request.action,
                    &session_request.resource,
                    None
                )
                .await
                .is_err());
        }
        assert!(pending
            .list_session_granted(session.id.as_str())
            .await
            .is_empty());
        assert!(pending
            .get_request(capsule_request.id.as_str())
            .await
            .unwrap()
            .is_granted());
        manager
            .validate(
                &capsule_token,
                session.id.as_str(),
                capsule_request.action,
                &capsule_request.resource,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            pending.state.read().await.session_requests
                [&RequestScope::Capsule(session.id.to_string())],
            vec![capsule_request.id.0]
        );
        let second_capsule = lifecycle_grant(
            &pending,
            &manager,
            RequestScope::Capsule(session.id.to_string()),
        )
        .await;
        assert!(pending
            .get_request(second_capsule.0.id.as_str())
            .await
            .unwrap()
            .is_granted());
    }

    #[tokio::test]
    async fn test_epoch_retirement_preserves_grant_landing_after_advance() {
        let (registry, pending, manager) = lifecycle_store();
        let session = registry.create_session(SessionType::Capsule, None).await;
        let (old_request, old_token) =
            lifecycle_grant(&pending, &manager, session.id.clone()).await;
        let epoch = manager.revoke_all("test epoch race").unwrap();
        // Deterministically place a new-epoch grant in the handler's gap.
        let (new_request, new_token) =
            lifecycle_grant(&pending, &manager, session.id.clone()).await;
        assert_eq!(new_token.constraints().epoch(), epoch);
        pending.revoke_all_granted_before_epoch(epoch).await;
        assert!(pending
            .get_request(old_request.id.as_str())
            .await
            .unwrap()
            .is_denied());
        let visible = pending.list_session_granted(session.id.as_str()).await;
        assert_eq!(
            visible.len(),
            1,
            "new-epoch grant must remain visible and revocable"
        );
        assert_eq!(visible[0].id, new_request.id);
        assert!(manager
            .validate(
                &old_token,
                session.id.as_str(),
                old_request.action,
                &old_request.resource,
                None
            )
            .await
            .is_err());
        manager
            .validate(
                &new_token,
                session.id.as_str(),
                new_request.action,
                &new_request.resource,
                None,
            )
            .await
            .unwrap();
    }

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
        use std::future::Future as _;
        use std::task::{Context, Waker};

        const CONTENDERS: usize = 64;
        let mut contenders = Vec::new();
        for index in 0..CONTENDERS {
            let store = Arc::clone(&store);
            contenders.push(Box::pin(tokio::task::unconstrained(async move {
                let session = if shared_session {
                    "shared-session".to_string()
                } else {
                    format!("concurrent-{index}")
                };
                request_for(&store, &session).await.is_pending()
            })));
        }

        // Poll every contender once before any can resume past the injected
        // decision/insertion yield. Correct code keeps the first writer held,
        // so the rest wait before checking capacity. If the writer is split at
        // that boundary, every contender checks the same last slot first.
        // Exclude Tokio's cooperative-budget yield from this first-poll probe;
        // the only suspension points are the lock and our explicit boundary.
        {
            let mut context = Context::from_waker(Waker::noop());
            for contender in &mut contenders {
                assert!(contender.as_mut().poll(&mut context).is_pending());
            }
        }
        let mut tasks = tokio::task::JoinSet::new();
        for contender in contenders {
            tasks.spawn(contender);
        }
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

    #[tokio::test]
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

    #[tokio::test]
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
            .contains_key(&RequestScope::Session(SessionId::from_string(
                "expired-session"
            ))));

        // Individual and epoch revocation make grant records eligible for cleanup.
        store.revoke_request(once.id.as_str()).await;
        assert_eq!(store.cleanup_old(0).await, 1);
        assert!(store.list_session_granted("mixed-session").await.is_empty());
        store.revoke_all_granted_before_epoch(1).await;
        assert_eq!(store.cleanup_old(0).await, 1);
        assert!(!store
            .state
            .read()
            .await
            .session_requests
            .contains_key(&RequestScope::Session(SessionId::from_string(
                "grant-session"
            ))));
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
    async fn test_retained_live_grants_fill_only_the_owning_session_share() {
        let store = create_test_store();
        for _ in 0..MAX_PENDING_PER_SESSION {
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
        assert!(request_for(&store, "grant-session").await.is_denied());
        assert!(request_for(&store, "new-session").await.is_pending());
        assert_eq!(
            store.list_session_granted("grant-session").await.len(),
            MAX_PENDING_PER_SESSION
        );
        assert_eq!(store.cleanup_old(0).await, 0);

        store.revoke_all_granted_before_epoch(1).await;
        assert!(request_for(&store, "grant-session").await.is_pending());
        assert!(store.list_session_granted("grant-session").await.is_empty());
    }

    #[tokio::test]
    async fn test_admission_reclaims_session_share_without_spending_live_grants() {
        use crate::capability::{CapabilityStore, TokenConstraints};
        use crate::primitives::metrics::MetricsManager;

        for revoke in [false, true] {
            let audit = Arc::new(AuditLog::new());
            let authority = Arc::new(CapabilityStore::new());
            let manager = CapabilityManager::new(
                authority.clone(),
                audit.clone(),
                Arc::new(MetricsManager::new()),
            );
            let store = PendingRequestStore::new(audit);
            let mut grants = Vec::new();
            for _ in 0..MAX_PENDING_PER_SESSION {
                let request = store
                    .admit_request(
                        &manager,
                        SessionId::from_string("owner"),
                        ResourceId::new("localhost://Users/self/Documents/test"),
                        Action::Read,
                        "bounded share".to_string(),
                    )
                    .await;
                assert!(request.is_pending());
                let token = manager.grant(
                    "owner",
                    request.resource.clone(),
                    request.action,
                    TokenConstraints::new(0, false, None, Some(1)),
                    None,
                );
                store
                    .grant_request(request.id.as_str(), token.clone(), GrantDuration::Once)
                    .await
                    .unwrap();
                grants.push((request, token));
            }
            let admit = |session| {
                store.admit_request(
                    &manager,
                    SessionId::from_string(session),
                    ResourceId::new("localhost://Users/self/Documents/test"),
                    Action::Read,
                    String::new(),
                )
            };
            assert!(admit("owner").await.is_denied());
            assert!(admit("another-session").await.is_pending());
            assert_eq!(
                store.list_session_granted("owner").await.len(),
                MAX_PENDING_PER_SESSION
            );
            let (retired, token) = &grants[0];
            if revoke {
                manager.revoke(*token.id(), "share recovery").await;
            } else {
                manager
                    .validate(token, "owner", Action::Read, &retired.resource, None)
                    .await
                    .unwrap();
            }
            assert!(admit("owner").await.is_pending());
            assert!(store.get_request(retired.id.as_str()).await.is_none());
            assert_eq!(
                store.list_session_granted("owner").await.len(),
                MAX_PENDING_PER_SESSION - 1
            );
            assert_eq!(
                authority.get_use_count(token.id()).await,
                u32::from(!revoke)
            );
            for (request, live_token) in &grants[1..] {
                assert!(store
                    .get_request(request.id.as_str())
                    .await
                    .unwrap()
                    .is_granted());
                assert_eq!(authority.get_use_count(live_token.id()).await, 0);
            }
            assert_eq!(
                store.state.read().await.requests.len(),
                MAX_PENDING_PER_SESSION + 1
            );
        }
    }

    #[tokio::test]
    async fn test_reclaim_queries_without_pending_lock_and_preserves_changed_records() {
        let store = create_test_store();
        let mut granted = Vec::new();
        for _ in 0..4 {
            let request = request_for(&store, "owner").await;
            let token = token_for(&request);
            store
                .grant_request(request.id.as_str(), token.clone(), GrantDuration::Session)
                .await
                .unwrap();
            granted.push((request, token));
        }
        for _ in 4..MAX_PENDING_PER_SESSION {
            assert!(request_for(&store, "owner").await.is_pending());
        }
        let removed = store
            .reclaim_retired_grants_with(
                Some(&RequestScope::Session(SessionId::from_string("owner"))),
                |token| {
                    let store = &store;
                    let granted = &granted;
                    async move {
                        // This fails immediately if either a read or write guard escaped
                        // the snapshot scope. It does not rely on a timeout or scheduler.
                        let mut state = store
                            .state
                            .try_write()
                            .expect("manager query holds pending lock");
                        let index = granted
                            .iter()
                            .position(|(_, old)| old.id() == token.id())
                            .unwrap();
                        let request = state
                            .requests
                            .get_mut(granted[index].0.id.as_str())
                            .unwrap();
                        match index {
                            0 => {}
                            1 => {
                                request.status = RequestStatus::Denied {
                                    reason: "changed during query".into(),
                                }
                            }
                            2 => {
                                request.status = RequestStatus::Granted {
                                    token: Box::new(token_for(request)),
                                    duration: GrantDuration::Session,
                                }
                            }
                            3 => {
                                request.status = RequestStatus::Granted {
                                    token: Box::new(token),
                                    duration: GrantDuration::Once,
                                }
                            }
                            _ => unreachable!(),
                        }
                        drop(state);
                        tokio::task::yield_now().await;
                        true
                    }
                },
            )
            .await;
        assert_eq!(removed, 1);
        assert!(store.get_request(granted[0].0.id.as_str()).await.is_none());
        assert!(store
            .get_request(granted[1].0.id.as_str())
            .await
            .unwrap()
            .is_denied());
        let replacement = store.get_request(granted[2].0.id.as_str()).await.unwrap();
        assert_ne!(replacement.granted_token().unwrap().id(), granted[2].1.id());
        assert!(matches!(
            store
                .get_request(granted[3].0.id.as_str())
                .await
                .unwrap()
                .status,
            RequestStatus::Granted {
                duration: GrantDuration::Once,
                ..
            }
        ));
        let state = store.state.read().await;
        assert_eq!(state.requests.len(), MAX_PENDING_PER_SESSION - 1);
        assert_eq!(
            state.session_requests[&RequestScope::Session(SessionId::from_string("owner"))].len(),
            state.requests.len()
        );
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
        assert_eq!(
            store
                .cleanup_retired_grants_at_capacity(&manager, None)
                .await,
            0
        );
        for index in 0..MAX_PENDING_REQUESTS - grants.len() {
            assert!(request_for(&store, &format!("prefill-{index}"))
                .await
                .is_pending());
        }

        assert_eq!(
            store
                .cleanup_retired_grants_at_capacity(&manager, None)
                .await,
            2
        );
        for (index, (request, token, used)) in grants.iter().enumerate() {
            assert_eq!(
                store.get_request(request.id.as_str()).await.is_some(),
                index >= 2
            );
            assert_eq!(counts.get_use_count(token.id()).await, *used);
        }
        let state = store.state.read().await;
        assert!(!state.session_requests.contains_key(&RequestScope::Session(
            SessionId::from_string("exhausted-only")
        )));
        assert_eq!(
            state.session_requests[&RequestScope::Session(SessionId::from_string("mixed"))].len(),
            3
        );
        assert_eq!(state.requests.len(), MAX_PENDING_REQUESTS - 2);
        assert_eq!(
            state.session_requests.values().map(Vec::len).sum::<usize>(),
            state.requests.len()
        );
        drop(state);

        assert!(request_for(&store, "recovered").await.is_pending());
        assert!(request_for(&store, "recovered").await.is_pending());
        assert_eq!(
            store
                .cleanup_retired_grants_at_capacity(&manager, None)
                .await,
            0
        );
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
        let epoch = manager.revoke_all("retire old epoch").unwrap();
        store.revoke_all_granted_before_epoch(epoch).await;
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

        assert_eq!(
            store
                .cleanup_retired_grants_at_capacity(&manager, None)
                .await,
            2
        );
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
        assert!(!state.session_requests.contains_key(&RequestScope::Session(
            SessionId::from_string("stale-epoch")
        )));
        assert_eq!(
            state.session_requests[&RequestScope::Session(SessionId::from_string("mixed"))].len(),
            1
        );
        assert_eq!(state.requests.len(), MAX_PENDING_REQUESTS - 2);
        assert_eq!(
            state.session_requests.values().map(Vec::len).sum::<usize>(),
            state.requests.len()
        );
        drop(state);

        assert!(request_for(&store, "recovered").await.is_pending());
        assert!(request_for(&store, "recovered").await.is_pending());
        assert_eq!(
            store
                .cleanup_retired_grants_at_capacity(&manager, None)
                .await,
            0
        );
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
            .contains_key(&RequestScope::Session(SessionId::from_string(
                "expired-grant-session"
            ))));
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
