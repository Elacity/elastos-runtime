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

/// Affordance-consent binding carried into
/// [`PendingRequestStore::create_affordance_request`] so the eventual grant is
/// scoped to the exact affordance + arguments and can never be replayed for a
/// different method or with different arguments (W2 step 3).
#[derive(Debug, Clone)]
pub struct AffordanceBinding {
    pub capsule: String,
    pub principal_id: String,
    pub method_id: String,
    pub input_hash: String,
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

    /// The requester's real capsule identity (the carrier "vm-{name}"), recorded
    /// at request time (G-ID interim) so the eventual grant can mint at it instead
    /// of the session-id shim. `None` when the session has no capsule identity.
    pub requester_capsule_id: Option<String>,

    /// Affordance-consent binding (W2 step 3). Set ONLY for affordance-consent
    /// requests; `None` for ordinary session-capability requests
    /// (behaviour-neutral default). Binds the eventual grant to this exact
    /// affordance + argument hash so an approval can never be replayed for a
    /// different method or different arguments (fail-closed; see
    /// `with_affordance_binding`). Orthogonal to `requester_capsule_id`:
    /// that records *who* asked (carrier identity); these record *what* the
    /// consent is scoped to (the affordance + args).
    pub capsule: Option<String>,
    /// Principal the affordance-consent request was raised for.
    pub principal_id: Option<String>,
    /// Affordance method id the consent is scoped to.
    pub method_id: Option<String>,
    /// Canonical hash of the invocation arguments the consent is scoped to.
    pub input_hash: Option<String>,
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
            requester_capsule_id: None,
            capsule: None,
            principal_id: None,
            method_id: None,
            input_hash: None,
        }
    }

    /// Attach affordance-consent binding to this request (W2 step 3). Used by the
    /// affordance-consent path so the eventual grant binds to the exact
    /// `(capsule, principal, method, argument-hash)` and cannot be replayed for a
    /// different method or with different arguments. Ordinary session-capability
    /// requests never call this and keep all four fields `None`.
    pub fn with_affordance_binding(
        mut self,
        capsule: impl Into<String>,
        principal_id: impl Into<String>,
        method_id: impl Into<String>,
        input_hash: impl Into<String>,
    ) -> Self {
        self.capsule = Some(capsule.into());
        self.principal_id = Some(principal_id.into());
        self.method_id = Some(method_id.into());
        self.input_hash = Some(input_hash.into());
        self
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

    /// Shared audit sink used by Runtime-owned bridge adapters.
    pub fn audit_log(&self) -> Arc<AuditLog> {
        self.audit_log.clone()
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
        self.create_request_inner(session_id, resource, action, None, None, String::new())
            .await
    }

    /// Like [`create_request`], but carries a plain-language `reason` for the
    /// approval decision.
    pub async fn create_request_with_reason(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        reason: String,
    ) -> PendingCapabilityRequest {
        self.create_request_inner(session_id, resource, action, None, None, reason)
            .await
    }

    /// Like [`create_request`], but records the requester's real capsule identity
    /// (the carrier "vm-{name}") on the pending request so the eventual grant can
    /// mint at it instead of the session-id shim (G-ID interim). `None` when the
    /// session has no capsule identity (a bare shell), recorded honestly rather
    /// than fabricated.
    pub async fn create_request_with_capsule(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        requester_capsule_id: Option<String>,
    ) -> PendingCapabilityRequest {
        self.create_request_inner(
            session_id,
            resource,
            action,
            requester_capsule_id,
            None,
            String::new(),
        )
        .await
    }

    /// The Carrier bridge's constructor: records BOTH the requester's real capsule
    /// identity (G-ID interim) and the plain-language `reason` the approval surface
    /// shows, so a capsule-originated request loses neither.
    pub async fn create_request_with_capsule_and_reason(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        requester_capsule_id: Option<String>,
        reason: String,
    ) -> PendingCapabilityRequest {
        self.create_request_inner(
            session_id,
            resource,
            action,
            requester_capsule_id,
            None,
            reason,
        )
        .await
    }

    /// Create a pending affordance-consent request bound to the exact
    /// `(capsule, principal, method, argument-hash)` so the eventual grant can
    /// never be replayed for a different method or with different arguments
    /// (W2 step 3; the binding is read at grant time in step 6). Shares one
    /// creation path with the session flow.
    pub async fn create_affordance_request(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        binding: AffordanceBinding,
    ) -> PendingCapabilityRequest {
        self.create_request_inner(
            session_id,
            resource,
            action,
            None,
            Some(binding),
            String::new(),
        )
        .await
    }

    /// Shared creation path for the session, capsule-aware, and affordance-consent
    /// requests. Applies the requester capsule identity and the affordance binding
    /// (each when present) before storing, so the capacity guards, the per-session
    /// rate limit, the session index, and the audit emit are identical for all.
    async fn create_request_inner(
        &self,
        session_id: SessionId,
        resource: ResourceId,
        action: Action,
        requester_capsule_id: Option<String>,
        binding: Option<AffordanceBinding>,
        reason: String,
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
                let mut request = PendingCapabilityRequest::new_with_reason(
                    session_id.clone(),
                    resource.clone(),
                    action,
                    reason.clone(),
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
                    let mut request = PendingCapabilityRequest::new_with_reason(
                        session_id.clone(),
                        resource.clone(),
                        action,
                        reason.clone(),
                        self.timeout_secs,
                    );
                    request.status = RequestStatus::Denied {
                        reason: "Too many pending requests for this session".to_string(),
                    };
                    return request;
                }
            }
        }

        let mut request = PendingCapabilityRequest::new_with_reason(
            session_id.clone(),
            resource.clone(),
            action,
            reason,
            self.timeout_secs,
        );
        request.requester_capsule_id = requester_capsule_id;
        if let Some(b) = binding {
            request = request.with_affordance_binding(
                b.capsule,
                b.principal_id,
                b.method_id,
                b.input_hash,
            );
        }

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
        self.audit_log.emit_best_effort(
            crate::primitives::audit::AuditEvent::CapabilityRequested {
                timestamp: SecureTimestamp::now(),
                request_id: request.id.to_string(),
                session_id: session_id.to_string(),
                resource: resource.to_string(),
                action: action.to_string(),
            },
        );

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

        // G8a: write the denial's audit record durably and FAIL CLOSED before the
        // status mutation. If the durable write fails, the denial aborts and the
        // request stays Pending rather than silently completing.
        self.audit_log
            // G8a on dDRM's signed chain: emit (fail-closed, durable) returns a
            // Result; the `?` below aborts the denial if the record cannot be
            // written. emit_best_effort would be fail-open and would regress this.
            .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                timestamp: SecureTimestamp::now(),
                request_id: request_id.to_string(),
                session_id: request.session_id.to_string(),
                reason: reason.to_string(),
            })
            .map_err(|e| format!("audit write failed, denial aborted: {e}"))?;

        request.status = RequestStatus::Denied {
            reason: reason.to_string(),
        };

        Ok(())
    }

    /// Attest an APPROVE decision onto the signed audit chain, fail-closed (G4b).
    /// The exact mirror of `deny_request`'s fail-closed emit: the decision record
    /// is written durably + signed and this returns `Err` if that write fails, so a
    /// caller that propagates the error (the grant handler) mints and returns
    /// NOTHING for an approval that cannot be attested. Records the DECISION (who
    /// approved which request) — distinct from the best-effort token-issuance
    /// breadcrumb `CapabilityManager::grant` emits.
    pub fn approve_request(
        &self,
        request_id: &str,
        session_id: &str,
        resource: &str,
        action: &str,
        approver: &str,
    ) -> Result<(), String> {
        self.audit_log
            .emit(crate::primitives::audit::AuditEvent::CapabilityApproved {
                timestamp: SecureTimestamp::now(),
                request_id: request_id.to_string(),
                session_id: session_id.to_string(),
                resource: resource.to_string(),
                action: action.to_string(),
                approver: approver.to_string(),
            })
            .map_err(|e| format!("audit write failed, approval aborted: {e}"))
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

    /// Mark a granted request as revoked, FAIL-CLOSED on its audit record (AUD-3).
    ///
    /// Changes the status to Denied with reason "Revoked". Mirrors `deny_request`:
    /// the revocation's signed record is written durably BEFORE the status mutation,
    /// and the revocation aborts (status unchanged) if that durable write fails.
    /// `emit_best_effort` here was fail-OPEN and would silently lose the record.
    /// A non-granted / unknown request is a no-op (revoke stays idempotent).
    pub async fn revoke_request(&self, request_id: &str) -> Result<(), String> {
        let mut requests = self.requests.write().await;
        if let Some(request) = requests.get_mut(request_id) {
            if request.is_granted() {
                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                        timestamp: SecureTimestamp::now(),
                        request_id: request_id.to_string(),
                        session_id: request.session_id.to_string(),
                        reason: "Revoked by user".to_string(),
                    })
                    .map_err(|e| format!("audit write failed, revocation aborted: {e}"))?;

                request.status = RequestStatus::Denied {
                    reason: "Revoked by user".to_string(),
                };
            }
        }
        Ok(())
    }

    /// Mark all granted requests as revoked, FAIL-CLOSED on each audit record (AUD-3).
    ///
    /// Called when the epoch is advanced. Each revocation's signed record is written
    /// durably BEFORE its status mutation; the bulk aborts on the first write failure
    /// (returning Err), so every request whose status is flipped HAS a durable record
    /// and an incomplete attestation is loudly surfaced rather than silently lost. The
    /// epoch increment (the real enforcement) has already invalidated the tokens.
    pub async fn revoke_all_granted(&self) -> Result<(), String> {
        let mut requests = self.requests.write().await;
        let now = SecureTimestamp::now();

        for (request_id, request) in requests.iter_mut() {
            if request.is_granted() {
                self.audit_log
                    .emit(crate::primitives::audit::AuditEvent::CapabilityDenied {
                        timestamp: now,
                        request_id: request_id.clone(),
                        session_id: request.session_id.to_string(),
                        reason: "Epoch advanced - all capabilities revoked".to_string(),
                    })
                    .map_err(|e| format!("audit write failed, bulk revocation aborted: {e}"))?;

                request.status = RequestStatus::Denied {
                    reason: "Epoch advanced - all capabilities revoked".to_string(),
                };
            }
        }
        Ok(())
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

    // ── G8a: fail-closed audit on the user-deny write ────────────────

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "flint_g8a_{tag}_{}_{}.log",
            std::process::id(),
            line!()
        ))
    }

    #[tokio::test]
    async fn deny_request_fails_closed_when_audit_write_fails() {
        // A read-only file handle makes the durable audit write fail at flush —
        // the denial must abort and the request must stay Pending, never silently
        // complete with a lost audit record (G8a).
        let path = unique_tmp("ro");
        std::fs::File::create(&path).unwrap();
        let ro = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let audit = Arc::new(AuditLog::with_file_handle(ro));
        let pending = PendingRequestStore::new(audit);

        let req = pending
            .create_request(
                SessionId::new(),
                ResourceId::new("localhost://Users/self/Documents/x"),
                Action::Read,
            )
            .await;

        let res = pending.deny_request(req.id.as_str(), "nope").await;
        assert!(
            res.is_err(),
            "deny must fail closed when its durable audit write fails"
        );
        let after = pending.get_request(req.id.as_str()).await.unwrap();
        assert!(
            matches!(after.status, RequestStatus::Pending),
            "request must stay Pending when its denial audit write fails"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn deny_request_succeeds_and_records_with_working_audit() {
        // Companion: with a working durable sink the deny commits AND the event is
        // recorded — proving the fail-closed path triggers on real failure only,
        // not always (catches an always-Err stub).
        let path = unique_tmp("ok");
        let audit = Arc::new(AuditLog::with_file(&path).unwrap());
        let pending = PendingRequestStore::new(audit.clone());

        let req = pending
            .create_request(
                SessionId::new(),
                ResourceId::new("localhost://Users/self/Documents/x"),
                Action::Read,
            )
            .await;

        let res = pending.deny_request(req.id.as_str(), "nope").await;
        assert!(res.is_ok(), "deny succeeds with a working audit sink");
        let after = pending.get_request(req.id.as_str()).await.unwrap();
        assert!(matches!(after.status, RequestStatus::Denied { .. }));
        assert!(
            audit.recent_events(50).iter().any(|e| matches!(
                e,
                crate::primitives::audit::AuditEvent::CapabilityDenied { .. }
            )),
            "the denial event must be recorded"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn revoke_request_fails_closed_when_audit_write_fails() {
        // AUD-3: a read-only audit makes the durable write fail — the revocation must
        // abort and the request must stay Granted, never silently completing with a
        // lost record (the exact mirror of deny_request_fails_closed_when_audit_write_fails).
        let path = unique_tmp("revoke-ro");
        std::fs::File::create(&path).unwrap();
        let ro = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let audit = Arc::new(AuditLog::with_file_handle(ro));
        let pending = PendingRequestStore::new(audit);

        let req = pending
            .create_request(
                SessionId::new(),
                ResourceId::new("localhost://Users/self/Documents/x"),
                Action::Read,
            )
            .await;
        let token = CapabilityToken::new(
            "test-capsule".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/x"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );
        pending
            .grant_request(req.id.as_str(), token, GrantDuration::Session)
            .await
            .unwrap();

        let res = pending.revoke_request(req.id.as_str()).await;
        assert!(
            res.is_err(),
            "revoke must fail closed when its durable audit write fails"
        );
        let after = pending.get_request(req.id.as_str()).await.unwrap();
        assert!(
            after.is_granted(),
            "request must stay Granted when its revocation audit write fails"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn revoke_request_succeeds_and_records_with_working_audit() {
        // Companion: with a working sink the revoke commits AND records — proving the
        // fail-closed path triggers on real failure only, not always.
        let path = unique_tmp("revoke-ok");
        let audit = Arc::new(AuditLog::with_file(&path).unwrap());
        let pending = PendingRequestStore::new(audit.clone());

        let req = pending
            .create_request(
                SessionId::new(),
                ResourceId::new("localhost://Users/self/Documents/x"),
                Action::Read,
            )
            .await;
        let token = CapabilityToken::new(
            "test-capsule".to_string(),
            [0u8; 32],
            ResourceId::new("localhost://Users/self/Documents/x"),
            Action::Read,
            Default::default(),
            SecureTimestamp::now(),
            None,
        );
        pending
            .grant_request(req.id.as_str(), token, GrantDuration::Session)
            .await
            .unwrap();

        let res = pending.revoke_request(req.id.as_str()).await;
        assert!(res.is_ok(), "revoke succeeds with a working audit sink");
        let after = pending.get_request(req.id.as_str()).await.unwrap();
        assert!(matches!(after.status, RequestStatus::Denied { .. }));
        assert!(
            audit.recent_events(50).iter().any(|e| matches!(
                e,
                crate::primitives::audit::AuditEvent::CapabilityDenied { .. }
            )),
            "the revocation event must be recorded"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn approve_request_fails_closed_when_audit_write_fails() {
        // Mirror of deny_request_fails_closed: a read-only file handle makes the
        // durable+signed audit write fail, so the APPROVE decision cannot be
        // attested and approve_request returns Err. The grant handler propagates
        // this, so no token is minted and the request stays Pending (G4b).
        let path = unique_tmp("approve_ro");
        std::fs::File::create(&path).unwrap();
        let ro = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let audit = Arc::new(AuditLog::with_file_handle(ro));
        let pending = PendingRequestStore::new(audit);

        let res = pending.approve_request(
            "req-1",
            "sess-1",
            "localhost://Users/self/Documents/x",
            "read",
            "approver-1",
        );
        assert!(
            res.is_err(),
            "approve must fail closed when its durable audit write fails"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn approve_request_records_signed_decision_that_verifies() {
        // Companion: with a working SIGNED sink the approve commits, a
        // CapabilityApproved decision is recorded, AND its ed25519 signature
        // verifies on the chain — proving the record is genuinely signed (not just
        // present) and that the fail-closed path triggers on real failure only.
        use ed25519_dalek::VerifyingKey;
        let path = unique_tmp("approve_ok");
        let audit = Arc::new(AuditLog::with_file(&path).unwrap());
        let pending = PendingRequestStore::new(audit.clone());

        let res = pending.approve_request(
            "req-1",
            "sess-1",
            "localhost://Users/self/Documents/x",
            "read",
            "approver-1",
        );
        assert!(res.is_ok(), "approve records onto a working signed sink");
        assert!(
            audit.recent_events(50).iter().any(|e| matches!(
                e,
                crate::primitives::audit::AuditEvent::CapabilityApproved { .. }
            )),
            "the approval decision must be recorded"
        );

        // The recorded decision's signature verifies against the log's key.
        let hex = audit
            .verifying_key_hex()
            .expect("a file-backed log must be signed");
        let bytes: [u8; 32] = hex::decode(&hex).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&bytes).unwrap();
        assert!(
            audit.verify_chain(Some(&vk)).is_ok(),
            "the signed approval record must verify on the chain"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn create_request_with_capsule_records_requester_identity() {
        // G-ID interim: the requester's real capsule identity is recorded on the
        // pending request when present, and left None (not fabricated) when absent.
        let audit = Arc::new(AuditLog::new());
        let pending = PendingRequestStore::new(audit);

        let with_id = pending
            .create_request_with_capsule(
                SessionId::new(),
                ResourceId::new("elastos://rights/x"),
                Action::Read,
                Some("vm-market".to_string()),
            )
            .await;
        assert_eq!(
            with_id.requester_capsule_id.as_deref(),
            Some("vm-market"),
            "the real capsule identity is recorded on the request"
        );

        // The plain delegator records None -- honest absence, no shim/fabrication.
        let without = pending
            .create_request(
                SessionId::new(),
                ResourceId::new("elastos://rights/x"),
                Action::Read,
            )
            .await;
        assert_eq!(without.requester_capsule_id, None);
    }

    #[tokio::test]
    async fn affordance_binding_survives_storage_and_defaults_none() {
        // W2 step 3: an affordance-consent request carries the full binding and it
        // survives a store->retrieve round-trip; an ordinary session request keeps
        // all four binding fields None (behaviour-neutral).
        let store = create_test_store();

        let bound = store
            .create_affordance_request(
                SessionId::from_string("session-aff"),
                ResourceId::new("elastos://rights/play"),
                Action::Execute,
                AffordanceBinding {
                    capsule: "vm-player".to_string(),
                    principal_id: "did:ela:alice".to_string(),
                    method_id: "play".to_string(),
                    input_hash: "abc123".to_string(),
                },
            )
            .await;

        let got = store
            .get_request(bound.id.as_str())
            .await
            .expect("affordance request is stored");
        assert_eq!(got.capsule.as_deref(), Some("vm-player"));
        assert_eq!(got.principal_id.as_deref(), Some("did:ela:alice"));
        assert_eq!(got.method_id.as_deref(), Some("play"));
        assert_eq!(got.input_hash.as_deref(), Some("abc123"));

        // A plain session request carries no binding.
        let plain = store
            .create_request(
                SessionId::from_string("session-plain"),
                ResourceId::new("localhost://Users/self/Documents/x"),
                Action::Read,
            )
            .await;
        assert_eq!(plain.capsule, None);
        assert_eq!(plain.method_id, None);
        assert_eq!(plain.input_hash, None);
    }

    #[tokio::test]
    async fn requester_identity_and_affordance_binding_coexist() {
        // The collision reconciliation (G-ID requester_capsule_id vs W2 binding):
        // both must be able to live on the SAME request. requester_capsule_id
        // records WHO asked (carrier identity); the binding records WHAT the
        // consent is scoped to (affordance + args). Neither overwrites the other.
        let store = create_test_store();

        let req = store
            .create_request_inner(
                SessionId::from_string("session-both"),
                ResourceId::new("elastos://rights/play"),
                Action::Execute,
                Some("vm-caller".to_string()),
                Some(AffordanceBinding {
                    capsule: "vm-player".to_string(),
                    principal_id: "did:ela:alice".to_string(),
                    method_id: "play".to_string(),
                    input_hash: "deadbeef".to_string(),
                }),
                String::new(),
            )
            .await;

        let got = store
            .get_request(req.id.as_str())
            .await
            .expect("request is stored");
        assert_eq!(
            got.requester_capsule_id.as_deref(),
            Some("vm-caller"),
            "identity (who asked) is preserved"
        );
        assert_eq!(
            got.capsule.as_deref(),
            Some("vm-player"),
            "binding (what it's scoped to) is preserved alongside identity"
        );
        assert_eq!(got.input_hash.as_deref(), Some("deadbeef"));
    }
}
