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

use elastos_common::localhost::{
    is_overbroad_grant_resource, is_supported_resource_scheme, is_system_only_backend_resource,
};
use elastos_runtime::approval::{self, ApprovalDecision};
use elastos_runtime::capability::manager::ValidationError;
use elastos_runtime::capability::{
    pending::{AffordanceBinding, PendingRequestStore},
    token::TokenId,
    Action, AffordanceGrantReceiptV1, CapabilityManager, CapabilityToken, EnvelopeCheck,
    GrantDuration, IntentDeclarationV1, PolicyEvaluator, PolicyOutcome, ResourceId,
    StandingGrantService, TokenConstraints,
};
use elastos_runtime::session::Session;

/// Shared state for capability handlers
#[derive(Clone)]
pub struct CapabilityState {
    pub pending_store: Arc<PendingRequestStore>,
    pub capability_manager: Arc<CapabilityManager>,
    pub policy_evaluator: Arc<PolicyEvaluator>,
    /// The standing-grant service (issue/revoke/dispatch for unsupervised agent acts), backed by the
    /// manager's own key + audit log. Shared so every shell-only verb hits the same grant registry.
    pub standing_service: Arc<StandingGrantService>,
}

// === Request Capability ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestCapabilityInput {
    /// Resource to request access to (e.g., "localhost://MyWebSite/Pictures/*")
    pub resource: String,
    /// Action to request (e.g., "read", "write")
    pub action: String,
    /// Affordance-consent binding (W2). All four are present together for an
    /// affordance-consent request and absent together for an ordinary session
    /// capability request; a partial set is rejected fail-closed.
    #[serde(default)]
    pub capsule: Option<String>,
    #[serde(default)]
    pub principal_id: Option<String>,
    #[serde(default)]
    pub method_id: Option<String>,
    #[serde(default)]
    pub input_hash: Option<String>,
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
    // AUD-5: refuse a BARE scheme-level wildcard (elastos://* etc.) — it would
    // prefix-match every resource under the scheme. Legit grants are scheme-scoped.
    if is_overbroad_grant_resource(&input.resource) {
        return Ok(Json(RequestCapabilityOutput {
            status: "denied".to_string(),
            request_id: None,
            token: None,
            reason: Some(
                "scheme-level wildcard grants are not permitted; scope to at least one \
                 path segment, e.g. elastos://<provider>/*"
                    .to_string(),
            ),
        }));
    }

    let resource = ResourceId::new(&input.resource);

    // For now, all requests go to pending (no auto-grant policy yet)
    // Future: check if session already has this capability, or if policy allows auto-grant

    // Affordance-consent requests (W2) carry all four binding fields together and
    // route through create_affordance_request so the eventual grant binds to the
    // exact (capsule, principal, method, args). Ordinary session requests carry
    // none and keep flint's G-ID behaviour (mint at the real capsule identity,
    // session.vm_id). A partial binding is rejected fail-closed, never silently
    // dropped.
    let request = match (
        input.capsule.as_ref(),
        input.principal_id.as_ref(),
        input.method_id.as_ref(),
        input.input_hash.as_ref(),
    ) {
        (None, None, None, None) => {
            state
                .pending_store
                .create_request_with_capsule(
                    session.id.clone(),
                    resource,
                    action,
                    session.vm_id.clone(),
                )
                .await
        }
        (Some(capsule), Some(principal_id), Some(method_id), Some(input_hash)) => {
            state
                .pending_store
                .create_affordance_request(
                    session.id.clone(),
                    resource,
                    action,
                    AffordanceBinding {
                        capsule: capsule.clone(),
                        principal_id: principal_id.clone(),
                        method_id: method_id.clone(),
                        input_hash: input_hash.clone(),
                    },
                )
                .await
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "affordance-consent binding requires all of capsule, principal_id, method_id, input_hash"
                    .to_string(),
            ));
        }
    };

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

// === Validate And Consume (W2 step 7) ===

/// Parse an action string into an [`Action`], or a 400 listing the allowed set.
fn parse_action(raw: &str) -> Result<Action, (StatusCode, String)> {
    match raw.to_lowercase().as_str() {
        "read" => Ok(Action::Read),
        "write" => Ok(Action::Write),
        "execute" => Ok(Action::Execute),
        "delete" => Ok(Action::Delete),
        "message" => Ok(Action::Message),
        "admin" => Ok(Action::Admin),
        _ => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Invalid action: {raw}. Expected: read, write, execute, delete, message, admin"
            ),
        )),
    }
}

/// Map a [`ValidationError`] to a fail-closed response with a DISTINCT, safe code
/// per failure. We never forward the error's `Display` (it embeds expected/got
/// resources + capsule ids), which would leak internal topology to the caller.
fn map_validation_error(err: &ValidationError) -> (StatusCode, String) {
    // Most failures are caller-authorization problems (403). A failed durable
    // signed-audit write (W2 step 9) is a SERVER fault, surfaced fail-closed as
    // 503 — the act was refused because the redemption could not be recorded.
    let (status, code) = match err {
        ValidationError::InvalidVersion { .. } => {
            (StatusCode::FORBIDDEN, "token_version_unsupported")
        }
        ValidationError::InvalidSignature => (StatusCode::FORBIDDEN, "token_signature_invalid"),
        ValidationError::UntrustedIssuer => (StatusCode::FORBIDDEN, "token_issuer_untrusted"),
        ValidationError::WrongCapsule { .. } => (StatusCode::FORBIDDEN, "token_wrong_caller"),
        ValidationError::WrongAction { .. } => (StatusCode::FORBIDDEN, "token_wrong_action"),
        ValidationError::WrongResource { .. } => (StatusCode::FORBIDDEN, "token_wrong_resource"),
        ValidationError::TokenRevoked => (StatusCode::FORBIDDEN, "token_revoked"),
        ValidationError::TokenExpired => (StatusCode::FORBIDDEN, "token_expired"),
        ValidationError::FutureDatedToken => (StatusCode::FORBIDDEN, "token_not_yet_valid"),
        ValidationError::UseLimitExceeded { .. } => (StatusCode::FORBIDDEN, "token_already_used"),
        ValidationError::ClassificationExceeded { .. } => {
            (StatusCode::FORBIDDEN, "token_classification_insufficient")
        }
        ValidationError::ClassificationUnavailable { .. } => {
            (StatusCode::FORBIDDEN, "token_classification_unavailable")
        }
        ValidationError::DelegationNotAllowed => {
            (StatusCode::FORBIDDEN, "token_delegation_not_allowed")
        }
        ValidationError::DelegationScopeWidened => {
            (StatusCode::FORBIDDEN, "token_delegation_scope_widened")
        }
        ValidationError::AuditWriteFailed => {
            (StatusCode::SERVICE_UNAVAILABLE, "audit_write_failed")
        }
    };
    (status, code.to_string())
}

/// Body for `POST /api/capability/validate-and-consume`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateAndConsumeInput {
    /// The granted affordance-consent token (base64).
    pub token: String,
    /// The affordance method being invoked; must equal the token's bound method.
    pub method_id: String,
    /// The resource the invocation targets; must match the token's resource.
    pub resource: String,
    /// The action being performed; must match the token's action.
    pub action: String,
    /// The invocation arguments; re-hashed and compared to the token's binding.
    #[serde(default)]
    pub input: serde_json::Value,
}

/// Result of a successful redemption.
#[derive(Debug, Serialize)]
pub struct ValidateAndConsumeOutput {
    /// Always "consumed" on success (the single use has been atomically spent).
    pub status: String,
    /// Signed attestation of the redemption (W2 step 9) — the portable proof the
    /// caller keeps. Verifiable under the runtime's capability public key.
    pub receipt: AffordanceGrantReceiptV1,
}

/// POST /api/capability/validate-and-consume
///
/// Redeem an affordance-consent token for a SINGLE invocation (W2 step 7). The
/// runtime — the sole key holder — re-validates the token AND re-checks that the
/// invocation matches the EXACT `(method, arguments)` the user approved, then
/// atomically consumes the single use (emitting a signed `CapabilityUse`). This
/// is the only validator for affordance-consent tokens; the edge cannot bypass
/// it. Every failure is fail-closed with a distinct code and no internal prose.
pub async fn validate_and_consume(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>,
    Json(input): Json<ValidateAndConsumeInput>,
) -> Result<Json<ValidateAndConsumeOutput>, (StatusCode, String)> {
    // The caller identity is the AUTHENTICATED session's capsule identity
    // ("vm-{name}"), never a value from the request body (Principle 16: a surface
    // is not authority). Fail closed when the session has no capsule identity.
    let caller = session.vm_id.as_deref().ok_or((
        StatusCode::FORBIDDEN,
        "redeeming session has no capsule identity".to_string(),
    ))?;

    // Decode the token; a malformed token is refused, never trusted.
    let token = CapabilityToken::from_base64(&input.token).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "malformed capability token".to_string(),
        )
    })?;

    // This endpoint redeems ONLY affordance-consent tokens — they carry a binding.
    // A token without one must go through the ordinary capability gate, not here.
    let (bound_method, bound_hash) = match (
        token.constraints().method_id(),
        token.constraints().input_hash(),
    ) {
        (Some(method), Some(hash)) => (method, hash),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "token is not an affordance-consent token".to_string(),
            ));
        }
    };

    // Re-check the binding BEFORE consuming, so a mismatched redemption fails
    // closed WITHOUT burning the single use. Distinct codes for method vs args.
    if bound_method != input.method_id {
        return Err((StatusCode::FORBIDDEN, "consent_method_mismatch".to_string()));
    }
    let recomputed = elastos_common::canonical_input_hash(&input.input);
    if bound_hash != recomputed {
        return Err((StatusCode::FORBIDDEN, "consent_args_mismatch".to_string()));
    }

    // Full validation (signature, issuer, caller, action, resource, revocation,
    // expiry) AND the atomic single-use consume + signed CapabilityUse all happen
    // inside validate() — the one canonical validator. It consumes only after
    // every check passes, so the failures above never spend the use either.
    let resource = ResourceId::new(&input.resource);
    let action = parse_action(&input.action)?;
    state
        .capability_manager
        .validate(&token, caller, action, &resource, None)
        .await
        .map_err(|err| map_validation_error(&err))?;

    // The use is now consumed AND a signed durable record was written (blocking,
    // inside validate()). Mint the portable signed receipt the caller keeps —
    // "if there's no receipt, there's no act" (W2 step 9).
    let receipt = state.capability_manager.issue_affordance_receipt(
        &token.id().to_string(),
        caller,
        &input.method_id,
        &recomputed,
        &input.resource,
        action,
    );

    Ok(Json(ValidateAndConsumeOutput {
        status: "consumed".to_string(),
        receipt,
    }))
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

/// POST /api/capability/grant
///
/// Grant a pending capability request (shell only).
pub async fn grant_request(
    State(state): State<CapabilityState>,
    Extension(session): Extension<Session>, // Shell check done by middleware
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

    // AUD-5 defense-in-depth: refuse to MINT a bare scheme-level wildcard even if one
    // reached a pending request (request_capability already guards intake; this covers
    // any other request path). The token would prefix-match every resource under the scheme.
    if is_overbroad_grant_resource(&request.resource.to_string()) {
        return Err((
            StatusCode::FORBIDDEN,
            "scheme-level wildcard grants are not permitted; scope to at least one path segment"
                .to_string(),
        ));
    }

    // Policy evaluation (observational audit only; the PolicyEvaluator is a
    // non-authoritative recorder — see KNOWN_GAPS G4/G4b).
    let rationale = input.rationale.as_deref().unwrap_or("Shell auto-grant");
    let _decision = state
        .policy_evaluator
        .evaluate(&request, PolicyOutcome::Grant, rationale);

    // Canonical consent gate (G4a, Principle 10): the live grant is authorized by
    // the SAME fail-closed core the preview uses (approval::decide). The
    // authenticated consent-broker POST (it passed consent_broker_only_middleware and references an
    // existing pending request) is the explicit approver, so today this resolves
    // to Approved; routing it THROUGH decide means any future approver source that
    // yields None / Some(false) fails closed and mints nothing, instead of the old
    // unconditional mint after a discarded policy run.
    let mode = approval::required_approval(&[request.action]);
    if approval::decide(&mode, Some(true)) != ApprovalDecision::Approved {
        return Err((
            StatusCode::FORBIDDEN,
            format!("capability grant not approved: {}", input.request_id),
        ));
    }

    // G4b: attest the APPROVE decision onto the signed audit chain, fail-closed and
    // BEFORE the mint — the exact mirror of the deny side. An approval that cannot
    // be durably + signed recorded grants NOTHING: the token below is never minted,
    // the request stays Pending, and nothing reaches the client. Records the
    // DECISION (who approved which request), distinct from grant()'s best-effort
    // token-issuance breadcrumb.
    let approver = session
        .owner
        .clone()
        .unwrap_or_else(|| session.id.to_string());
    state
        .pending_store
        .approve_request(
            &input.request_id,
            &request.session_id.to_string(),
            &request.resource.to_string(),
            &request.action.to_string(),
            &approver,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Create the capability token (reached only on an Approved AND attested
    // decision).
    //
    // W2 step 6 (the enforcement crux): an affordance-consent request carries a
    // binding, so its grant READS that binding and is scoped to the EXACT
    // affordance + arguments — minted at the BOUND capsule, single-use, with a
    // short expiry, and with (method_id, input_hash) sealed into the signed
    // token. This is deliberately distinct from the ordinary session/capability
    // grant, which keeps flint's G-ID identity path unchanged.
    let (token, grant_duration) = match (
        request.capsule.as_deref(),
        request.method_id.as_deref(),
        request.input_hash.as_deref(),
    ) {
        (Some(capsule), Some(method_id), Some(input_hash)) => {
            // 1 hour: long enough for a human to act on the consent prompt, short
            // enough that an approved-but-unused grant lapses rather than lingers.
            const AFFORDANCE_GRANT_TTL_SECS: u64 = 3600;
            let constraints = TokenConstraints::for_affordance(method_id, input_hash);
            let expiry = Some(elastos_common::SecureTimestamp::after_secs(
                AFFORDANCE_GRANT_TTL_SECS,
            ));
            let token = state.capability_manager.grant(
                capsule,
                request.resource.clone(),
                request.action,
                constraints,
                expiry,
            );
            (token, GrantDuration::Once)
        }
        _ => {
            let constraints = match duration {
                GrantDuration::Once => TokenConstraints::new(0, false, None, Some(1)),
                GrantDuration::Session => TokenConstraints::default(),
            };

            // G-ID flip: mint at the requester's REAL capsule identity ("vm-{name}",
            // recorded on the request), not the session-id shim. Fail closed
            // FORBIDDEN if the requester had no capsule identity -- mint NOTHING
            // rather than fabricate one (mirror of the approval guard above). The
            // request stays Pending.
            let requester_capsule_id = match request.requester_capsule_id.as_deref() {
                Some(id) => id,
                None => {
                    return Err((
                        StatusCode::FORBIDDEN,
                        format!(
                            "capability grant has no requester capsule identity: {}",
                            input.request_id
                        ),
                    ));
                }
            };
            let token = state.capability_manager.grant(
                requester_capsule_id,
                request.resource.clone(),
                request.action,
                constraints,
                None, // No expiry for now (session-scoped)
            );
            (token, duration)
        }
    };

    // Mark request as granted
    state
        .pending_store
        .grant_request(&input.request_id, token.clone(), grant_duration)
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
            // Revoke the token, FAIL-CLOSED on durable custody (G8b): the manager
            // emits the signed revoke record BEFORE killing the token, so on an
            // audit-write failure the token stays valid and we surface it rather
            // than reporting a revoke with no durable record.
            state
                .capability_manager
                .revoke(*token.id(), "Revoked by user via API")
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("revoke could not be durably attested: {e}"),
                    )
                })?;

            // Mark the request as revoked in the pending store, fail-closed on the
            // audit record (AUD-3): the token is already revoked above, but if the
            // revocation cannot be durably+signed attested we surface it rather than
            // returning success with a lost record.
            state
                .pending_store
                .revoke_request(&request_id)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("revocation could not be attested: {e}"),
                    )
                })?;

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
) -> Result<Json<RevokeAllOutput>, (StatusCode, String)> {
    let new_epoch = state.capability_manager.revoke_all(&input.reason);

    // The envelope side of the mass kill: the epoch advance killed every backing TOKEN, but the
    // standing-grant registry knows nothing of epochs — without this, an epoch-dead mandate keeps
    // rendering (and, once dispatch is wired, dispatching) as LIVE.
    let _ = state.standing_service.revoke_all();

    // Mark all granted requests as revoked, fail-closed on the audit records (AUD-3):
    // the epoch increment above already invalidated the tokens, but an incomplete
    // attestation is surfaced rather than silently lost.
    state
        .pending_store
        .revoke_all_granted()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("epoch advanced but revocation attestation failed: {e}"),
            )
        })?;

    Ok(Json(RevokeAllOutput {
        success: true,
        new_epoch,
        reason: input.reason,
    }))
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

// === Standing grants (shell-only): the unsupervised-agent authority verbs ===
//
// A standing grant lets an agent act repeatedly under the intent-proof loop without a
// per-act human prompt. Issuing/revoking is AUTHORITY, so these live behind the shell-only
// router (consent_broker_only_middleware) exactly like grant/deny — an ordinary capsule
// session can never reach them. `issue` mints a REAL signed capability token (the
// cryptographic root) and derives the standing envelope from it; `revoke` is the kill switch.

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueStandingGrantInput {
    /// The acting capsule identity the grant authorizes (e.g. "vm-agent").
    pub capsule: String,
    /// The resource the grant covers (e.g. "elastos://mail/send").
    pub resource: String,
    /// The action (read | write | execute | delete | message | admin).
    pub action: String,
    /// The affordance method ids the agent may invoke under this grant (non-empty).
    pub methods: Vec<String>,
    /// Optional time-to-live in seconds; omitted ⇒ no expiry (until revoked).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct IssueStandingGrantOutput {
    /// The standing grant id (the backing token's id) — revoke or dispatch against this.
    pub grant_id: String,
    /// The backing capability token's id — identical to `grant_id`, surfaced explicitly because it
    /// is the key for the mandate's audit trail (`export_mandate_receipt_for_capability`).
    pub token_id: String,
}

/// POST /api/standing-grants/issue  (shell-only)
///
/// Issue a standing grant for unsupervised agent dispatch. Mints a real signed capability token
/// for (capsule, resource, action, ttl) — the cryptographic root — then derives and stores the
/// standing envelope with the authorized method set. Fail-closed on an unknown action or empty
/// method set.
pub async fn issue_standing_grant(
    State(state): State<CapabilityState>,
    Json(input): Json<IssueStandingGrantInput>,
) -> Result<Json<IssueStandingGrantOutput>, (StatusCode, String)> {
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
    if input.methods.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "a standing grant must authorize at least one method".to_string(),
        ));
    }
    let expiry = input
        .ttl_secs
        .map(elastos_common::SecureTimestamp::after_secs);
    // Mint a real signed token (the cryptographic root), then elevate it to a standing grant.
    let token = state.capability_manager.grant(
        &input.capsule,
        ResourceId::new(input.resource),
        action,
        TokenConstraints::default(),
        expiry,
    );
    let methods: std::collections::BTreeSet<String> = input.methods.into_iter().collect();
    let grant_id = state.standing_service.issue_from_token(&token, methods);
    let token_id = token.id().to_string();
    Ok(Json(IssueStandingGrantOutput { grant_id, token_id }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeStandingGrantInput {
    /// The grant id returned by issue.
    pub grant_id: String,
}

#[derive(Debug, Serialize)]
pub struct RevokeStandingGrantOutput {
    /// True iff a live grant was revoked by this call (already-revoked / unknown ⇒ false).
    pub revoked: bool,
}

/// POST /api/standing-grants/revoke  (shell-only) — the autonomy kill switch.
///
/// Revoke a standing grant by id, fail-closed AND durably attested. The grant id IS the backing
/// capability token's id, and killing only the in-memory envelope would leave the mandate's audit
/// trail showing it live forever — so this first revokes the BACKING TOKEN through the manager
/// (which emits the signed `CapabilityRevoke` record BEFORE mutating, per AUD-3: if the durable
/// write fails, the revoke ABORTS and the error surfaces rather than a revoke existing with no
/// record), then kills the standing envelope so the dispatcher denies every not-yet-started act.
/// The mandate's receipt (`export_mandate_receipt_for_capability`) thereafter carries the revoke.
pub async fn revoke_standing_grant(
    State(state): State<CapabilityState>,
    Json(input): Json<RevokeStandingGrantInput>,
) -> Result<Json<RevokeStandingGrantOutput>, (StatusCode, String)> {
    let token_id = TokenId::from_hex(input.grant_id.trim()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("grant_id is not a valid token id: {e}"),
        )
    })?;
    // Durable custody first (emit-before-mutate): a revoke that cannot be signed onto the audit
    // chain does not happen — mirroring revoke_capability. The envelope stays live so the failure
    // is loud and re-runnable, never a silent half-revoke the receipt can't prove.
    state
        .capability_manager
        .revoke(token_id, "standing grant revoked via API")
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("revoke could not be durably attested: {e}"),
            )
        })?;
    // Kill the envelope by the CANONICAL id (the parsed token's lowercase-hex Display form — the
    // exact key the registry stores). Keying off the caller's raw string would let an UPPERCASE
    // spelling revoke the token yet miss the envelope, leaving the dispatch path live.
    let revoked = state.standing_service.revoke(&token_id.to_string());
    Ok(Json(RevokeStandingGrantOutput { revoked }))
}

#[derive(Debug, Serialize)]
pub struct PreviewStandingGrantOutput {
    /// "allowed" | "denied" — whether the declared intent WOULD pass its standing grant.
    pub verdict: String,
    /// The fail-closed denial reason (snake_case) when denied; `null` when allowed.
    pub reason: Option<String>,
}

/// POST /api/standing-grants/preview  (shell-only)
///
/// DRY-RUN the intent gate for a SIGNED [`IntentDeclarationV1`]: authenticate the declaration
/// (its signature must verify against the key it names), then report whether it falls within its
/// standing grant. This is the READ-ONLY half of dispatch — it records NOTHING and runs NO act, so
/// it is side-effect-free (safe for dashboards / debugging an agent's authority). Fail-closed: a
/// forged or malformed declaration is rejected (400) before any grant is consulted; a missing grant
/// is a `denied` verdict with reason `no_standing_grant`.
pub async fn preview_standing_grant(
    State(state): State<CapabilityState>,
    Json(intent): Json<IntentDeclarationV1>,
) -> Result<Json<PreviewStandingGrantOutput>, (StatusCode, String)> {
    // Authenticate first: the gateway did not construct this declaration, so it must prove the
    // intent was signed by the key it claims before speaking to whether it is authorized.
    let out = match state.standing_service.authenticated_preview(&intent) {
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                "intent declaration signature did not verify".to_string(),
            ));
        }
        Some(EnvelopeCheck::Allowed) => PreviewStandingGrantOutput {
            verdict: "allowed".to_string(),
            reason: None,
        },
        Some(EnvelopeCheck::Denied(reason)) => PreviewStandingGrantOutput {
            verdict: "denied".to_string(),
            reason: Some(reason.as_str().to_string()),
        },
    };
    Ok(Json(out))
}

/// One mandate as the operator surface renders it — the "mandate card" data shape.
#[derive(Debug, Serialize)]
pub struct MandateCard {
    /// The mandate's token id (keys the receipt + revoke).
    pub token_id: String,
    /// The acting capsule identity the mandate authorizes.
    pub capsule: String,
    /// Resource scope.
    pub resource: String,
    /// Action scope.
    pub action: String,
    /// Affordance methods the agent may invoke.
    pub methods: Vec<String>,
    /// Expiry (None = until revoked).
    pub expires_at: Option<elastos_common::SecureTimestamp>,
    /// Explicitly revoked?
    pub revoked: bool,
    /// Live right now (issued, not revoked, not expired)?
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct ListMandatesOutput {
    /// Every standing mandate issued this runtime lifetime, revoked ones included (an operator
    /// surface shows what was killed, it does not erase it), sorted by token id.
    pub mandates: Vec<MandateCard>,
    /// The runtime audit chain's signer key (hex) — the pin the operator passes to
    /// `elastos verify-receipt --signer`. `None` for an unsigned/memory-only log. Honest bounds:
    /// this key rides the loopback control plane, whose trust root is the operator's 0600
    /// coords/attach-secret files — the attach exchange authenticates the CLIENT to the runtime;
    /// runtime identity rests on that filesystem + loopback assumption, not on a cryptographic
    /// server credential. It breaks receipt-SELF-pinning; it is not third-party attestation.
    pub signer_public_key_hex: Option<String>,
}

/// GET /api/standing-grants  (shell-only) — the operator's mandate list.
pub async fn list_standing_grants(
    State(state): State<CapabilityState>,
) -> Json<ListMandatesOutput> {
    let mut mandates = Vec::new();
    for env in state.standing_service.list() {
        // The card's LIVE bit must consult BOTH kill paths: the envelope flag (standing revoke,
        // mass revoke_all) AND the manager's individual token-revocation store — a mandate whose
        // backing token was killed by any other route must never render live. An unparseable id
        // is fail-closed inactive.
        let token_dead = match TokenId::from_hex(&env.grant_id) {
            Ok(id) => state.capability_manager.is_token_revoked(&id).await,
            Err(_) => true,
        };
        mandates.push(MandateCard {
            active: env.is_active() && !token_dead,
            token_id: env.grant_id,
            capsule: env.capsule,
            resource: env.resource,
            action: env.action,
            methods: env.allowed_methods.into_iter().collect(),
            expires_at: env.expires_at,
            revoked: env.revoked || token_dead,
        });
    }
    Json(ListMandatesOutput {
        mandates,
        signer_public_key_hex: state.capability_manager.audit_log().verifying_key_hex(),
    })
}

#[derive(Debug, Serialize)]
pub struct DispatchIntentOutput {
    /// "acted" | "denied".
    pub outcome: String,
    /// The fail-closed denial reason (snake_case) when denied; `None` when acted.
    pub reason: Option<String>,
    /// The signed declared-vs-done reconciliation when acted; `None` when denied.
    pub reconciliation: Option<elastos_runtime::capability::IntentReconciliationV1>,
}

/// POST /api/standing-grants/dispatch  (shell-only) — the ACT leg of the mandate loop.
///
/// Run ONE agent act under its standing mandate, fail-closed: authenticate the signed
/// [`IntentDeclarationV1`] (a forged declaration is rejected before any grant lookup), close
/// G-M1 (the envelope gate alone knows nothing of the manager's token-revocation store — if the
/// backing token is individually dead, the envelope is marked revoked FIRST so the gate denies
/// with the honest `Revoked` reason), then run the intent gate (declaration recorded on-chain
/// BEFORE the act; no custody ⇒ no act). The act mints the signed affordance receipt — the same
/// proof-of-act primitive the consent flow uses. Closes G-M2: the outcome is ALSO recorded as a
/// token-keyed `CapabilityUse` (success mirrors the outcome), so the mandate's exported receipt
/// carries every intent-channel act, not just validate-path redemptions.
pub async fn dispatch_standing_intent(
    State(state): State<CapabilityState>,
    Json(intent): Json<IntentDeclarationV1>,
) -> Result<Json<DispatchIntentOutput>, (StatusCode, String)> {
    if !intent.verify_self() {
        return Err((
            StatusCode::BAD_REQUEST,
            "intent declaration signature did not verify".to_string(),
        ));
    }
    let action = match intent.action.to_lowercase().as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" => Action::Execute,
        "delete" => Action::Delete,
        "message" => Action::Message,
        "admin" => Action::Admin,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("invalid action in intent: {other}"),
            ));
        }
    };
    // G-M1: consult the manager's individual token-revocation store, which the envelope gate
    // cannot see. Self-heal the envelope so the denial carries the true `revoked` reason (and
    // sticks for future dispatches). Unparseable grant ids fail closed at the gate (NoGrant).
    if let Ok(token_id) = TokenId::from_hex(intent.standing_grant_id.trim()) {
        if state.capability_manager.is_token_revoked(&token_id).await {
            let _ = state.standing_service.revoke(&token_id.to_string());
        }
    }
    let manager = state.capability_manager.clone();
    let (i_token, i_capsule, i_method, i_hash, i_resource) = (
        intent.standing_grant_id.clone(),
        intent.capsule.clone(),
        intent.method_id.clone(),
        intent.input_hash.clone(),
        intent.resource.clone(),
    );
    let outcome = state.standing_service.dispatch(&intent, move || {
        Some(manager.issue_affordance_receipt(
            &i_token, &i_capsule, &i_method, &i_hash, &i_resource, action,
        ))
    });
    use elastos_runtime::capability::IntentGateOutcome;
    let (out, acted) = match outcome {
        IntentGateOutcome::BlockedNoCustody(e) => {
            // Custody is mandatory: the declaration could not land on the chain, so nothing ran
            // and nothing further is recordable — surface it, emit nothing else.
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("intent custody write failed; act not run: {e}"),
            ));
        }
        IntentGateOutcome::Denied(reason) => (
            DispatchIntentOutput {
                outcome: "denied".to_string(),
                reason: Some(reason.as_str().to_string()),
                reconciliation: None,
            },
            false,
        ),
        IntentGateOutcome::Acted(rec) => (
            DispatchIntentOutput {
                outcome: "acted".to_string(),
                reason: None,
                reconciliation: Some(rec),
            },
            true,
        ),
    };
    // G-M2: token-keyed projection of the outcome, so export_mandate_receipt_for_capability
    // carries the intent-channel act (or its denial) in the mandate's receipt.
    if let Ok(token_id) = TokenId::from_hex(intent.standing_grant_id.trim()) {
        state.capability_manager.audit_log().capability_use(
            &token_id,
            &intent.capsule,
            &ResourceId::new(intent.resource.clone()),
            action,
            acted,
        );
    }
    Ok(Json(out))
}

/// GET /api/mandate/:token_id/receipt  (shell-only)
///
/// Export the PORTABLE per-mandate receipt for one capability token: the signed, set-bound bundle
/// of its grant + every use/revoke, straight from the runtime's durable audit chain — the artifact
/// an operator hands an auditor, verified off-box with `elastos verify-receipt`. Read-only over
/// the chain; mints nothing, mutates nothing. `404` when the token has no durable records (unknown
/// id, or a memory-only/unsigned log — absence is reported, never fabricated).
pub async fn mandate_receipt(
    State(state): State<CapabilityState>,
    Path(token_id): Path<String>, // Shell check done by middleware
) -> Result<Json<elastos_runtime::primitives::MandateReceipt>, (StatusCode, String)> {
    // Canonicalize: only a well-formed token id can key a mandate (and its Display form is the
    // exact string the audit records carry).
    let token_id = TokenId::from_hex(token_id.trim())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid token id: {e}")))?
        .to_string();
    let receipt = state
        .capability_manager
        .audit_log()
        .export_mandate_receipt_for_capability(&token_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no durable audit records for mandate {token_id}"),
            )
        })?;
    Ok(Json(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_supported_resource_schemes() {
        assert!(is_supported_resource_scheme("elastos://did/*"));
        assert!(is_supported_resource_scheme("elastos://ai/local/chat"));
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

    fn test_state() -> CapabilityState {
        let audit_log = std::sync::Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        let standing_service = std::sync::Arc::new(capability_manager.standing_grant_service());

        CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log,
            )),
            standing_service,
        }
    }

    #[tokio::test]
    async fn issue_then_revoke_standing_grant_over_the_handlers() {
        let state = test_state();
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
            }),
        )
        .await
        .expect("issue ok")
        .0;
        assert!(!out.grant_id.is_empty(), "issue returns a grant id");
        assert!(
            state.standing_service.is_active(&out.grant_id),
            "the issued grant is active in the shared service"
        );

        // Revoke → true; the grant goes inactive; a second revoke → false (idempotent kill switch).
        let rev = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput {
                grant_id: out.grant_id.clone(),
            }),
        )
        .await
        .expect("revoke ok")
        .0;
        assert!(rev.revoked, "revoking a live grant returns true");
        assert!(!state.standing_service.is_active(&out.grant_id));
        let rev2 = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput {
                grant_id: out.grant_id.clone(),
            }),
        )
        .await
        .expect("ok")
        .0;
        assert!(!rev2.revoked, "double-revoke returns false");
    }

    /// Like [`test_state`] but with a DURABLE (file-backed, signed) audit log, so the mandate's
    /// grant/use/revoke land on a real chain a receipt can be exported from.
    fn test_state_with_durable_audit(dir: &std::path::Path) -> CapabilityState {
        let audit_log = std::sync::Arc::new(
            elastos_runtime::primitives::audit::AuditLog::with_file(dir.join("audit.log"))
                .expect("file-backed audit log"),
        );
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        let standing_service = std::sync::Arc::new(capability_manager.standing_grant_service());
        CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log,
            )),
            standing_service,
        }
    }

    /// The whole Flint loop over the real handlers: GRANT a mandate, the agent ACTS under it,
    /// REVOKE it (durably attested — the ratchet this sprint adds), then export the RECEIPT and
    /// verify it off-box: it must carry the grant + the use + the revoke and authenticate against
    /// the runtime's pinned signer. A receipt that showed a revoked mandate as live would be the
    /// exact dishonesty this loop exists to prevent.
    #[tokio::test]
    async fn mandate_lifecycle_grant_use_revoke_exports_a_verifiable_receipt() {
        use elastos_runtime::primitives::audit::AuditEvent;
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());

        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["pay.invoke".to_string()],
                ttl_secs: Some(3600),
            }),
        )
        .await
        .expect("issue ok")
        .0;
        assert_eq!(out.token_id, out.grant_id, "the grant id IS the mandate's token id");

        // The agent acts under the mandate. This injects the CapabilityUse the way
        // CapabilityManager::validate emits it on every token redemption (manager.rs, check #8
        // path) — the production act path that exists today. NOTE the intent-channel dispatcher
        // (StandingGrantService::dispatch) is not yet wired to any endpoint and emits
        // intent-keyed (not token-keyed) records; when it lands, its acts must ALSO reach this
        // receipt or the loop under-reports — tracked in docs/KNOWN_GAPS.md.
        let token_id = TokenId::from_hex(&out.token_id).expect("token id round-trips");
        state.capability_manager.audit_log().capability_use(
            &token_id,
            "vm-agent",
            &ResourceId::new("elastos://pay/vendor"),
            Action::Write,
            true,
        );

        let rev = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput {
                grant_id: out.grant_id.clone(),
            }),
        )
        .await
        .expect("revoke ok")
        .0;
        assert!(rev.revoked);

        let receipt = mandate_receipt(State(state.clone()), Path(out.token_id.clone()))
            .await
            .expect("receipt exists")
            .0;
        assert_eq!(receipt.records.len(), 3, "grant + use + revoke");
        assert!(
            receipt
                .records
                .iter()
                .any(|r| matches!(r.event, AuditEvent::CapabilityRevoke { .. })),
            "the REVOKE is durably attested in the mandate's receipt"
        );
        let signer = state
            .capability_manager
            .audit_log()
            .verifying_key_hex()
            .expect("signed log");
        let verdict = elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer));
        assert!(verdict.authenticated, "receipt authenticates when pinned: {verdict:?}");
        assert!(verdict.set_binding_ok && verdict.scope_ok);

        // An unknown mandate is ABSENT (404), never an empty "clean" receipt.
        let missing = mandate_receipt(State(state.clone()), Path(TokenId::new().to_string())).await;
        assert!(matches!(missing, Err((StatusCode::NOT_FOUND, _))));
        // A malformed token id is a 400, before any chain read.
        let bad = mandate_receipt(State(state), Path("not-hex".to_string())).await;
        assert!(matches!(bad, Err((StatusCode::BAD_REQUEST, _))));
    }

    /// Sign an intent under a fresh agent key naming the mandate. `verify_self` proves internal
    /// authenticity (signed by the key it names), which is what the dispatch handler requires.
    fn signed_intent(grant_id: &str, method: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "intent-1",
            "vm-agent",
            method,
            "cafe01",
            "elastos://pay/vendor",
            "write",
            grant_id,
        )
    }

    /// The ACT leg closes G-M2: a dispatched act lands as a token-keyed CapabilityUse in the
    /// mandate's exported receipt — grant + act + revoke, all in one verifiable artifact.
    #[tokio::test]
    async fn dispatch_acts_under_the_mandate_and_the_receipt_carries_the_act() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["pay.invoke".to_string()],
                ttl_secs: Some(3600),
            }),
        )
        .await
        .unwrap()
        .0;

        let resp = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "pay.invoke")),
        )
        .await
        .expect("dispatch ok")
        .0;
        assert_eq!(resp.outcome, "acted");
        assert!(resp.reconciliation.is_some(), "signed declared-vs-done reconciliation returned");

        // A method OUTSIDE the envelope is denied fail-closed — and the denial is receipted too.
        let denied = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "pay.refund")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(denied.outcome, "denied");

        let receipt = mandate_receipt(State(state.clone()), Path(out.token_id.clone()))
            .await
            .unwrap()
            .0;
        use elastos_runtime::primitives::audit::AuditEvent;
        let uses: Vec<bool> = receipt
            .records
            .iter()
            .filter_map(|r| match &r.event {
                AuditEvent::CapabilityUse { success, .. } => Some(*success),
                _ => None,
            })
            .collect();
        assert_eq!(uses, vec![true, false], "the act AND the denied attempt are both receipted");
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        let verdict = elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer));
        assert!(verdict.authenticated, "receipt with acts still authenticates: {verdict:?}");
    }

    /// G-M1 regression: a backing token killed DIRECTLY through the manager (a path the envelope
    /// registry cannot see) must deny dispatch — the handler consults the revocation store and
    /// self-heals the envelope, so the denial carries the honest `revoked` reason.
    #[tokio::test]
    async fn dispatch_denies_when_the_backing_token_is_dead_by_any_path() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["pay.invoke".to_string()],
                ttl_secs: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // Kill the TOKEN directly via the manager — not via revoke_standing_grant.
        let token_id = TokenId::from_hex(&out.token_id).unwrap();
        state
            .capability_manager
            .revoke(token_id, "killed out-of-band")
            .await
            .unwrap();
        assert!(
            state.standing_service.is_active(&out.grant_id),
            "precondition: the envelope alone still believes it is live"
        );

        let resp = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "pay.invoke")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.outcome, "denied");
        assert_eq!(resp.reason.as_deref(), Some("revoked"), "the honest reason, not a silent pass");
        assert!(!state.standing_service.is_active(&out.grant_id), "envelope self-healed to revoked");

        // A forged declaration is rejected before any grant lookup or record.
        let mut forged = signed_intent(&out.token_id, "pay.invoke");
        forged.method_id = "tampered".to_string();
        let err = dispatch_standing_intent(State(state), Json(forged)).await;
        assert!(matches!(err, Err((StatusCode::BAD_REQUEST, _))));
    }

    /// The operator's mandate list renders honest card states: a live mandate is ACTIVE, a revoked
    /// one stays LISTED (flagged, never erased), and the response carries the runtime's signer pin.
    #[tokio::test]
    async fn mandate_list_shows_live_and_revoked_cards_with_signer_pin() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let issue = |resource: &str| IssueStandingGrantInput {
            capsule: "vm-agent".to_string(),
            resource: resource.to_string(),
            action: "write".to_string(),
            methods: vec!["pay.invoke".to_string()],
            ttl_secs: Some(3600),
        };
        let a = issue_standing_grant(State(state.clone()), Json(issue("elastos://pay/a")))
            .await
            .unwrap()
            .0;
        let b = issue_standing_grant(State(state.clone()), Json(issue("elastos://pay/b")))
            .await
            .unwrap()
            .0;
        let _ = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput { grant_id: b.grant_id.clone() }),
        )
        .await
        .unwrap();

        let out = list_standing_grants(State(state.clone())).await.0;
        assert_eq!(out.mandates.len(), 2, "revoked mandates stay listed");
        let card = |id: &str| out.mandates.iter().find(|m| m.token_id == id).unwrap();
        assert!(card(&a.token_id).active && !card(&a.token_id).revoked);
        assert!(!card(&b.token_id).active && card(&b.token_id).revoked);
        // The signer pin comes over the authenticated channel and matches the audit log's key.
        assert_eq!(
            out.signer_public_key_hex,
            state.capability_manager.audit_log().verifying_key_hex()
        );

        // Red-team regression: the MASS kill switch (epoch advance) must not leave any card LIVE —
        // the epoch kills every backing token, and the envelope registry is revoked alongside.
        let _ = revoke_all_capabilities(
            State(state.clone()),
            Extension(Session::new(elastos_runtime::session::SessionType::Shell, None)),
            Json(RevokeAllInput { reason: "incident".to_string() }),
        )
        .await
        .expect("revoke-all ok");
        let out = list_standing_grants(State(state)).await.0;
        assert!(
            out.mandates.iter().all(|m| !m.active && m.revoked),
            "after revoke-all, no mandate may render LIVE: {:?}",
            out.mandates
        );
    }

    /// Regression (red-team): `hex::decode` accepts UPPERCASE but the envelope registry is keyed
    /// by the token's lowercase Display form. Keying the envelope kill off the caller's raw string
    /// would revoke the TOKEN yet leave the ENVELOPE live — the dispatch path's only check. The
    /// handler must canonicalize before both kills.
    #[tokio::test]
    async fn revoke_with_uppercase_id_still_kills_the_standing_envelope() {
        let state = test_state();
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: None,
            }),
        )
        .await
        .expect("issue ok")
        .0;
        assert!(state.standing_service.is_active(&out.grant_id));

        let rev = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput {
                grant_id: out.grant_id.to_uppercase(),
            }),
        )
        .await
        .expect("revoke ok")
        .0;
        assert!(rev.revoked, "an UPPERCASE spelling of the id must still kill the envelope");
        assert!(!state.standing_service.is_active(&out.grant_id));
    }

    #[tokio::test]
    async fn issue_standing_grant_is_fail_closed_on_bad_input() {
        let state = test_state();
        // Unknown action ⇒ 400, no grant issued.
        let err = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "frobnicate".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        // Empty method set ⇒ 400 (a grant that authorizes nothing is refused).
        let err2 = issue_standing_grant(
            State(state),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec![],
                ttl_secs: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err2.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn preview_standing_grant_verdicts_and_rejects_forgery() {
        let state = test_state();
        // Issue a grant that authorizes only the "send" method.
        let grant_id = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
            }),
        )
        .await
        .expect("issue ok")
        .0
        .grant_id;

        // The agent signs its own intent declaration.
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let declare = |method: &str| {
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i-preview",
                "vm-agent",
                method,
                "h1",
                "elastos://mail/send",
                "execute",
                &grant_id,
            )
        };

        // In-envelope, authentic ⇒ allowed.
        let ok = preview_standing_grant(State(state.clone()), Json(declare("send")))
            .await
            .expect("preview ok")
            .0;
        assert_eq!(ok.verdict, "allowed");
        assert!(ok.reason.is_none());

        // Out-of-envelope method, authentic ⇒ denied with the honest reason (no side effects).
        let denied = preview_standing_grant(State(state.clone()), Json(declare("delete")))
            .await
            .expect("preview ok")
            .0;
        assert_eq!(denied.verdict, "denied");
        assert_eq!(denied.reason.as_deref(), Some("method_not_in_envelope"));

        // Forged: tamper a signed field AFTER signing so the signature no longer verifies ⇒ 400,
        // rejected on authenticity before any verdict is given.
        let mut forged = declare("send");
        forged.action = "admin".to_string();
        let err = preview_standing_grant(State(state), Json(forged))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn request_capability_records_requester_capsule_identity() {
        // G-ID interim: an HTTP capsule request records the session's real capsule
        // identity (vm_id = "vm-{name}", which the supervisor now populates) on the
        // pending request, so the eventual grant can mint at it. No gate changes yet.
        let state = test_state();
        let out = request_capability(
            State(state.clone()),
            Extension(Session::new_capsule("vm-market".to_string())),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/has".to_string(),
                action: "read".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("request accepted")
        .0;
        let request_id = out.request_id.expect("a pending request id");
        let request = state
            .pending_store
            .get_request(&request_id)
            .await
            .expect("request stored");
        assert_eq!(
            request.requester_capsule_id.as_deref(),
            Some("vm-market"),
            "the HTTP path records the session's capsule identity on the request"
        );
    }

    #[tokio::test]
    async fn grant_mints_at_requester_capsule_identity() {
        // G-ID flip: the granted token is keyed on the requester's capsule identity
        // (session.vm_id = "vm-market"), NOT the session UUID -- so it validates at
        // the carrier/HTTP gates which compare against the same vm-{name}.
        use elastos_runtime::capability::token::CapabilityToken;
        use elastos_runtime::capability::{Action, ResourceId};
        let state = test_state();
        let session = Session::new_capsule("vm-market".to_string());
        let req = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/has".to_string(),
                action: "read".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("request accepted")
        .0;
        let grant = grant_request(
            State(state.clone()),
            Extension(session),
            Json(GrantRequestInput {
                request_id: req.request_id.expect("request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect("grant approved + attested")
        .0;
        let token = CapabilityToken::from_base64(&grant.token.expect("token minted")).unwrap();
        let resource = ResourceId::new("elastos://rights/has");
        assert!(
            state
                .capability_manager
                .validate(&token, "vm-market", Action::Read, &resource, None)
                .await
                .is_ok(),
            "token validates at the capsule identity vm-market (mint off the session-id shim)"
        );
        assert!(
            state
                .capability_manager
                .validate(&token, "vm-other", Action::Read, &resource, None)
                .await
                .is_err(),
            "and not against a different identity"
        );
    }

    #[tokio::test]
    async fn affordance_grant_reads_binding_and_mints_single_use_bound_token() {
        // W2 step 6 (enforcement crux): an affordance-consent request (all four
        // binding fields present) is granted into a token minted at the BOUND
        // capsule, single-use, carrying the exact (method_id, input_hash) the user
        // approved -- distinct from the ordinary G-ID session path.
        use elastos_runtime::capability::token::CapabilityToken;
        use elastos_runtime::capability::{Action, ResourceId};
        let state = test_state();
        // The requesting session is vm-caller; the affordance binds vm-player.
        let session = Session::new_capsule("vm-caller".to_string());
        let req = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/play".to_string(),
                action: "execute".to_string(),
                capsule: Some("vm-player".to_string()),
                principal_id: Some("did:ela:alice".to_string()),
                method_id: Some("play".to_string()),
                input_hash: Some("hash123".to_string()),
            }),
        )
        .await
        .expect("affordance request accepted")
        .0;
        let grant = grant_request(
            State(state.clone()),
            Extension(session),
            Json(GrantRequestInput {
                request_id: req.request_id.expect("request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect("affordance grant approved + attested")
        .0;
        let token = CapabilityToken::from_base64(&grant.token.expect("token minted")).unwrap();

        // The binding the user approved is carried in the (signed) token.
        assert_eq!(token.constraints().method_id(), Some("play"));
        assert_eq!(token.constraints().input_hash(), Some("hash123"));
        assert_eq!(
            token.constraints().max_uses(),
            Some(1),
            "affordance grant is single-use"
        );

        // Minted at the BOUND capsule (vm-player), NOT the requesting session
        // identity (vm-caller): validates at vm-player and nowhere else.
        let resource = ResourceId::new("elastos://rights/play");
        assert!(
            state
                .capability_manager
                .validate(&token, "vm-player", Action::Execute, &resource, None)
                .await
                .is_ok(),
            "token validates at the bound capsule vm-player"
        );
        assert!(
            state
                .capability_manager
                .validate(&token, "vm-caller", Action::Execute, &resource, None)
                .await
                .is_err(),
            "token does not validate at the requesting session identity"
        );
    }

    /// Helper: run request -> grant for an affordance-consent request bound to
    /// vm-player and return the minted base64 token.
    async fn mint_affordance_token(
        state: &CapabilityState,
        method_id: &str,
        input_hash: &str,
    ) -> String {
        let session = Session::new_capsule("vm-player".to_string());
        let req = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/play".to_string(),
                action: "execute".to_string(),
                capsule: Some("vm-player".to_string()),
                principal_id: Some("did:ela:alice".to_string()),
                method_id: Some(method_id.to_string()),
                input_hash: Some(input_hash.to_string()),
            }),
        )
        .await
        .expect("affordance request accepted")
        .0;
        grant_request(
            State(state.clone()),
            Extension(session),
            Json(GrantRequestInput {
                request_id: req.request_id.expect("request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect("affordance grant")
        .0
        .token
        .expect("token minted")
    }

    fn redeem_input(
        token: &str,
        method_id: &str,
        args: serde_json::Value,
    ) -> ValidateAndConsumeInput {
        ValidateAndConsumeInput {
            token: token.to_string(),
            method_id: method_id.to_string(),
            resource: "elastos://rights/play".to_string(),
            action: "execute".to_string(),
            input: args,
        }
    }

    /// Helper: run request -> grant for an ORDINARY (non-affordance) session
    /// capability and return the minted base64 token (no binding).
    async fn mint_session_token(state: &CapabilityState) -> String {
        let session = Session::new_capsule("vm-player".to_string());
        let req = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/play".to_string(),
                action: "execute".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("session request accepted")
        .0;
        grant_request(
            State(state.clone()),
            Extension(session),
            Json(GrantRequestInput {
                request_id: req.request_id.expect("request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect("session grant")
        .0
        .token
        .expect("token minted")
    }

    /// The end-to-end consent journey: a consent-gated invocation is requested,
    /// approved, redeemed for its signed receipt — and every "user said no / the
    /// grant is dead / this isn't a consent token" branch fails closed. The
    /// (method-swap, arg-swap, wrong-caller, replay) matrix is covered by
    /// `validate_and_consume_enforces_binding_single_use_and_caller`; the
    /// signature/expiry/revocation primitives are enforced inside `validate()`
    /// (covered by the capability-conformance battery) and flow through here
    /// unchanged.
    #[tokio::test]
    async fn test_affordance_consent_journey() {
        let state = test_state();
        let hash = elastos_common::canonical_input_hash(&serde_json::json!({"track": "film-x"}));
        let args = || serde_json::json!({"track": "film-x"});
        let player = || Extension(Session::new_capsule("vm-player".to_string()));

        // ── Happy path: request (consent-gated) → grant → redeem → signed receipt.
        let token = mint_affordance_token(&state, "play", &hash).await;
        let redeemed = validate_and_consume(
            State(state.clone()),
            player(),
            Json(redeem_input(&token, "play", args())),
        )
        .await
        .expect("the approved invocation redeems exactly once");
        assert_eq!(redeemed.0.status, "consumed");
        assert!(
            redeemed
                .0
                .receipt
                .verify(state.capability_manager.public_key()),
            "the journey yields a receipt that verifies under the runtime key"
        );
        assert_eq!(redeemed.0.receipt.method_id, "play");
        assert_eq!(redeemed.0.receipt.input_hash, hash);
        assert_eq!(redeemed.0.receipt.capsule, "vm-player");

        // ── Deny branch: the user refuses; the request is denied and never
        // yields a redeemable token.
        let session = Session::new_capsule("vm-player".to_string());
        let pending = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/play".to_string(),
                action: "execute".to_string(),
                capsule: Some("vm-player".to_string()),
                principal_id: Some("did:ela:alice".to_string()),
                method_id: Some("play".to_string()),
                input_hash: Some(hash.clone()),
            }),
        )
        .await
        .expect("consent request accepted")
        .0;
        let denied = deny_request(
            State(state.clone()),
            Extension(session),
            Json(DenyRequestInput {
                request_id: pending.request_id.expect("request id"),
                reason: Some("not now".to_string()),
            }),
        )
        .await
        .expect("deny is recorded")
        .0;
        assert!(denied.success, "the user's refusal is recorded fail-closed");

        // ── Revoked branch: a revoked grant cannot be redeemed.
        let token2 = mint_affordance_token(&state, "play", &hash).await;
        let revoked_id = *CapabilityToken::from_base64(&token2)
            .expect("decode token")
            .id();
        state
            .capability_manager
            .revoke(revoked_id, "journey: revoked before redemption")
            .await
            .unwrap();
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&token2, "play", args())),
            )
            .await
            .is_err(),
            "a revoked grant cannot be redeemed"
        );

        // ── Not-an-affordance-token: an ordinary session token is refused here
        // (it carries no binding; it must use the ordinary capability gate).
        let ordinary = mint_session_token(&state).await;
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&ordinary, "play", args())),
            )
            .await
            .is_err(),
            "an ordinary session token is not redeemable as an affordance consent"
        );
    }

    #[tokio::test]
    async fn validate_and_consume_enforces_binding_single_use_and_caller() {
        // W2 step 7: the runtime re-validates an affordance token AND re-checks the
        // exact (method, args) the user approved, then atomically spends the one
        // use. Every mismatch fails closed; only a correct redemption consumes.
        let state = test_state();
        let hash = elastos_common::canonical_input_hash(&serde_json::json!({"x": 1}));
        let player = || Extension(Session::new_capsule("vm-player".to_string()));

        // Correct redemption succeeds once; replay is refused (single use spent).
        let token = mint_affordance_token(&state, "play", &hash).await;
        let redeemed = validate_and_consume(
            State(state.clone()),
            player(),
            Json(redeem_input(&token, "play", serde_json::json!({"x": 1}))),
        )
        .await
        .expect("a correct redemption consumes the single use");
        // W2 step 9: the redemption returns a signed receipt that verifies under
        // the runtime's capability key and binds the exact (method, args).
        let receipt = &redeemed.0.receipt;
        assert!(
            receipt.verify(state.capability_manager.public_key()),
            "receipt must verify under the runtime capability key"
        );
        assert_eq!(receipt.method_id, "play");
        assert_eq!(receipt.input_hash, hash);
        assert_eq!(receipt.capsule, "vm-player");
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&token, "play", serde_json::json!({"x": 1}))),
            )
            .await
            .is_err(),
            "a single-use affordance token cannot be replayed"
        );

        // A fresh token: method-swap, args-swap, and wrong-caller each fail closed
        // and must NOT burn the use (the binding checks precede the consume).
        let t2 = mint_affordance_token(&state, "play", &hash).await;
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&t2, "delete", serde_json::json!({"x": 1}))),
            )
            .await
            .is_err(),
            "method-swap is refused"
        );
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&t2, "play", serde_json::json!({"x": 2}))),
            )
            .await
            .is_err(),
            "argument-swap is refused"
        );
        assert!(
            validate_and_consume(
                State(state.clone()),
                Extension(Session::new_capsule("vm-other".to_string())),
                Json(redeem_input(&t2, "play", serde_json::json!({"x": 1}))),
            )
            .await
            .is_err(),
            "a different caller identity is refused"
        );
        // The three refused attempts did not spend the use: a correct redemption
        // of the same token still succeeds.
        assert!(
            validate_and_consume(
                State(state.clone()),
                player(),
                Json(redeem_input(&t2, "play", serde_json::json!({"x": 1}))),
            )
            .await
            .is_ok(),
            "fail-closed attempts must not burn the single use"
        );
    }

    #[tokio::test]
    async fn grant_fails_closed_when_no_capsule_identity() {
        // G-ID flip: a Capsule session with no capsule identity (vm_id=None) records
        // requester_capsule_id=None, so the grant FAILS CLOSED (FORBIDDEN) and mints
        // nothing -- never fabricates an identity.
        let state = test_state();
        let session = Session::new(elastos_runtime::session::SessionType::Capsule, None);
        let req = request_capability(
            State(state.clone()),
            Extension(session.clone()),
            Json(RequestCapabilityInput {
                resource: "elastos://rights/has".to_string(),
                action: "read".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("request accepted")
        .0;
        let err = grant_request(
            State(state),
            Extension(session),
            Json(GrantRequestInput {
                request_id: req.request_id.expect("request id"),
                duration: "session".to_string(),
                rationale: None,
            }),
        )
        .await
        .expect_err("grant must fail closed with no capsule identity");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_request_capability_denies_ipfs_backend() {
        let output = request_capability(
            State(test_state()),
            Extension(Session::new_capsule("capsule-1".to_string())),
            Json(RequestCapabilityInput {
                resource: "elastos://ipfs/add".to_string(),
                action: "write".to_string(),
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
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
                capsule: None,
                principal_id: None,
                method_id: None,
                input_hash: None,
            }),
        )
        .await
        .expect("content contract request should be accepted")
        .0;

        assert_eq!(output.status, "pending");
        assert!(output.request_id.is_some());
        assert!(output.reason.is_none());
    }
}
