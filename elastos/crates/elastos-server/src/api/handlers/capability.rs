//! Capability request handlers
//!
//! Handles capability request/grant/deny flow.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use elastos_common::localhost::{is_supported_resource_scheme, is_system_only_backend_resource};
use elastos_runtime::capability::{
    pending::{PendingCapabilityRequest, PendingRequestStore},
    Action, CapabilityManager, GrantDuration, PolicyDecision, PolicyEvaluator, PolicyOutcome,
    ResourceId, TokenConstraints,
};
use elastos_runtime::primitives::time::SecureTimestamp;
use elastos_runtime::session::Session;

use crate::provider_resource::ensure_generic_wallet_capability;

/// Shared state for capability handlers
#[derive(Clone)]
pub struct CapabilityState {
    pub pending_store: Arc<PendingRequestStore>,
    pub capability_manager: Arc<CapabilityManager>,
    pub policy_evaluator: Arc<PolicyEvaluator>,
}

// === Request Capability ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestCapabilityInput {
    /// Resource to request access to (e.g., "localhost://MyWebSite/Pictures/*")
    pub resource: String,
    /// Action to request (e.g., "read", "write")
    pub action: String,
    /// Plain-language reason shown during approval.
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct RequestCapabilityOutput {
    /// Status: "pending", "granted", or "auto_denied"
    pub status: String,
    /// Request ID (if pending)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Capability token (if auto-granted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Reason (if auto-denied)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// POST /api/capability/request
///
/// Request a capability token. Returns immediately with either:
/// - status: "pending" + request_id (needs user approval)
/// - status: "granted" + token (auto-granted)
/// - status: "auto_denied" + reason (policy rejection)
pub async fn request_capability(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>,
    Json(input): Json<RequestCapabilityInput>,
) -> Result<Json<RequestCapabilityOutput>, (StatusCode, String)> {
    // Parse action
    let action = match input.action.to_lowercase().as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" => Action::Execute,
        "delete" => Action::Delete,
        "message" => Action::Message,
        "admin" => Action::Admin,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid action: {}. Expected: read, write, execute, delete, message, admin",
                    input.action
                ),
            ));
        }
    };

    if !is_supported_resource_scheme(&input.resource) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Unsupported resource scheme. Allowed: elastos://, localhost://".to_string(),
        ));
    }

    if is_system_only_backend_resource(&input.resource) {
        return Ok(Json(RequestCapabilityOutput {
            status: "denied".to_string(),
            request_id: None,
            token: None,
            reason: Some(
                "system backends are not app capabilities; use elastos://content".to_string(),
            ),
        }));
    }

    if let Err(reason) = ensure_generic_wallet_capability(&input.resource, action) {
        return Ok(Json(RequestCapabilityOutput {
            status: "denied".to_string(),
            request_id: None,
            token: None,
            reason: Some(reason),
        }));
    }

    let resource = ResourceId::new(&input.resource);

    // For now, all requests go to pending (no auto-grant policy yet)
    // Future: check if session already has this capability, or if policy allows auto-grant

    let request = state
        .pending_store
        .create_request_with_reason(session.id.clone(), resource, action, input.reason)
        .await;

    // If pre-denied (e.g. rate limit), surface the denial immediately
    if request.is_denied() {
        return Ok(Json(RequestCapabilityOutput {
            status: "denied".to_string(),
            request_id: Some(request.id.to_string()),
            token: None,
            reason: Some("Too many pending requests".to_string()),
        }));
    }

    Ok(Json(RequestCapabilityOutput {
        status: "pending".to_string(),
        request_id: Some(request.id.to_string()),
        token: None,
        reason: None,
    }))
}

// === Request Status ===

#[derive(Debug, Serialize)]
pub struct RequestStatusOutput {
    /// Status: "pending", "granted", "denied", or "expired"
    pub status: String,
    /// Capability token (if granted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Reason (if denied)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// GET /api/capability/request/:id
///
/// Check the status of a capability request.
pub async fn request_status(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>,
    Path(request_id): Path<String>,
) -> Result<Json<RequestStatusOutput>, (StatusCode, String)> {
    let request = state
        .pending_store
        .get_request(&request_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Request not found: {}", request_id),
            )
        })?;

    // Verify the request belongs to this session
    if request.session_id != session.id {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot access another session's request".to_string(),
        ));
    }

    // Check for expiry
    if request.is_expired() {
        return Ok(Json(RequestStatusOutput {
            status: "expired".to_string(),
            token: None,
            reason: Some("Request timed out".to_string()),
        }));
    }

    match &request.status {
        elastos_runtime::capability::RequestStatus::Pending => Ok(Json(RequestStatusOutput {
            status: "pending".to_string(),
            token: None,
            reason: None,
        })),
        elastos_runtime::capability::RequestStatus::Granted { token, .. } => {
            Ok(Json(RequestStatusOutput {
                status: "granted".to_string(),
                token: Some(token.to_base64().unwrap_or_default()),
                reason: None,
            }))
        }
        elastos_runtime::capability::RequestStatus::Denied { reason } => {
            Ok(Json(RequestStatusOutput {
                status: "denied".to_string(),
                token: None,
                reason: Some(reason.clone()),
            }))
        }
        elastos_runtime::capability::RequestStatus::Expired => Ok(Json(RequestStatusOutput {
            status: "expired".to_string(),
            token: None,
            reason: Some("Request timed out".to_string()),
        })),
    }
}

// === List Pending (Shell Only) ===

#[derive(Debug, Serialize)]
pub struct PendingRequestOutput {
    pub request_id: String,
    pub session_id: String,
    pub resource: String,
    pub action: String,
    pub reason: String,
    pub requested_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
pub struct ListPendingOutput {
    pub requests: Vec<PendingRequestOutput>,
}

/// GET /api/capability/pending
///
/// List all pending capability requests (shell only).
pub async fn list_pending(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
) -> Json<ListPendingOutput> {
    let pending = state.pending_store.list_pending().await;

    let requests = pending
        .into_iter()
        .map(|r| PendingRequestOutput {
            request_id: r.id.to_string(),
            session_id: r.session_id.to_string(),
            resource: r.resource.to_string(),
            action: r.action.to_string(),
            reason: r.reason,
            requested_at: r.requested_at.unix_secs,
            expires_at: r.expires_at.unix_secs,
        })
        .collect();

    Json(ListPendingOutput { requests })
}

// === Grant Request (Shell Only) ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRequestInput {
    /// Request ID to grant
    pub request_id: String,
    /// Duration: "once" or "session"
    #[serde(default = "default_duration")]
    pub duration: String,
    /// Shell's rationale for granting (passed to PolicyEvaluator audit)
    #[serde(default)]
    pub rationale: Option<String>,
}

fn default_duration() -> String {
    "session".to_string()
}

#[derive(Debug, Serialize)]
pub struct GrantRequestOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn policy_refusal_reason(decision: &PolicyDecision) -> Option<String> {
    if decision.outcome == PolicyOutcome::Grant {
        return None;
    }
    let reason = decision.rationale.trim();
    Some(if reason.is_empty() {
        "policy refused the capability grant".to_string()
    } else {
        reason.to_string()
    })
}

async fn refuse_pending_grant(
    state: &CapabilityState,
    request_id: &str,
    reason: String,
) -> (StatusCode, String) {
    if let Err(err) = state.pending_store.deny_request(request_id, &reason).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    (StatusCode::FORBIDDEN, reason)
}

/// Token material for one granted request.
///
/// The shell duration still selects the base token. Policy fields only
/// tighten that token. An absent limit leaves the previous field unchanged,
/// so a grant with default constraints matches the previous shell grant.
fn grant_from_policy(
    request: &PendingCapabilityRequest,
    duration: GrantDuration,
    decision: &PolicyDecision,
) -> Result<(ResourceId, TokenConstraints, Option<SecureTimestamp>), String> {
    let mut resource = request.resource.clone();
    let limits = &decision.effective_constraints;
    if let Some(scope) = limits.resource_scope.as_deref() {
        let narrowed = ResourceId::new(scope);
        if !narrowed.matches(&request.resource) {
            return Err("policy resource scope is outside the requested resource".to_string());
        }
        resource = narrowed;
    }

    let shell_max_uses = match duration {
        GrantDuration::Once => Some(1),
        GrantDuration::Session => None,
    };
    let max_uses = match (shell_max_uses, limits.max_uses) {
        (Some(existing), Some(limit)) => Some(existing.min(limit)),
        (Some(existing), None) => Some(existing),
        (None, Some(limit)) => Some(limit),
        (None, None) => None,
    };
    // Delegation stays off. A policy grant cannot turn it on.
    // Classification has no shell ceiling, so the policy ceiling is the limit.
    let constraints = TokenConstraints::new(0, false, limits.max_classification, max_uses);

    let mut expiry = None;
    if let Some(ttl) = limits.ttl_secs {
        let now = SecureTimestamp::now().unix_secs;
        let unix = now
            .checked_add(ttl)
            .ok_or_else(|| "policy ttl overflows the expiry clock".to_string())?;
        expiry = Some(SecureTimestamp::at(unix));
    }
    if let Some(effective) = decision.effective_expiry {
        expiry = Some(match expiry {
            Some(from_ttl) if from_ttl.unix_secs <= effective.unix_secs => from_ttl,
            Some(_) => effective,
            None => effective,
        });
    }

    Ok((resource, constraints, expiry))
}

/// POST /api/capability/grant
///
/// Grant a pending capability request (shell only).
///
/// The primary policy decision gates the grant. Deny and defer refuse it.
/// A grant keeps the shell duration and applies any tighter limits from the
/// decision. The shell's previous token shape is unchanged when the decision
/// carries no tighter limit.
pub async fn grant_request(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
    Json(input): Json<GrantRequestInput>,
) -> Result<Json<GrantRequestOutput>, (StatusCode, String)> {
    // Parse duration
    let duration: GrantDuration = input
        .duration
        .parse()
        .map_err(|e: String| (StatusCode::BAD_REQUEST, e))?;

    // Get the pending request
    let request = state
        .pending_store
        .get_request(&input.request_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Request not found: {}", input.request_id),
            )
        })?;

    if !request.is_pending() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Request {} is not pending", input.request_id),
        ));
    }

    // The value returned here is the primary decision. Shadow decisions stay
    // inside the evaluator and are not part of this result.
    let rationale = input.rationale.as_deref().unwrap_or("Shell auto-grant");
    let decision = state
        .policy_evaluator
        .evaluate(&request, PolicyOutcome::Grant, rationale);
    if let Some(reason) = policy_refusal_reason(&decision) {
        return Err(refuse_pending_grant(&state, &input.request_id, reason).await);
    }

    let (resource, constraints, expiry) = match grant_from_policy(&request, duration, &decision) {
        Ok(material) => material,
        Err(reason) => {
            return Err(refuse_pending_grant(&state, &input.request_id, reason).await);
        }
    };

    // Use session ID as capsule ID for now
    let token = state.capability_manager.grant(
        &request.session_id.to_string(),
        resource,
        request.action,
        constraints,
        expiry,
    );

    // Mark request as granted
    state
        .pending_store
        .grant_request(&input.request_id, token.clone(), duration)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(GrantRequestOutput {
        success: true,
        token: Some(token.to_base64().unwrap_or_default()),
        error: None,
    }))
}

// === Deny Request (Shell Only) ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenyRequestInput {
    /// Request ID to deny
    pub request_id: String,
    /// Reason for denial (optional)
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DenyRequestOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// POST /api/capability/deny
///
/// Deny a pending capability request (shell only).
pub async fn deny_request(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
    Json(input): Json<DenyRequestInput>,
) -> Result<Json<DenyRequestOutput>, (StatusCode, String)> {
    let reason = input.reason.unwrap_or_else(|| "Denied by user".to_string());

    // Policy evaluation (observational)
    if let Some(request) = state.pending_store.get_request(&input.request_id).await {
        let _decision = state
            .policy_evaluator
            .evaluate(&request, PolicyOutcome::Deny, &reason);
    }

    state
        .pending_store
        .deny_request(&input.request_id, &reason)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    Ok(Json(DenyRequestOutput {
        success: true,
        error: None,
    }))
}

// === List Capabilities ===

#[derive(Debug, Serialize)]
pub struct CapabilityOutput {
    pub request_id: String,
    pub resource: String,
    pub action: String,
    pub duration: String,
    pub granted_at: u64,
}

#[derive(Debug, Serialize)]
pub struct ListCapabilitiesOutput {
    pub capabilities: Vec<CapabilityOutput>,
}

/// GET /api/capability/list
///
/// List active capabilities for the current session.
pub async fn list_capabilities(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>,
) -> Json<ListCapabilitiesOutput> {
    // Get granted requests for this session
    let session_id = session.id.to_string();
    let granted_requests = state.pending_store.list_session_granted(&session_id).await;

    let capabilities = granted_requests
        .into_iter()
        .filter_map(|r| {
            if let elastos_runtime::capability::RequestStatus::Granted { duration, .. } = &r.status
            {
                Some(CapabilityOutput {
                    request_id: r.id.to_string(),
                    resource: r.resource.to_string(),
                    action: r.action.to_string(),
                    duration: duration.to_string(),
                    granted_at: r.requested_at.unix_secs, // Use requested_at as proxy for granted_at
                })
            } else {
                None
            }
        })
        .collect();

    Json(ListCapabilitiesOutput { capabilities })
}

// === Revoke Capability (Shell Only) ===

#[derive(Debug, Serialize)]
pub struct RevokeCapabilityOutput {
    pub success: bool,
    pub revoked_request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// DELETE /api/capability/:id
///
/// Revoke a specific capability by request ID (shell only).
pub async fn revoke_capability(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
    Path(request_id): Path<String>,
) -> Result<Json<RevokeCapabilityOutput>, (StatusCode, String)> {
    // Get the request to find the token
    let request = state
        .pending_store
        .get_request(&request_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Capability request not found: {}", request_id),
            )
        })?;

    // Check if it was granted
    match &request.status {
        elastos_runtime::capability::RequestStatus::Granted { token, .. } => {
            // Revoke the token
            state
                .capability_manager
                .revoke(*token.id(), "Revoked by user via API")
                .await;

            // Mark the request as revoked in the pending store
            state.pending_store.revoke_request(&request_id).await;

            Ok(Json(RevokeCapabilityOutput {
                success: true,
                revoked_request_id: request_id,
                error: None,
            }))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            format!("Capability {} was not granted", request_id),
        )),
    }
}

// === Revoke All Capabilities (Shell Only) ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeAllInput {
    /// Reason for revoking all capabilities
    #[serde(default = "default_revoke_reason")]
    pub reason: String,
}

fn default_revoke_reason() -> String {
    "Revoked all by user".to_string()
}

#[derive(Debug, Serialize)]
pub struct RevokeAllOutput {
    pub success: bool,
    pub new_epoch: u64,
    pub reason: String,
}

/// POST /api/capability/revoke-all
///
/// Revoke all capabilities by advancing the epoch (shell only).
/// All tokens with epoch < new_epoch will be rejected.
pub async fn revoke_all_capabilities(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
    Json(input): Json<RevokeAllInput>,
) -> Json<RevokeAllOutput> {
    let new_epoch = state.capability_manager.revoke_all(&input.reason);

    // Mark all granted requests as revoked
    state.pending_store.revoke_all_granted().await;

    Json(RevokeAllOutput {
        success: true,
        new_epoch,
        reason: input.reason,
    })
}

// === Session Info ===

#[derive(Debug, Serialize)]
pub struct SessionInfoOutput {
    pub session_id: String,
    pub session_type: String,
    pub vm_id: Option<String>,
    pub capabilities_count: usize,
    pub created_at: u64,
    pub last_active: u64,
}

/// GET /api/session
///
/// Get information about the current session.
pub async fn session_info(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>,
) -> Json<SessionInfoOutput> {
    let session_id = session.id.to_string();
    let capabilities_count = state
        .pending_store
        .list_session_granted(&session_id)
        .await
        .len();

    Json(SessionInfoOutput {
        session_id,
        session_type: session.session_type.to_string(),
        vm_id: session.vm_id.clone(),
        capabilities_count,
        created_at: session.created_at.unix_secs,
        last_active: session.last_active.unix_secs,
    })
}

// === Audit Log API (Shell Only) ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditLogQuery {
    /// Maximum number of events to return (default: 100, max: 1000)
    #[serde(default = "default_audit_limit")]
    pub limit: usize,
    /// Filter by event type (e.g., "capability_grant", "capability_revoke")
    #[serde(rename = "type")]
    pub event_type: Option<String>,
}

fn default_audit_limit() -> usize {
    100
}

#[derive(Debug, Serialize)]
pub struct AuditLogOutput {
    /// List of audit events (newest first)
    pub events: Vec<elastos_runtime::primitives::audit::AuditEvent>,
    /// Total events in memory buffer
    pub total_in_memory: usize,
    /// Current epoch
    pub current_epoch: u64,
}

/// GET /api/audit
///
/// Get recent audit log events (shell only).
/// Query params:
/// - limit: Max events to return (default 100, max 1000)
/// - type: Filter by event type (e.g., "capability_grant")
pub async fn get_audit_log(
    State(state): State<CapabilityState>,
    Extension(_session): Extension<Session>, // Shell check done by middleware
    Query(query): Query<AuditLogQuery>,
) -> Json<AuditLogOutput> {
    let limit = query.limit.min(1000);
    let audit_log = state.capability_manager.audit_log();

    let events = if let Some(ref event_type) = query.event_type {
        audit_log.recent_events_filtered(limit, Some(event_type))
    } else {
        audit_log.recent_events(limit)
    };

    Json(AuditLogOutput {
        events,
        total_in_memory: audit_log.event_count(),
        current_epoch: state.capability_manager.current_epoch(),
    })
}

/// Available audit event types for filtering
#[derive(Debug, Serialize)]
pub struct AuditEventTypesOutput {
    pub event_types: Vec<&'static str>,
}

/// GET /api/audit/types
///
/// List available audit event types for filtering.
pub async fn get_audit_event_types(
    Extension(_session): Extension<Session>,
) -> Json<AuditEventTypesOutput> {
    Json(AuditEventTypesOutput {
        event_types: vec![
            "runtime_start",
            "runtime_stop",
            "capsule_launch",
            "capsule_stop",
            "capability_grant",
            "capability_revoke",
            "capability_use",
            "capability_requested",
            "capability_denied",
            "content_fetch",
            "auth_attempt",
            "epoch_advance",
            "config_change",
            "security_warning",
            "session_created",
            "session_destroyed",
            "policy_proposal",
            "policy_decision_made",
            "policy_divergence",
            "custom",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_supported_resource_schemes() {
        assert!(is_supported_resource_scheme("elastos://did/*"));
        assert!(is_supported_resource_scheme(
            "localhost://MyWebSite/Documents/*"
        ));
    }

    #[test]
    fn test_rejects_unsupported_resource_schemes() {
        assert!(!is_supported_resource_scheme("elastos:/broken"));
        assert!(!is_supported_resource_scheme("localhost:/broken"));
        assert!(!is_supported_resource_scheme("resource-without-scheme"));
        assert!(!is_supported_resource_scheme(""));
    }

    #[test]
    fn test_system_only_backend_resource_detection() {
        assert!(is_system_only_backend_resource("elastos://ipfs/add"));
        assert!(is_system_only_backend_resource("elastos://ipfs"));
        assert!(is_system_only_backend_resource("elastos://kubo/rpc"));
        assert!(is_system_only_backend_resource(
            "elastos://ipfs-cluster/pins"
        ));
        assert!(is_system_only_backend_resource("elastos://elacity-sdk/pin"));
        assert!(is_system_only_backend_resource(
            "elastos://ipfs-provider/add"
        ));
        assert!(is_system_only_backend_resource("elastos://gateway/raw"));
        assert!(!is_system_only_backend_resource(
            "elastos://content/publish"
        ));
        assert!(!is_system_only_backend_resource(
            "localhost://MyWebSite/Documents/x"
        ));
    }

    fn assert_rejects_unknown_field<T: serde::de::DeserializeOwned>(value: serde_json::Value) {
        let err = match serde_json::from_value::<T>(value) {
            Ok(_) => panic!("expected capability body to reject unknown fields"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn test_capability_inputs_reject_hidden_authority_fields() {
        assert_rejects_unknown_field::<RequestCapabilityInput>(json!({
            "resource": "elastos://content/publish",
            "action": "write",
            "capability_token": "must-not-be-accepted"
        }));
        assert_rejects_unknown_field::<GrantRequestInput>(json!({
            "request_id": "request:test",
            "duration": "session",
            "rationale": "ok",
            "token": "must-not-be-accepted"
        }));
        assert_rejects_unknown_field::<DenyRequestInput>(json!({
            "request_id": "request:test",
            "reason": "no",
            "override": true
        }));
        assert_rejects_unknown_field::<RevokeAllInput>(json!({
            "reason": "rotate",
            "session_id": "session:other"
        }));
        assert_rejects_unknown_field::<AuditLogQuery>(json!({
            "limit": 10,
            "type": "capability_grant",
            "include_private": true
        }));
    }

    fn test_state_with_verifier(
        verifier: Box<dyn elastos_runtime::capability::PolicyVerifier>,
    ) -> CapabilityState {
        let audit_log = std::sync::Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));

        CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(verifier, audit_log)),
        }
    }

    fn test_state() -> CapabilityState {
        test_state_with_verifier(Box::new(
            elastos_runtime::capability::evaluator::ShellPassthroughVerifier,
        ))
    }

    struct FixedDecisionVerifier {
        outcome: PolicyOutcome,
        constraints: elastos_runtime::capability::ProposedConstraints,
        expiry: Option<elastos_runtime::primitives::time::SecureTimestamp>,
        rationale: &'static str,
    }

    impl elastos_runtime::capability::PolicyVerifier for FixedDecisionVerifier {
        fn verify(
            &self,
            request: &elastos_runtime::capability::pending::PendingCapabilityRequest,
            _proposal: &elastos_runtime::capability::GrantProposal,
            shadow: bool,
        ) -> elastos_runtime::capability::PolicyDecision {
            elastos_runtime::capability::PolicyDecision {
                id: elastos_runtime::capability::DecisionId::new(),
                request_id: request.id.to_string(),
                resource: request.resource.to_string(),
                action: request.action.to_string(),
                outcome: self.outcome,
                checks: Vec::new(),
                effective_constraints: self.constraints.clone(),
                effective_expiry: self.expiry,
                rationale: self.rationale.to_string(),
                decided_at: elastos_runtime::primitives::time::SecureTimestamp::now(),
                shadow,
            }
        }
    }

    async fn pending_documents_request(state: &CapabilityState) -> (Session, String) {
        let session = Session::new_capsule("capsule-policy".to_string());
        let output = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "localhost://MyWebSite/Documents/*".to_string(),
                action: "read".to_string(),
                reason: "read documents".to_string(),
            }),
        )
        .await
        .expect("document capability request should be accepted")
        .0;
        assert_eq!(output.status, "pending");
        (session, output.request_id.expect("pending request id"))
    }

    fn assert_no_capability_grant(state: &CapabilityState) {
        let events = state.capability_manager.audit_log().recent_events(20);
        assert!(
            events
                .iter()
                .all(|event| event.event_type_name() != "capability_grant"),
            "a refused grant must not issue a capability token"
        );
    }

    #[tokio::test]
    async fn test_request_capability_denies_ipfs_backend() {
        let output = request_capability(
            State(test_state()),
            Extension(Session::new_capsule("capsule-1".to_string())),
            Json(RequestCapabilityInput {
                resource: "elastos://ipfs/add".to_string(),
                action: "write".to_string(),
                reason: "publish content".to_string(),
            }),
        )
        .await
        .expect("ipfs backend request should return a structured denial")
        .0;

        assert_eq!(output.status, "denied");
        assert_eq!(output.request_id, None);
        assert!(output.reason.unwrap().contains("elastos://content"));
    }

    #[tokio::test]
    async fn test_request_capability_allows_content_contract() {
        let output = request_capability(
            State(test_state()),
            Extension(Session::new_capsule("capsule-1".to_string())),
            Json(RequestCapabilityInput {
                resource: "elastos://content/publish".to_string(),
                action: "write".to_string(),
                reason: "publish content".to_string(),
            }),
        )
        .await
        .expect("content contract request should be accepted")
        .0;

        assert_eq!(output.status, "pending");
        assert!(output.request_id.is_some());
        assert!(output.reason.is_none());
    }

    #[tokio::test]
    async fn generic_wallet_capability_requests_fail_before_pending() {
        let state = test_state();
        for (resource, action) in [
            ("elastos://wallet/*", "read"),
            ("elastos://wallet/account/list", "read"),
            ("elastos://wallet/proof/challenge", "read"),
            ("elastos://wallet/approval", "write"),
            ("elastos://wallet/meta/status", "write"),
        ] {
            let output = request_capability(
                State(state.clone()),
                Extension(Session::new_capsule("capsule-1".to_string())),
                Json(RequestCapabilityInput {
                    resource: resource.to_string(),
                    action: action.to_string(),
                    reason: "request generic Wallet authority".to_string(),
                }),
            )
            .await
            .expect("generic Wallet capability request should return a structured denial")
            .0;

            assert_eq!(output.status, "denied", "{resource} {action}");
            assert_eq!(output.request_id, None, "{resource} {action}");
            assert!(
                output
                    .reason
                    .unwrap()
                    .contains("private Runtime Wallet Bus"),
                "{resource} {action}"
            );
        }

        assert!(
            state.pending_store.list_pending().await.is_empty(),
            "rejected Wallet capability requests must not create pending approvals"
        );
    }

    #[tokio::test]
    async fn read_only_wallet_status_capability_remains_requestable() {
        let state = test_state();
        let output = request_capability(
            State(state.clone()),
            Extension(Session::new_capsule("capsule-1".to_string())),
            Json(RequestCapabilityInput {
                resource: crate::provider_resource::WALLET_STATUS_RESOURCE.to_string(),
                action: "read".to_string(),
                reason: "read Wallet provider status".to_string(),
            }),
        )
        .await
        .expect("read-only Wallet status capability should remain requestable")
        .0;

        assert_eq!(output.status, "pending");
        assert!(output.request_id.is_some());
        let pending = state.pending_store.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].resource.to_string(),
            crate::provider_resource::WALLET_STATUS_RESOURCE
        );
        assert_eq!(pending[0].action, Action::Read);
    }

    async fn grant_documents(
        state: &CapabilityState,
        request_id: &str,
        duration: &str,
    ) -> Result<GrantRequestOutput, (StatusCode, String)> {
        grant_request(
            State(state.clone()),
            Extension(Session::new_shell("shell".to_string())),
            Json(GrantRequestInput {
                request_id: request_id.to_string(),
                duration: duration.to_string(),
                rationale: Some("owner approved".to_string()),
            }),
        )
        .await
        .map(|response| response.0)
    }

    #[tokio::test]
    async fn policy_denied_grant_is_refused() {
        for outcome in [PolicyOutcome::Deny, PolicyOutcome::Defer] {
            let state = test_state_with_verifier(Box::new(FixedDecisionVerifier {
                outcome,
                constraints: elastos_runtime::capability::ProposedConstraints::default(),
                expiry: None,
                rationale: "blocked by policy",
            }));
            let (session, request_id) = pending_documents_request(&state).await;

            let session_id = session.id.to_string();
            let err = grant_documents(&state, &request_id, "session")
                .await
                .expect_err("policy denial must refuse the grant");

            assert_eq!(err.0, StatusCode::FORBIDDEN, "{outcome}");
            assert!(err.1.contains("blocked by policy"), "{outcome}");
            assert_no_capability_grant(&state);

            let status = request_status(
                State(state.clone()),
                Extension(session),
                Path(request_id.clone()),
            )
            .await
            .expect("denied request remains readable")
            .0;
            assert_eq!(status.status, "denied", "{outcome}");
            assert_eq!(status.token, None, "{outcome}");
            assert_eq!(
                status.reason.as_deref(),
                Some("blocked by policy"),
                "{outcome}"
            );
            assert!(
                state
                    .pending_store
                    .list_session_granted(&session_id)
                    .await
                    .is_empty(),
                "{outcome}"
            );
        }
    }

    #[tokio::test]
    async fn policy_allowed_grant_matches_previous_token() {
        let state = test_state();
        let (session, request_id) = pending_documents_request(&state).await;

        let output = grant_documents(&state, &request_id, "session")
            .await
            .expect("passthrough policy grant should succeed");

        assert!(output.success);
        assert!(output.error.is_none());
        let token = elastos_runtime::capability::CapabilityToken::from_base64(
            output.token.as_deref().expect("granted token"),
        )
        .expect("granted token decodes");
        assert_eq!(token.capsule(), session.id.as_str());
        assert_eq!(
            token.resource().as_str(),
            "localhost://MyWebSite/Documents/*"
        );
        assert_eq!(token.action(), Action::Read);
        assert!(token.expiry().is_none());
        assert_eq!(token.constraints().epoch(), 0);
        assert!(!token.constraints().delegatable());
        assert_eq!(token.constraints().max_classification(), None);
        assert_eq!(token.constraints().max_uses(), None);

        let status = request_status(
            State(state.clone()),
            Extension(session.clone()),
            Path(request_id),
        )
        .await
        .expect("granted request remains readable")
        .0;
        assert_eq!(status.status, "granted");
        assert!(status.token.is_some());

        let listed = list_capabilities(State(state.clone()), Extension(session)).await;
        assert_eq!(listed.capabilities.len(), 1);
        assert_eq!(
            listed.capabilities[0].resource,
            "localhost://MyWebSite/Documents/*"
        );
        assert_eq!(listed.capabilities[0].action, "read");
        assert_eq!(listed.capabilities[0].duration, "session");

        let once_state = test_state();
        let (_once_session, once_id) = pending_documents_request(&once_state).await;
        let once = grant_documents(&once_state, &once_id, "once")
            .await
            .expect("passthrough once grant should succeed");
        let once_token = elastos_runtime::capability::CapabilityToken::from_base64(
            once.token.as_deref().expect("once token"),
        )
        .expect("once token decodes");
        assert!(once_token.expiry().is_none());
        assert_eq!(once_token.constraints().max_uses(), Some(1));
        assert!(!once_token.constraints().delegatable());
        assert_eq!(
            once_token.resource().as_str(),
            "localhost://MyWebSite/Documents/*"
        );
    }

    #[tokio::test]
    async fn policy_limit_is_applied_and_cannot_widen_the_grant() {
        let before = elastos_runtime::primitives::time::SecureTimestamp::now().unix_secs;
        let state = test_state_with_verifier(Box::new(FixedDecisionVerifier {
            outcome: PolicyOutcome::Grant,
            constraints: elastos_runtime::capability::ProposedConstraints {
                resource_scope: Some("localhost://MyWebSite/Documents/photos/*".to_string()),
                ttl_secs: Some(120),
                max_uses: Some(4),
                delegatable: true,
                max_classification: Some(3),
            },
            expiry: None,
            rationale: "narrow the grant",
        }));
        let (_session, request_id) = pending_documents_request(&state).await;
        let output = grant_documents(&state, &request_id, "session")
            .await
            .expect("a tighter policy grant should succeed");
        let after = elastos_runtime::primitives::time::SecureTimestamp::now().unix_secs;
        let token = elastos_runtime::capability::CapabilityToken::from_base64(
            output.token.as_deref().expect("limited token"),
        )
        .expect("limited token decodes");

        assert_eq!(
            token.resource().as_str(),
            "localhost://MyWebSite/Documents/photos/*"
        );
        assert_eq!(token.constraints().max_uses(), Some(4));
        assert_eq!(token.constraints().max_classification(), Some(3));
        assert!(!token.constraints().delegatable());
        let expiry = token
            .expiry()
            .expect("ttl becomes the token expiry")
            .unix_secs;
        assert!(expiry >= before + 120, "{expiry} {before}");
        assert!(expiry <= after + 120, "{expiry} {after}");

        let once_state = test_state_with_verifier(Box::new(FixedDecisionVerifier {
            outcome: PolicyOutcome::Grant,
            constraints: elastos_runtime::capability::ProposedConstraints {
                resource_scope: None,
                ttl_secs: None,
                max_uses: Some(4),
                delegatable: true,
                max_classification: None,
            },
            expiry: None,
            rationale: "use limit cannot loosen a one-use grant",
        }));
        let (_once_session, once_id) = pending_documents_request(&once_state).await;
        let once = grant_documents(&once_state, &once_id, "once")
            .await
            .expect("once grant with a looser use limit should stay one-use");
        let once_token = elastos_runtime::capability::CapabilityToken::from_base64(
            once.token.as_deref().expect("once token"),
        )
        .expect("once token decodes");
        assert_eq!(once_token.constraints().max_uses(), Some(1));
        assert!(!once_token.constraints().delegatable());
        assert!(once_token.expiry().is_none());

        let widened = test_state_with_verifier(Box::new(FixedDecisionVerifier {
            outcome: PolicyOutcome::Grant,
            constraints: elastos_runtime::capability::ProposedConstraints {
                resource_scope: Some("localhost://MyWebSite/*".to_string()),
                ttl_secs: None,
                max_uses: None,
                delegatable: false,
                max_classification: None,
            },
            expiry: None,
            rationale: "scope is wider than the request",
        }));
        let (_wide_session, wide_id) = pending_documents_request(&widened).await;
        let err = grant_documents(&widened, &wide_id, "session")
            .await
            .expect_err("a wider policy scope must refuse the grant");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(err.1.contains("outside the requested resource"));
        assert_no_capability_grant(&widened);
    }
}
