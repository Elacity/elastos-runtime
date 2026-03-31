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

use elastos_common::localhost::is_supported_resource_scheme;
use elastos_runtime::capability::{
    pending::PendingRequestStore, Action, CapabilityManager, GrantDuration, PolicyEvaluator,
    PolicyOutcome, ResourceId, TokenConstraints,
};
use elastos_runtime::session::Session;

/// Shared state for capability handlers
#[derive(Clone)]
pub struct CapabilityState {
    pub pending_store: Arc<PendingRequestStore>,
    pub capability_manager: Arc<CapabilityManager>,
    pub policy_evaluator: Arc<PolicyEvaluator>,
}

// === Request Capability ===

#[derive(Debug, Deserialize)]
pub struct RequestCapabilityInput {
    /// Resource to request access to (e.g., "localhost://Users/self/Pictures/*")
    pub resource: String,
    /// Action to request (e.g., "read", "write")
    pub action: String,
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

/// Return true if the resource URI uses a supported scheme.
///
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

    let resource = ResourceId::new(&input.resource);

    // For now, all requests go to pending (no auto-grant policy yet)
    // Future: check if session already has this capability, or if policy allows auto-grant

    let request = state
        .pending_store
        .create_request(session.id.clone(), resource, action)
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
            requested_at: r.requested_at.unix_secs,
            expires_at: r.expires_at.unix_secs,
        })
        .collect();

    Json(ListPendingOutput { requests })
}

// === Grant Request (Shell Only) ===

#[derive(Debug, Deserialize)]
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

/// POST /api/capability/grant
///
/// Grant a pending capability request (shell only).
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

    // Policy evaluation (observational)
    let rationale = input.rationale.as_deref().unwrap_or("Shell auto-grant");
    let _decision = state
        .policy_evaluator
        .evaluate(&request, PolicyOutcome::Grant, rationale);

    // Create the capability token
    let constraints = match duration {
        GrantDuration::Once => TokenConstraints::new(0, false, None, Some(1)),
        GrantDuration::Session => TokenConstraints::default(),
    };

    // Use session ID as capsule ID for now
    let token = state.capability_manager.grant(
        &request.session_id.to_string(),
        request.resource.clone(),
        request.action,
        constraints,
        None, // No expiry for now (session-scoped)
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

    #[test]
    fn test_supported_resource_schemes() {
        assert!(is_supported_resource_scheme("elastos://did/*"));
        assert!(is_supported_resource_scheme("elastos://ai/local/chat"));
        assert!(is_supported_resource_scheme(
            "localhost://Users/self/Documents/*"
        ));
    }

    #[test]
    fn test_rejects_unsupported_resource_schemes() {
        assert!(!is_supported_resource_scheme("elastos:/broken"));
        assert!(!is_supported_resource_scheme("localhost:/broken"));
        assert!(!is_supported_resource_scheme("resource-without-scheme"));
        assert!(!is_supported_resource_scheme(""));
    }
}
