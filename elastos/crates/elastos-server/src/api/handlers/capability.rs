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
    /// Performs a dispatched intent and reports what was ACTUALLY done — the independent "done" the
    /// reconciliation checks against the declaration. An unregistered method declines (⇒
    /// `Undelivered`), so an authorized-but-unperformed intent is never a fabricated `Matched`.
    pub intent_executor: Arc<dyn crate::intent_executor::IntentExecutor>,
    /// The spend meter backing `runtime.pay` — the SAME instance the executor's pay closure debits,
    /// so the operator provisioning surface and the enforcement path can never disagree. `None` when
    /// no payment rail is wired (the fail-closed default): provisioning is then refused honestly
    /// (503) rather than accepting a budget nothing enforces.
    pub spend_meter: Option<Arc<elastos_runtime::primitives::spend::SpendMeter>>,
    /// The payment ledger (Sprint 30) — the SAME instance the pay closure records into: the
    /// reconciliation surface reads/resolves exactly what enforcement wrote. `None` when pay is
    /// unwired.
    pub payment_ledger: Option<Arc<crate::payment_ledger::PaymentLedger>>,
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

    // The envelope side of the mass kill: the epoch advance kills every backing TOKEN, but the
    // standing-grant registry knows nothing of epochs — without this, an epoch-dead mandate keeps
    // rendering (and, once dispatch is wired, dispatching) as LIVE. A registry persistence failure
    // surfaces, and the error claims only what is established (guardian F2): `advance_epoch` is
    // itself best-effort-persisted and returns the OLD epoch on failure without signaling the
    // handler — so under a correlated disk failure "all tokens dead" would be a guess, not a fact.
    // A mass kill whose envelopes may resurrect as "Live" cards on restart must not report clean
    // success either way.
    state.standing_service.revoke_all().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "epoch advance requested (epoch now {new_epoch}) but the mandate registry could \
                 not record the envelope revokes — retry, and verify the epoch actually \
                 advanced: {e}"
            ),
        )
    })?;

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
    /// Optional AUTHORIZED AGENT ed25519 verifying key (hex). When set, ONLY intents signed by
    /// this key may act under the mandate — the audit attribution is the real agent. Omitted ⇒
    /// capsule-string-only authorization (weaker; see KNOWN_GAPS G-M4).
    #[serde(default)]
    pub agent_pubkey: Option<String>,
    /// Optional per-mandate dispatch-rate budget: acts per `MANDATE_DISPATCH_WINDOW_SECS` window
    /// (Sprint 22 — rate is a first-class grant property, like scope/expiry/agent). Omitted ⇒ the
    /// global default (`MANDATE_DISPATCH_LIMIT`). Zero is refused (a zero-rate mandate authorizes
    /// nothing; revoke is the kill switch, not a budget).
    #[serde(default)]
    pub dispatch_limit: Option<u32>,
    /// The RESPONSIBLE ENTITY (Sprint 32): the DID the OPERATOR DECLARES accountable for every act
    /// under this mandate — the EU-AI-Act liability attribution. Validated (`did:` URI, bounded) and
    /// recorded verbatim in the SIGNED CapabilityGrant chain event, so the portable receipt proves
    /// WHICH entity the operator DECLARED accountable. OPERATOR-ASSERTED, NOT ATTESTED: the runtime
    /// does not resolve the DID or obtain the named entity's counter-signature — a receipt is proof
    /// of the operator's declaration, not the entity's consent (entity counter-signing is a tracked
    /// gap). Optional on the raw API/CLI (back-compat); the web mint surface REQUIRES it (gateway
    /// narrowing, like the agent key).
    #[serde(default)]
    pub responsible_entity: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IssueStandingGrantOutput {
    /// The standing grant id (the backing token's id) — revoke or dispatch against this.
    pub grant_id: String,
    /// The backing capability token's id — identical to `grant_id`, surfaced explicitly because it
    /// is the key for the mandate's audit trail (`export_mandate_receipt_for_capability`).
    pub token_id: String,
}

/// Validate the operator-asserted responsible-entity DID (Sprint 32, council F3/F6). SYNTACTIC
/// only — anti-garbage, bounded, injection-safe; it does NOT authenticate or resolve the DID (that
/// would falsely imply the entity consented — council F2). `None`/blank ⇒ `Ok(None)` (the field is
/// optional on the API/CLI). Shape: `did:<lowercase-alnum-method>:<non-empty-msid>`, ≤256 ASCII
/// chars, charset {alnum : . - _ %} (path/query/fragment excluded — an identifier, not a URL).
pub(crate) fn validate_responsible_entity(raw: Option<&str>) -> Result<Option<String>, String> {
    let did = match raw.map(str::trim) {
        None | Some("") => return Ok(None),
        Some(d) => d,
    };
    let bad = |m: &str| {
        Err(format!(
            "responsible_entity must be a did: URI (e.g. did:web:acme.example), \u{2264}256 chars, \
             DID charset \u{2014} {m}. It is recorded verbatim on the signed chain and must not be \
             garbage"
        ))
    };
    if did.len() > 256 {
        return bad("too long");
    }
    if !did
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '-' | '_' | '%'))
    {
        return bad("illegal character");
    }
    // did:<method>:<msid> — method is a non-empty run of lowercase ASCII alphanumerics; msid is
    // non-empty. splitn(3) keeps any further colons inside the method-specific id.
    let mut parts = did.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("did"), Some(method), Some(msid))
            if !method.is_empty()
                && method
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && !msid.is_empty() =>
        {
            Ok(Some(did.to_string()))
        }
        _ => bad("expected did:<method>:<id> with a lowercase method and a non-empty id"),
    }
}

/// Issue a mandate — the ONE shared mint path, used by the API server's [`issue_standing_grant`]
/// and the gateway's mandates shell app (`api::gateway::gateway_mandates`), so the fail-closed
/// guards (action whitelist, non-empty methods, AUD-5 overbroad-wildcard refusal, agent-key
/// validation, durable-before-visible issuance) can never drift between surfaces.
///
/// Mints a real signed capability token for (capsule, resource, action, ttl) — the cryptographic
/// root — then derives and stores the standing envelope with the authorized method set.
pub async fn issue_mandate(
    standing_service: &StandingGrantService,
    capability_manager: &CapabilityManager,
    input: IssueStandingGrantInput,
) -> Result<IssueStandingGrantOutput, (StatusCode, String)> {
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
    // AUD-5 defense-in-depth (mirrors grant_request): refuse to mint a bare scheme-level wildcard
    // mandate — it would prefix-match every resource under the scheme.
    if is_overbroad_grant_resource(&input.resource) {
        return Err((
            StatusCode::FORBIDDEN,
            "scheme-level wildcard mandates are not permitted; scope to at least one path segment"
                .to_string(),
        ));
    }
    // Validate the optional agent key up front: a present-but-malformed key must fail closed, never
    // silently degrade to an unbound (weaker) mandate.
    let agent_pubkey = match input.agent_pubkey.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(hex_key) => {
            let bytes = hex::decode(hex_key)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("agent_pubkey not hex: {e}")))?;
            let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("agent_pubkey must be 32 bytes (64 hex chars), got {}", bytes.len()),
                )
            })?;
            // Reject a key that is not a real, non-weak ed25519 point (council, Sprint 20 red-team
            // F1): the identity / low-order points parse as valid 32 bytes but a forged signature
            // validates for them under any message — a mandate "bound" to such a key would be
            // satisfiable by anyone, an effectively-UNBOUND mandate wearing a "bound" badge. A
            // non-canonical or small-order key is refused here so a bound mandate always means a
            // real, single-agent binding. (The dispatch gate also uses verify_strict as belt.)
            let vk = ed25519_dalek::VerifyingKey::from_bytes(&arr).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "agent_pubkey is not a valid ed25519 public key".to_string(),
                )
            })?;
            if vk.is_weak() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "agent_pubkey is a weak (small-order) ed25519 key — its binding would be \
                     forgeable; use a real agent key"
                        .to_string(),
                ));
            }
            Some(hex::encode(arr))
        }
    };
    // Confidentiality binding (Sprint 25 council F2): a mandate authorizing `runtime.state_get` READS
    // the principal's durable state. An UNBOUND mandate (agent_pubkey None) skips the gate's
    // WrongAgent check, so it is protected only by token-id secrecy — anyone who learns the token
    // could read the state under the capsule string. Require an agent key for a state_get mandate,
    // fail-closed at the mint. (The gateway shell already binds every mint; this closes the
    // raw-API/CLI path for the confidentiality-sensitive read. The write side `state_put` keeps its
    // pre-existing accepted unbound contract — a symmetric tightening is tracked in KNOWN_GAPS G-M4.)
    if agent_pubkey.is_none()
        && input.methods.iter().any(|m| m == "runtime.state_get")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "a mandate authorizing runtime.state_get must bind an agent key — an unbound state-read \
             mandate would expose the principal's durable state to anyone who learns its token id"
                .to_string(),
        ));
    }
    // The dispatch budget must be in [1, MAX]. Zero would mint a mandate that LOOKS live on the
    // card yet denies every act (revoke is the kill switch, not a budget); an unclamped limit would
    // uncap the fsync-flood bound (council red-team F2 / guardian F3). This is the friendly 400;
    // issue_from_token re-checks the same invariant at the service layer for any non-HTTP caller.
    if let Some(n) = input.dispatch_limit {
        if n == 0 || n > elastos_runtime::capability::MANDATE_DISPATCH_LIMIT_MAX {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "dispatch_limit must be between 1 and {} acts per window (got {}); omit it for \
                     the default. To stop a mandate acting, revoke it.",
                    elastos_runtime::capability::MANDATE_DISPATCH_LIMIT_MAX,
                    n
                ),
            ));
        }
    }
    // Sprint 32: the liability attribution is recorded VERBATIM on the signed chain, so a
    // present-but-malformed entity fails CLOSED (garbage is worse than none). SYNTACTIC
    // anti-garbage + injection-safety ONLY — it does NOT authenticate the DID (that would falsely
    // imply the entity consented — council F2). Re-validated at the service choke point too so a
    // non-HTTP caller can't smuggle an unbounded blob onto the chain (council F6).
    let responsible_entity = validate_responsible_entity(input.responsible_entity.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    let expiry = input
        .ttl_secs
        .map(elastos_common::SecureTimestamp::after_secs);
    // Provability floor (Sprint 24 council F1): grant_durable's "on the chain" is only as durable as
    // the audit log's backing. A MEMORY-ONLY log makes emit() succeed with no disk write, while the
    // standing registry IS persisted — so a restart would restore the mandate without its grant
    // record (the F1 gap, via the audit door). Surface that misconfiguration loudly rather than
    // silently mint an un-provable "durable" mandate. (A hard refusal would break the legitimate
    // ephemeral/dev mode where nothing is cross-restart durable anyway; the durable-custody
    // deployment sets ELASTOS_AUDIT_LOG_PATH and this never fires.)
    if capability_manager.audit_log().log_path().is_none() {
        tracing::warn!(
            capsule = %input.capsule,
            "minting a mandate into a MEMORY-ONLY audit log — its grant record is not cross-restart \
             durable; set ELASTOS_AUDIT_LOG_PATH for provable custody (Sprint 24 F1)"
        );
    }
    // Mint a real signed token (the cryptographic root), then elevate it to a standing grant.
    // FAIL-CLOSED mint (Sprint 24, closes Sprint 23 council F1): the signed durable CapabilityGrant
    // must land on the audit chain BEFORE the mandate exists — so a mandate, whose registry entry is
    // now retention-pruned (Sprint 23), can never exist without a provable grant event. If the audit
    // write fails, no token is returned and no mandate is issued (symmetric with the fail-closed
    // revoke). A best-effort mint could leave a since-pruned mandate with no trace anywhere.
    let token = capability_manager
        .grant_durable(
            &input.capsule,
            ResourceId::new(input.resource),
            action,
            TokenConstraints::default(),
            expiry,
            responsible_entity.clone(),
        )
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("mandate grant could not be durably recorded on the audit chain — not issued: {e}"),
            )
        })?;
    let methods: std::collections::BTreeSet<String> = input.methods.into_iter().collect();
    // Durable-before-visible (G-M5): a mandate that cannot be recorded to the persistent registry
    // is NOT issued — the operator retries into a working store rather than holding a grant that
    // silently evaporates on the next restart.
    let grant_id = standing_service
        .issue_from_token(
            &token,
            methods,
            agent_pubkey,
            input.dispatch_limit,
            responsible_entity,
        )
        .map_err(|e| {
            // Reverse-failure honesty (Sprint 24 council F2): the CapabilityGrant already landed on
            // the chain, but the mandate did NOT get issued. Emit a compensating (best-effort)
            // revoke so the chain reads "granted, then aborted" — an honest completed lifecycle —
            // rather than an orphan grant a receipt-verifier would read as a live, never-revoked
            // mandate. Best-effort: the grant is already durable; this can only make the record more
            // honest, never less.
            capability_manager
                .audit_log()
                .capability_revoke(token.id(), "issuance aborted: standing-registry persist failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("mandate could not be durably recorded — not issued: {e}"),
            )
        })?;
    let token_id = token.id().to_string();
    Ok(IssueStandingGrantOutput { grant_id, token_id })
}

/// POST /api/standing-grants/issue  (shell-only)
///
/// Issue a standing grant for unsupervised agent dispatch — see [`issue_mandate`] for the shared
/// fail-closed mint path.
pub async fn issue_standing_grant(
    State(state): State<CapabilityState>,
    Json(input): Json<IssueStandingGrantInput>,
) -> Result<Json<IssueStandingGrantOutput>, (StatusCode, String)> {
    let out = issue_mandate(&state.standing_service, &state.capability_manager, input).await?;
    Ok(Json(out))
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

/// Revoke a mandate — the ONE shared kill path, used by the API server's [`revoke_standing_grant`]
/// and the gateway's mandates shell app (`api::gateway::gateway_mandates`), so the fail-closed
/// ORDER of the kill can never drift between surfaces.
///
/// Fail-closed AND durably attested. The grant id IS the backing capability token's id, and killing
/// only the in-memory envelope would leave the mandate's audit trail showing it live forever — so
/// this first revokes the BACKING TOKEN through the manager (which emits the signed
/// `CapabilityRevoke` record BEFORE mutating, per AUD-3: if the durable write fails, the revoke
/// ABORTS and the error surfaces rather than a revoke existing with no record), then kills the
/// standing envelope so the dispatcher denies every not-yet-started act. The mandate's receipt
/// (`export_mandate_receipt_for_capability`) thereafter carries the revoke. `reason` names the
/// surface that pulled the switch — it lands verbatim in the signed audit record.
pub async fn revoke_mandate(
    standing_service: &StandingGrantService,
    capability_manager: &CapabilityManager,
    grant_id: &str,
    reason: &str,
) -> Result<bool, (StatusCode, String)> {
    let token_id = TokenId::from_hex(grant_id.trim()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("grant_id is not a valid token id: {e}"),
        )
    })?;
    // Durable custody first (emit-before-mutate): a revoke that cannot be signed onto the audit
    // chain does not happen — mirroring revoke_capability. The envelope stays live so the failure
    // is loud and re-runnable, never a silent half-revoke the receipt can't prove.
    capability_manager.revoke(token_id, reason).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("revoke could not be durably attested: {e}"),
        )
    })?;
    // Kill the envelope by the CANONICAL id (the parsed token's lowercase-hex Display form — the
    // exact key the registry stores). Keying off the caller's raw string would let an UPPERCASE
    // spelling revoke the token yet miss the envelope, leaving the dispatch path live. A registry
    // persistence failure surfaces loudly; the error claims exactly what the manager guarantees
    // (guardian F1): the signed CapabilityRevoke RECORD is durable (emit-before-mutate) and the
    // token is revoked in THIS runtime, but the token-state persist itself is best-effort
    // (`persist_revoked_tokens` logs, never errors) — so under a failing disk the honest claim is
    // "attested + revoked here", not "durably revoked". The caller retries (idempotent) rather
    // than trusting a revoke the registry may forget.
    standing_service.revoke(&token_id.to_string()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "revoke durably attested (signed audit record) and token revoked in this \
                 runtime, but the mandate registry could not record the envelope revoke — \
                 retry: {e}"
            ),
        )
    })
}

/// POST /api/standing-grants/revoke  (shell-only) — the autonomy kill switch.
///
/// See [`revoke_mandate`] for the fail-closed semantics (durably attest FIRST, then kill the
/// envelope).
pub async fn revoke_standing_grant(
    State(state): State<CapabilityState>,
    Json(input): Json<RevokeStandingGrantInput>,
) -> Result<Json<RevokeStandingGrantOutput>, (StatusCode, String)> {
    let revoked = revoke_mandate(
        &state.standing_service,
        &state.capability_manager,
        &input.grant_id,
        "standing grant revoked via API",
    )
    .await?;
    Ok(Json(RevokeStandingGrantOutput { revoked }))
}

// === Spend budgets (Sprint 28) — the operator provisioning surface for the runtime.pay cap ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSpendBudgetInput {
    /// The capsule (agent identity) whose money cap is being provisioned — the same string the pay
    /// closure debits (`intent.capsule`), held to the same slug bound the executor enforces.
    pub capsule: String,
    /// The TOTAL budget (cumulative cap). Lowering below what is already spent clamps remaining to
    /// zero — it never refunds silently.
    pub limit: u64,
}

#[derive(Debug, Serialize)]
pub struct SpendBudgetOutput {
    pub capsule: String,
    pub limit: u64,
    pub spent: u64,
    pub remaining: u64,
    /// True when the enforcing meter is POISONED (a persist failed post-publish): every payment
    /// and every provisioning change is refusing fail-closed until the process is restarted
    /// (council S29 F5/F10 — the ops surface for `SpendMeter::is_poisoned`).
    pub poisoned: bool,
    /// Units of this capsule's cap held by INDETERMINATE payments awaiting rail reconciliation
    /// (Sprint 30, council S29 RT-F4): spend the operator must NOT treat as confirmed — and must
    /// reconcile BEFORE raising the cap, or the raise can authorize real spend beyond intent.
    pub pending_units: u64,
    /// Count of this capsule's pending (indeterminate) payments.
    pub pending_count: u64,
}

/// POST /api/spend-budgets  (shell-only) — provision (or re-set) a capsule's money cap.
///
/// This is real operator AUTHORITY, so it is held to the mandate discipline:
/// - **Same-instance enforcement:** the meter here IS the one the executor's pay closure debits —
///   what the operator sets is exactly what is enforced, by construction.
/// - **Fail-closed without a rail:** no wired payment rail ⇒ no meter ⇒ 503. A budget nothing
///   enforces is never accepted.
/// - **Durable-only:** a money cap on a meter that refills across restart is a lie; a non-durable
///   meter refuses provisioning (503) rather than accepting a cap it cannot keep.
/// - **Attested:** the budget change lands on the signed audit chain as a `ConfigChange`
///   (`spend_budget:{capsule}`, old → new). Apply-then-attest with rollback: the event is emitted
///   only AFTER the budget is durably in force (the chain never claims a cap that isn't real), and
///   if the emit fails the budget is rolled back — a first-time provision is fully REMOVED, never
///   left as a provisioned-at-zero artifact (council S28 F7). Under a single failure no unattested
///   cap stays in force. Residuals, stated honestly (council S28 F2/F5): if the ROLLBACK itself
///   cannot persist (a correlated failure — audit chain and meter share `data_dir`), the cap the
///   operator requested may remain durably in force UNATTESTED; that double failure is surfaced
///   loudly (distinct 500 naming it, error log, best-effort attestation) — never silently
///   discarded. And a pay racing the failed attestation may appear on-chain under a cap whose
///   `ConfigChange` never landed (the payment's own receipt still lands; apply-then-attest is the
///   honest ordering — the inverse would put a FALSE claim on the chain).
pub async fn set_spend_budget(
    State(state): State<CapabilityState>,
    Json(input): Json<SetSpendBudgetInput>,
) -> Result<Json<SpendBudgetOutput>, (StatusCode, String)> {
    let Some(meter) = &state.spend_meter else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired — a spend budget nothing enforces is refused".to_string(),
        ));
    };
    set_spend_budget_core(
        meter,
        state.payment_ledger.as_deref(),
        state.capability_manager.audit_log(),
        input,
    )
    .map(Json)
}

/// The ONE shared provisioning path (Sprint 31): the API server's shell route and the gateway's
/// Mandates-app route both delegate here, so the fail-closed guards (durable-only, slug bound,
/// apply-then-attest with true rollback, loud double-failure) can never drift between surfaces —
/// the same one-source-of-truth rule as `issue_mandate`/`revoke_mandate`.
pub(crate) fn set_spend_budget_core(
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    ledger: Option<&crate::payment_ledger::PaymentLedger>,
    audit_log: &elastos_runtime::primitives::audit::AuditLog,
    input: SetSpendBudgetInput,
) -> Result<SpendBudgetOutput, (StatusCode, String)> {
    if !meter.is_durable() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the spend meter is not durable — a money cap that refills on restart is refused"
                .to_string(),
        ));
    }
    if !crate::intent_executor::valid_slug_1_64(&input.capsule) {
        return Err((
            StatusCode::BAD_REQUEST,
            "capsule must be a 1-64 char slug (the identity the pay gate debits)".to_string(),
        ));
    }
    // The PRIOR limit comes back from set_budget itself, read under the meter's write lock in the
    // same critical section as the mutation (council S28 F6) — the one linearizable old-value the
    // attestation and the rollback can both trust. Two lock-free snapshot() reads here would let
    // concurrent provisions attest the same stale "old".
    let prior_limit = meter.set_budget(&input.capsule, input.limit).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("budget not applied (durable persist failed, rolled back): {e}"),
        )
    })?;
    let config_change = || elastos_runtime::primitives::audit::AuditEvent::ConfigChange {
        timestamp: elastos_runtime::primitives::SecureTimestamp::now(),
        setting: format!("spend_budget:{}", input.capsule),
        old_value: prior_limit
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unprovisioned".to_string()),
        new_value: input.limit.to_string(),
    };
    if let Err(e) = audit_log.emit(config_change()) {
        // No unattested cap stays in force: restore the prior limit, or fully REMOVE a first-time
        // provision (never a provisioned-at-zero artifact the chain never granted — F7).
        let rollback = match prior_limit {
            Some(limit) => meter.set_budget(&input.capsule, limit).map(|_| ()),
            None => meter.remove_budget(&input.capsule).map(|_| ()),
        };
        if let Err(rb) = rollback {
            // DOUBLE FAILURE (council S28 F2, correlated: chain and meter share data_dir): the cap
            // the operator asked for is durably in force but its attestation never landed and the
            // rollback could not persist. Surface it loudly and distinctly — the operator must
            // intervene — and try the attestation once more best-effort (the chain may recover).
            tracing::error!(
                capsule = %input.capsule,
                limit = input.limit,
                "spend budget applied but NOT attested, and the rollback could not persist \
                 ({rb}) — an unattested money cap may be in force; operator intervention required"
            );
            audit_log.emit_best_effort(config_change());
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "budget applied but could NOT be attested on the audit chain ({e}), and the \
                     rollback could not persist ({rb}) — the cap may remain in force unattested; \
                     operator intervention required"
                ),
            ));
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("budget change could not be attested on the audit chain — rolled back: {e}"),
        ));
    }
    let snap = meter.snapshot(&input.capsule).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "budget vanished after set".to_string(),
    ))?;
    let (pending_units, pending_count) =
        ledger.map(|l| l.pending_for(&input.capsule)).unwrap_or((0, 0));
    Ok(SpendBudgetOutput {
        capsule: input.capsule,
        limit: snap.limit,
        spent: snap.spent,
        remaining: snap.remaining,
        poisoned: meter.is_poisoned(),
        pending_units,
        pending_count: pending_count as u64,
    })
}

/// GET /api/spend-budgets/:capsule  (shell-only) — the read-only projection of one capsule's money
/// cap (limit / spent / remaining), straight off the enforcing meter. 404 when unprovisioned.
pub async fn get_spend_budget(
    State(state): State<CapabilityState>,
    Path(capsule): Path<String>,
) -> Result<Json<SpendBudgetOutput>, (StatusCode, String)> {
    let Some(meter) = &state.spend_meter else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        ));
    };
    let snap = meter.snapshot(&capsule).ok_or((
        StatusCode::NOT_FOUND,
        "no spend budget provisioned for this capsule".to_string(),
    ))?;
    let (pending_units, pending_count) = state
        .payment_ledger
        .as_ref()
        .map(|l| l.pending_for(&capsule))
        .unwrap_or((0, 0));
    Ok(Json(SpendBudgetOutput {
        capsule,
        limit: snap.limit,
        spent: snap.spent,
        remaining: snap.remaining,
        poisoned: meter.is_poisoned(),
        pending_units,
        pending_count: pending_count as u64,
    }))
}

// === Payment reconciliation (Sprint 30) — resolving INDETERMINATE rail outcomes ===

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcilePaymentInput {
    /// The pending payment's idempotency key (from the decline reason / the pending list) — the
    /// handle the operator looked up ON THE RAIL before calling this.
    pub idempotency_key: String,
    /// The rail's verdict: `true` = the charge DID post (spend stands); `false` = it did NOT
    /// (the reservation is refunded).
    pub charged: bool,
}

#[derive(Debug, Serialize)]
pub struct ReconcilePaymentOutput {
    pub idempotency_key: String,
    pub capsule: String,
    pub amount: u64,
    /// The terminal status the entry resolved to.
    pub status: String,
    /// True iff the reservation was refunded AND the refund is durably in force.
    pub refunded: bool,
    /// Honest caveats the operator must read (refund persist failure, attestation failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// GET /api/payments/pending  (shell-only) — the reconciliation work list: every INDETERMINATE
/// payment (reservation held, outcome unknown), oldest first, with the idempotency key to look up
/// on the rail. Read-only projection of the ledger the pay path writes.
pub async fn list_pending_payments(
    State(state): State<CapabilityState>,
) -> Result<Json<Vec<crate::payment_ledger::PaymentRecord>>, (StatusCode, String)> {
    let Some(ledger) = &state.payment_ledger else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        ));
    };
    Ok(Json(ledger.pending()))
}

/// POST /api/payments/reconcile  (shell-only) — resolve one INDETERMINATE payment against the
/// rail's verdict. This is the ONLY path that releases an indeterminate reservation, and it is
/// deliberately manual-first: the operator asserts what the RAIL says (looked up by idempotency
/// key), never a guess.
///
/// Ordering, fail-closed against double-refund:
/// 1. The ledger entry resolves EXACTLY once (durable-before-visible with rollback) — a second
///    call, a retry, or a race gets 409 and can never refund twice. Errors are STRUCTURED
///    (council S30 G-F2) so automation never mistakes a retryable failure for a terminal one:
///    404 = no entry; 409 = already terminal; 503 = the resolution could not persist and the
///    entry is STILL PENDING — retry.
/// 2. Only then, and only for `charged=false`, the reservation is refunded via `try_refund` —
///    "refunded" is claimed ONLY when durably in force; a refund persist failure is surfaced in
///    `warning` (the entry stays resolved: the cap remains debited, recover by raising the limit).
///    An applied refund is marked on the ledger entry (`refund_applied`).
/// 3. The resolution is attested on the signed chain (`Custom` payment_reconciled event) with the
///    ACTUAL outcomes; an attestation failure is surfaced in `warning`, never hidden (the ledger
///    and meter are already consistent — un-resolving would reopen the double-refund window).
///
/// CRASH WINDOW, stated (council S30 G-F3): a crash between step 1 and step 2 leaves the entry
/// terminally `ResolvedNotCharged` with the refund never applied and no chain event — fail-closed
/// (the cap stays debited; exactly-once holds). FORENSIC: a `resolved_not_charged` ledger entry
/// with `refund_applied=false` and no matching `payment_reconciled` chain event means the refund
/// was lost; the recovery lever is raising the limit.
pub async fn reconcile_payment(
    State(state): State<CapabilityState>,
    Json(input): Json<ReconcilePaymentInput>,
) -> Result<Json<ReconcilePaymentOutput>, (StatusCode, String)> {
    let (Some(ledger), Some(meter)) = (&state.payment_ledger, &state.spend_meter) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        ));
    };
    reconcile_payment_core(ledger, meter, state.capability_manager.audit_log(), input).map(Json)
}

/// The ONE shared reconciliation path (Sprint 31): both surfaces (API shell route, gateway
/// Mandates-app route) delegate here so the exactly-once resolve, honest refund claim, structured
/// error mapping, and chain attestation can never drift.
pub(crate) fn reconcile_payment_core(
    ledger: &crate::payment_ledger::PaymentLedger,
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    audit_log: &elastos_runtime::primitives::audit::AuditLog,
    input: ReconcilePaymentInput,
) -> Result<ReconcilePaymentOutput, (StatusCode, String)> {
    // Council S31 G-F5: a not-charged resolution's REFUND cannot apply while the meter is
    // poisoned — and the resolve is one-shot, so proceeding would burn the refund handle
    // permanently. Refuse BEFORE touching the ledger (503: retryable after restart). A
    // charged=true verdict touches no meter and stays allowed.
    if !input.charged && meter.is_poisoned() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the spend meter is poisoned — a not-charged resolution's refund cannot apply; \
             restart the runtime, then reconcile (resolving now would permanently burn the \
             one-shot refund handle)"
                .to_string(),
        ));
    }
    let record = ledger
        .resolve(&input.idempotency_key, input.charged)
        .map_err(|e| {
            let status = match e {
                crate::payment_ledger::ResolveError::NotFound => StatusCode::NOT_FOUND,
                crate::payment_ledger::ResolveError::NotPending(_) => StatusCode::CONFLICT,
                // The obligation is STILL LIVE — automation must retry, never drop the work item.
                crate::payment_ledger::ResolveError::Persist
                | crate::payment_ledger::ResolveError::Lock => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, format!("cannot reconcile: {e}"))
        })?;
    let mut warning = None;
    let refunded = if input.charged {
        false
    } else {
        match meter.try_refund(&record.capsule, record.amount) {
            Ok(_) => {
                ledger.mark_refund_applied(&record.idempotency_key);
                true
            }
            Err(e) => {
                warning = Some(format!(
                    "resolution recorded but the refund could not be durably applied ({e}) — \
                     the cap remains debited; the operator's lever is raising the limit"
                ));
                false
            }
        }
    };
    let attested = audit_log.emit(
        elastos_runtime::primitives::audit::AuditEvent::Custom {
            event_type: "payment_reconciled".to_string(),
            details: serde_json::json!({
                "idempotency_key": record.idempotency_key,
                "capsule": record.capsule,
                "payee": record.payee,
                "amount": record.amount,
                "charged": input.charged,
                "refunded": refunded,
            }),
        },
    );
    if let Err(e) = attested {
        tracing::error!(
            key = %record.idempotency_key,
            "payment reconciliation could not be attested on the audit chain: {e}"
        );
        let note = format!(
            "reconciliation applied but NOT attested on the audit chain ({e}) — the ledger and \
             meter are consistent; the chain record is missing"
        );
        warning = Some(match warning {
            Some(w) => format!("{w}; {note}"),
            None => note,
        });
    }
    Ok(ReconcilePaymentOutput {
        idempotency_key: record.idempotency_key,
        capsule: record.capsule,
        amount: record.amount,
        status: format!("{:?}", record.status),
        refunded,
        warning,
    })
}

/// The Money panel's one-call read (Sprint 31): every provisioned budget (with pending
/// visibility) + the reconciliation work list + the poisoned flag — a read-only projection of the
/// enforcing meter and ledger, shared by both surfaces.
#[derive(Debug, Serialize)]
pub struct MoneyOverview {
    pub budgets: Vec<SpendBudgetOutput>,
    pub pending: Vec<crate::payment_ledger::PaymentRecord>,
    pub poisoned: bool,
}

pub(crate) fn money_overview(
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    ledger: &crate::payment_ledger::PaymentLedger,
) -> MoneyOverview {
    let poisoned = meter.is_poisoned();
    let budgets = meter
        .snapshot_all()
        .into_iter()
        .map(|(capsule, snap)| {
            let (pending_units, pending_count) = ledger.pending_for(&capsule);
            SpendBudgetOutput {
                capsule,
                limit: snap.limit,
                spent: snap.spent,
                remaining: snap.remaining,
                poisoned,
                pending_units,
                pending_count: pending_count as u64,
            }
        })
        .collect();
    MoneyOverview {
        budgets,
        pending: ledger.pending(),
        poisoned,
    }
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
    /// Is the mandate BOUND to one authorized agent key? `true` ⇒ only intents signed by that key
    /// may act (strong attribution). `false` ⇒ capsule-string-only authorization: ANY key acting as
    /// the capsule passes (weaker; G-M4). Surfaced so the operator can SEE the attribution strength
    /// of every mandate, not just trust it (P12).
    pub agent_bound: bool,
    /// The authorized agent's ed25519 verifying key (hex), when bound — the operator's own issued
    /// key, so exposing it here is not a leak (public key, operator surface). `None` when unbound.
    pub agent_pubkey: Option<String>,
    /// The dispatch-rate budget ENFORCED for this mandate: acts per `dispatch_window_secs` window.
    /// Always the effective number (the mandate's own limit when it set one, else the global
    /// default) — the card shows what the gate does, not a config abstraction (P12).
    pub dispatch_limit: u32,
    /// Whether `dispatch_limit` was set on THIS mandate at grant time (`true`) or is the global
    /// default (`false`) — so the operator can tell a deliberate dial from the baseline.
    pub dispatch_limit_custom: bool,
    /// The rate window (seconds) the budget is measured over.
    pub dispatch_window_secs: u64,
    /// The RESPONSIBLE ENTITY (Sprint 32): the DID the operator DECLARED accountable — the working
    /// registry MIRROR of the signed grant record. `None` = not recorded in the registry (a pre-S32
    /// or CLI-minted mandate, OR a hand-edited/repaired snapshot that dropped the unsigned field).
    /// AUTHORITATIVE SOURCE IS THE RECEIPT: the signed grant record in `elastos verify-receipt` is
    /// the ground truth; this card projection can be blanked/altered by a data_dir writer (the whole
    /// registry snapshot is unsigned) without touching the receipt (council S32 F3/F4).
    pub responsible_entity: Option<String>,
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

/// Build the operator's mandate list — the ONE source of truth for the "mandate card" projection,
/// shared by the API server's [`list_standing_grants`] and the gateway's read-only mandates app
/// (`api::gateway::gateway_mandates`), so the liveness invariant can never drift between the two
/// surfaces (a revoked/expired mandate rendering "Live" on one but not the other).
///
/// The card's LIVE bit must consult ALL kill paths, because the envelope's own `revoked` flag sees
/// only standing/`revoke_all` revocation — a backing token can also die by (a) an individual token
/// revoke through the manager's revocation store, or (b) a key-rotation EPOCH advance (captured in
/// the envelope at issue). A mandate killed by any of these must never render live. An unparseable
/// id is fail-closed inactive.
pub async fn mandate_cards(
    standing_service: &StandingGrantService,
    capability_manager: &CapabilityManager,
) -> ListMandatesOutput {
    let mut mandates = Vec::new();
    for env in standing_service.list() {
        let token_dead = match TokenId::from_hex(&env.grant_id) {
            Ok(id) => {
                capability_manager.is_token_revoked(&id).await
                    || !capability_manager.is_epoch_valid(env.token_epoch)
            }
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
            agent_bound: env.agent_pubkey.is_some(),
            agent_pubkey: env.agent_pubkey,
            dispatch_limit: env
                .dispatch_limit
                .unwrap_or(elastos_runtime::capability::MANDATE_DISPATCH_LIMIT),
            dispatch_limit_custom: env.dispatch_limit.is_some(),
            dispatch_window_secs: elastos_runtime::capability::MANDATE_DISPATCH_WINDOW_SECS,
            responsible_entity: env.responsible_entity,
        });
    }
    ListMandatesOutput {
        mandates,
        signer_public_key_hex: capability_manager.audit_log().verifying_key_hex(),
    }
}

/// GET /api/standing-grants  (shell-only) — the operator's mandate list.
pub async fn list_standing_grants(
    State(state): State<CapabilityState>,
) -> Json<ListMandatesOutput> {
    Json(mandate_cards(&state.standing_service, &state.capability_manager).await)
}

#[derive(Debug, Serialize)]
pub struct DispatchIntentOutput {
    /// The outcome, reflecting the gate AND the reconciliation:
    /// - `denied` — the intent fell outside a live mandate (see `reason`); nothing performed.
    /// - `performed` — authorized AND a real executor performed it as declared (`Matched`).
    /// - `diverged` — authorized, but the executor did something other than declared (`Diverged`).
    /// - `authorized_not_performed` — authorized, but NO RECEIPT ATTESTS performance
    ///   (`Undelivered`). This is absence-of-attestation, not proof-of-absence: for a MONEY act
    ///   the effect may still have occurred (an INDETERMINATE rail outcome — timeout/5xx/panic —
    ///   deliberately reconciles here; council S29 F5). Check `reason` before treating it as
    ///   "nothing happened".
    ///
    /// Only `performed` records a successful `CapabilityUse` in the mandate receipt.
    pub outcome: String,
    /// The fail-closed denial reason (snake_case) when `denied`; for an authorized-but-declined
    /// act, the EXECUTOR's decline reason (S29 F2) — for money acts this distinguishes a
    /// cap-refusal from an INDETERMINATE outcome and names the idempotency key to reconcile with
    /// the rail. `None` when performed.
    pub reason: Option<String>,
    /// The signed reconciliation when the intent was authorized (any of performed/diverged/
    /// authorized_not_performed); `None` when denied. Its `status` is the independent verdict of
    /// what the executor actually did vs. what was declared.
    pub reconciliation: Option<elastos_runtime::capability::IntentReconciliationV1>,
}

/// POST /api/agent/dispatch  (AGENT-FACING — Sprint 26) — the same ACT leg, but reachable by the
/// AGENT itself, not only the operator/shell. This is the North-Star move: "a mandate, not your
/// keys". The route carries NO consent-broker (shell-role) gate; the agent authenticates AS the
/// mandate holder — `verify_self` proves it holds a private key, and the mandate's agent-key binding
/// (G-M4) proves that key is the authorized agent. No ambient authority (P3): an UNBOUND mandate is
/// REFUSED here (only the operator's shell route may dispatch an unbound mandate), and a wrong-key
/// intent is refused BEFORE any rate budget is charged or durable write is made — CHARGE-ON-AUTHORIZED,
/// which closes the Sprint 21 victim-lockout residual (an attacker naming a victim's grant can no
/// longer burn its budget, because it never clears this gate). The response is a uniform 403 for
/// absent / unbound / wrong-key, so this less-trusted surface is not a grant-existence or binding
/// oracle. Everything past the gate is the identical hardened pipeline (freshness → per-mandate rate
/// → replay guard → liveness → gate → act → reconcile → receipt); the gate RE-checks the binding, so
/// this wrapper only ADDS the agent authentication + the charge-on-authorized ordering.
pub async fn dispatch_agent_intent(
    State(state): State<CapabilityState>,
    Json(intent): Json<IntentDeclarationV1>,
) -> Result<Json<DispatchIntentOutput>, (StatusCode, String)> {
    // 1. The intent must be validly self-signed before we trust `intent.signer`.
    if !intent.verify_self() {
        return Err((
            StatusCode::BAD_REQUEST,
            "intent declaration signature did not verify".to_string(),
        ));
    }
    // 2. Authenticate as the mandate holder: the named mandate must EXIST, be agent-BOUND, and its
    //    bound key must be the intent's signer. Absent / unbound / wrong-key all fail-closed the
    //    SAME way (no oracle). This runs BEFORE delegating, so the pipeline's rate budget + durable
    //    replay write are only ever reached by an authorized caller (charge-on-authorized).
    let authorized = TokenId::from_hex(intent.standing_grant_id.trim())
        .ok()
        .and_then(|t| state.standing_service.get(&t.to_string()))
        .and_then(|grant| grant.agent_pubkey)
        .map(|bound| intent.signer.trim().eq_ignore_ascii_case(bound.trim()))
        .unwrap_or(false);
    if !authorized {
        return Err((
            StatusCode::FORBIDDEN,
            "not authorized: agent dispatch requires an intent signed by the key your mandate is \
             bound to (unbound mandates are dispatched only from the operator shell)"
                .to_string(),
        ));
    }
    // 3. Delegate to the ONE hardened pipeline — the single source of truth (it re-verifies the
    //    signature and re-checks the binding in the gate). This wrapper adds only the agent auth.
    dispatch_standing_intent(State(state), Json(intent)).await
}

/// POST /api/standing-grants/dispatch  (shell-only) — the ACT leg of the mandate loop.
///
/// Run ONE agent act under its standing mandate, fail-closed. In order:
/// 1. authenticate the signed [`IntentDeclarationV1`] (`verify_self`) — a forged declaration is
///    rejected before any lookup or record;
/// 2. REPLAY GUARD (G-M5): each `intent_id` acts at most once per runtime lifetime — a re-POSTed
///    signed blob is refused `409`, so a captured/retried declaration cannot double-act;
/// 3. LIVENESS (G-M1): consult the manager's token-revocation store AND epoch validity (the pure
///    envelope gate can see neither) — if the backing token is dead by ANY path (individual
///    revoke, `revoke_all`, or a key-rotation epoch advance) the envelope is healed to revoked so
///    the gate denies with the true `revoked` reason;
/// 4. run the intent gate: the declaration is recorded on-chain BEFORE the act (no custody ⇒ no
///    act), and the gate binds capsule + BOUND AGENT KEY (G-M4, when the mandate set one) + method
///    + resource + action, all exact;
/// 5. run the intent gate: ONLY on authorization does the act closure invoke the real
///    [`IntentExecutor`](crate::intent_executor::IntentExecutor) — so a denied/revoked intent never
///    executes — and the receipt is minted from what the executor REPORTS it performed (G-M6);
/// 6. record a token-keyed `CapabilityUse` (G-M2) whose `success` reflects the reconciliation
///    STATUS (a real `matched` performance), so the mandate's exported receipt carries the act.
///
/// Honest scope: reconciliation attests report-fidelity (report == declaration), not reality
/// (effect == declaration) — that rests on the trusted-core executor's truthfulness. The production
/// executor set currently registers ONE real affordance (`runtime.audit_verify`, a side-effect-free
/// signature-verified chain read); every other method is `authorized_not_performed` until wired (G-M6).
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
    // FRESHNESS WINDOW (G-M7): a signed declaration EXPIRES — its `declared_at` must sit within
    // `[now - MAX_INTENT_AGE, now + MAX_CLOCK_SKEW]`. This bounds how long a captured intent can be
    // replayed AND lets the replay guard forget anything older than the window (so its seen-set is
    // bounded, not monotonic). Checked AFTER authenticity (a forged declaration is rejected first)
    // and BEFORE the replay guard registers the id, so a stale/future intent never burns an id.
    let now_secs = elastos_common::SecureTimestamp::now().unix_secs;
    if let Err(reason) = elastos_runtime::capability::check_intent_freshness(
        intent.declared_at.unix_secs,
        now_secs,
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("intent declaration outside the freshness window: {}", reason.as_str()),
        ));
    }
    // GRANT EXISTENCE (G-M7, Sprint 21 council fix): the intent names a standing grant. Resolve it
    // in memory HERE — after authenticity + freshness, but BEFORE the rate budget AND the durable
    // replay write. An unparseable or UNKNOWN grant_id is refused cheaply now, so a flood of
    // distinct FAKE grant_ids (each self-signed + fresh; `standing_grant_id` is attacker-chosen and
    // is only bound to a real mandate LATER, at the gate) can neither (a) reach the durable
    // replay-guard fsync nor (b) create a rate-map entry. This is what actually makes the rate
    // budget bound the fsync flood and keeps the rate map bounded: only REAL, operator-issued
    // grant_ids — a registry-bounded set — are ever counted or durably recorded. A never-issued
    // grant yields the SAME `denied`/`no_standing_grant` verdict the gate would, without paying for
    // it. (Real-but-revoked grants stay in the registry, so they pass here and are denied with the
    // true `revoked` reason downstream.)
    let grant_exists = TokenId::from_hex(intent.standing_grant_id.trim())
        .ok()
        .map(|t| state.standing_service.get(&t.to_string()).is_some())
        .unwrap_or(false);
    if !grant_exists {
        return Ok(Json(DispatchIntentOutput {
            outcome: "denied".to_string(),
            reason: Some(
                elastos_runtime::capability::EnvelopeDenial::NoGrant
                    .as_str()
                    .to_string(),
            ),
            reconciliation: None,
        }));
    }
    // RATE BUDGET (G-M7): each mandate may perform at most MANDATE_DISPATCH_LIMIT acts per window.
    // Checked AFTER authenticity + freshness + grant-existence (only well-formed fresh intents
    // naming a REAL mandate count) and BEFORE the replay guard's durable write, so a mandate-holding
    // agent flooding distinct intents is refused (429) before it costs an fsync + registry growth —
    // bounding the durable-write flood the replay guard's compaction alone did not (Sprint 19
    // bounded the SET; this bounds the RATE).
    if !state
        .standing_service
        .record_dispatch_within_budget(&intent.standing_grant_id, now_secs)
    {
        // Report THIS mandate's resolved budget, not the global default (council F1, P12): a
        // mandate dialed to 2/min must not be told it exceeded "60 acts" — the message would
        // contradict the gate. Re-resolve the enforced limit from the registry for the message.
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "mandate {} exceeded its dispatch rate budget ({} acts per {}s)",
                intent.standing_grant_id,
                state
                    .standing_service
                    .dispatch_limit_for(&intent.standing_grant_id),
                elastos_runtime::capability::MANDATE_DISPATCH_WINDOW_SECS
            ),
        ));
    }
    // Replay guard (G-M5): register the intent id BEFORE anything acts — durably, so the guard
    // survives restart. A duplicate is refused with no record and no act. Register only AFTER
    // authenticity + freshness (above) so a forged or stale blob cannot burn a future-legitimate
    // id. A guard that cannot be durably recorded REFUSES the act (fail-closed) with its true
    // reason — an intent that acts without a surviving replay record could act again after a reboot.
    match state.standing_service.record_fresh_intent(
        &intent.intent_id,
        intent.declared_at.unix_secs,
        now_secs,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return Err((
                StatusCode::CONFLICT,
                // "consumed", not "dispatched" (guardian F5): the id burns when REGISTERED — a
                // prior attempt may have been refused after registration and never acted.
                format!("intent {} was already consumed (replay refused)", intent.intent_id),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("replay guard could not be durably recorded — intent refused: {e}"),
            ));
        }
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
    // Liveness (G-M1): the envelope gate sees only its own `revoked` flag. A backing token can die
    // by three other paths the gate cannot see — individual revoke, `revoke_all`, and a key-rotation
    // epoch advance. Consult BOTH the revocation store and epoch validity (using the epoch captured
    // in the envelope at issue), and heal the envelope to revoked so the gate denies with the true
    // `revoked` reason and it sticks. Unparseable ids fail closed at the gate (NoGrant).
    if let Ok(token_id) = TokenId::from_hex(intent.standing_grant_id.trim()) {
        let revoked = state.capability_manager.is_token_revoked(&token_id).await;
        let epoch_dead = state
            .standing_service
            .get(&token_id.to_string())
            .map(|env| !state.capability_manager.is_epoch_valid(env.token_epoch))
            .unwrap_or(false);
        if revoked || epoch_dead {
            // The heal must STICK before dispatch proceeds: if the registry cannot record the
            // envelope revoke, refuse the intent rather than let the gate read a live envelope
            // whose backing token is dead. (The gate itself would still deny via the token check,
            // but a fail-open heal here would leave disk claiming LIVE across a restart.)
            if let Err(e) = state.standing_service.revoke(&token_id.to_string()) {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "mandate is token-dead but the registry could not record the envelope \
                         revoke — intent refused: {e}"
                    ),
                ));
            }
        }
    }
    let manager = state.capability_manager.clone();
    let executor = state.intent_executor.clone();
    let token_str = intent.standing_grant_id.clone();
    let intent_for_exec = intent.clone();
    // If the executor PERFORMS but reports an action the runtime cannot represent, we must NOT
    // silently record "authorized_not_performed" (that would HIDE a real effect); surface it.
    let unrepresentable = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let unrepresentable_w = unrepresentable.clone();
    // The executor's DECLINE reason must reach the caller (council S29 F2): for a money act an
    // INDETERMINATE outcome ("the charge may have posted; reconcile under this idempotency key")
    // and a plain cap-refusal are indistinguishable without it — and the reason is the ONLY
    // artifact naming the reconciliation key. Captured here, surfaced in the response `reason`
    // and error-logged; putting it on-chain stays the tracked follow-on (S27 F3).
    let decline_reason = Arc::new(std::sync::Mutex::new(None::<String>));
    let decline_reason_w = decline_reason.clone();
    // The act runs ONLY when the gate authorizes (dispatch calls this closure solely on the Acted
    // path), so a denied/revoked/wrong-agent intent never invokes the executor — "no authorization
    // ⇒ no act". The receipt is minted from what the executor REPORTS it performed, never from the
    // declaration, so reconcile compares performed-vs-declared honestly.
    // spawn_blocking (council S29 G-F8/RT-F2 residual): the executor can block for up to the
    // rail's timeout (the pay path joins its rail thread), and the gate itself does durable
    // fsync work — neither belongs on an async worker. The whole authorize→act→reconcile
    // pipeline runs on the blocking pool; the in-flight payment bound caps how many of these
    // can hold blocking threads at once.
    // The ENTIRE gate→act→reconcile→use-record pipeline lives inside ONE blocking task (council
    // S30 G-F6): a client disconnect cancels only the awaiting future, never the started blocking
    // task, so a performed act can no longer lose its G-M2 CapabilityUse projection to a
    // cancellation between the act and the record.
    let dispatch_service = state.standing_service.clone();
    let manager_for_use = state.capability_manager.clone();
    let intent_for_gate = intent.clone();
    let out = tokio::task::spawn_blocking(move || {
        let outcome = dispatch_service.dispatch(&intent_for_gate, move || {
        match executor.execute(&intent_for_exec) {
            crate::intent_executor::IntentExecution::Declined { reason } => {
                if reason.contains("INDETERMINATE") {
                    tracing::error!(
                        capsule = %intent_for_exec.capsule,
                        "indeterminate money outcome on dispatch: {reason}"
                    );
                }
                if let Ok(mut slot) = decline_reason_w.lock() {
                    *slot = Some(reason);
                }
                None
            }
            crate::intent_executor::IntentExecution::Performed {
                capsule,
                method_id,
                input_hash,
                resource,
                action: performed_action,
            } => {
                let performed = match performed_action.to_lowercase().as_str() {
                    "read" => Action::Read,
                    "write" => Action::Write,
                    "execute" => Action::Execute,
                    "delete" => Action::Delete,
                    "message" => Action::Message,
                    "admin" => Action::Admin,
                    _ => {
                        unrepresentable_w.store(true, std::sync::atomic::Ordering::SeqCst);
                        return None;
                    }
                };
                Some(manager.issue_affordance_receipt(
                    &token_str,
                    &capsule,
                    &method_id,
                    &input_hash,
                    &resource,
                    performed,
                ))
            }
        }
        });
        if unrepresentable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "executor reported an unrepresentable action; the act may have occurred but \
                 could not be reconciled — refusing to record it as either performed or \
                 not-performed"
                    .to_string(),
            ));
        }
        use elastos_runtime::capability::{IntentGateOutcome, ReconciliationStatus};
        let (out, acted) = match outcome {
            IntentGateOutcome::BlockedNoCustody(e) => {
                // Custody is mandatory: the declaration could not land on the chain, so nothing
                // ran and nothing further is recordable — surface it, emit nothing else.
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
            IntentGateOutcome::Acted(rec) => {
                // The intent was AUTHORIZED (passed the gate); the reconciliation says whether it
                // was actually PERFORMED. `success` (and the receipt's use) is true ONLY for a
                // `Matched` performance — a `Diverged` act (executor did something else) or
                // `Undelivered` one (nothing performed it) records success=false and says so
                // honestly in the outcome.
                let (label, matched) = match rec.status {
                    ReconciliationStatus::Matched => ("performed", true),
                    ReconciliationStatus::Diverged => ("diverged", false),
                    ReconciliationStatus::Undelivered => ("authorized_not_performed", false),
                };
                (
                    DispatchIntentOutput {
                        outcome: label.to_string(),
                        // The executor's decline reason (S29 F2): distinguishes a cap-refusal
                        // from an INDETERMINATE money outcome, and carries the idempotency key
                        // the operator reconciles with. `None` for a performed act or a
                        // non-executor decline.
                        reason: decline_reason.lock().ok().and_then(|mut s| s.take()),
                        reconciliation: Some(rec),
                    },
                    matched,
                )
            }
        };
        // G-M2: token-keyed projection of the outcome, so export_mandate_receipt_for_capability
        // carries the intent-channel act (or its denial) in the mandate's receipt. Best-effort
        // (like the validate-path use records): a lost emit under-reports in the receipt but the
        // intent-keyed declaration + reconciliation are already durably on the chain.
        if let Ok(token_id) = TokenId::from_hex(intent_for_gate.standing_grant_id.trim()) {
            manager_for_use.audit_log().capability_use(
                &token_id,
                &intent_for_gate.capsule,
                &ResourceId::new(intent_for_gate.resource.clone()),
                action,
                acted,
            );
        }
        Ok(out)
    })
    .await
    .map_err(|e| {
        // JoinError = the blocking task PANICKED (blocking tasks are never cancelled once
        // started). Honest bound (council S30 G-F5): the panic may have struck AFTER the act —
        // the act may have been performed and not reported here. The declaration (and, if it
        // ran, the reconciliation) is on the chain; a money act is in the payment ledger; a
        // retry of the same intent is refused by the replay guard.
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "dispatch task panicked — the act may have been performed and not reported; \
                 consult the audit chain and payment ledger before acting ({e})"
            ),
        )
    })??;
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
        let intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(audit_log.clone(), None),
        );

        CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log,
            )),
            standing_service,
            intent_executor,
            spend_meter: None,
            payment_ledger: None,
        }
    }

    /// G-M4 (Sprint 20): the mandate card SURFACES binding honestly (P12), and the API/CLI path
    /// STILL allows an UNBOUND mandate for the trusted operator (G-M3) — only the web surface
    /// requires binding. A bound mandate's card carries `agent_bound=true` + the key; an unbound
    /// one carries `agent_bound=false`.
    #[tokio::test]
    async fn card_surfaces_agent_binding_and_api_still_allows_unbound() {
        let state = test_state();
        let agent = hex::encode(
            ed25519_dalek::SigningKey::generate(&mut rand::thread_rng())
                .verifying_key()
                .to_bytes(),
        );
        // BOUND via the API — allowed, card shows it.
        let bound = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent.clone()),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .expect("bound issue ok")
        .0;
        // UNBOUND via the API — STILL allowed (G-M3, the trusted operator/CLI path).
        let unbound = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent2".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .expect("unbound issue STILL allowed on the API path (G-M3)")
        .0;

        let cards = mandate_cards(&state.standing_service, &state.capability_manager).await;
        let bc = cards.mandates.iter().find(|c| c.token_id == bound.grant_id).unwrap();
        assert!(bc.agent_bound, "bound mandate card shows agent_bound");
        assert_eq!(bc.agent_pubkey.as_deref(), Some(agent.as_str()));
        let uc = cards.mandates.iter().find(|c| c.token_id == unbound.grant_id).unwrap();
        assert!(!uc.agent_bound, "unbound mandate card shows agent_bound=false");
        assert!(uc.agent_pubkey.is_none());
    }

    /// Council red-team F1 (Sprint 20): a WEAK (small-order / identity) ed25519 key is refused at
    /// issue — it parses as valid 32 bytes but a forged signature validates for it under any
    /// message, so a mandate "bound" to it would be forgeable (effectively unbound wearing a bound
    /// badge). A real key still binds.
    #[tokio::test]
    async fn issue_refuses_a_weak_agent_key() {
        let state = test_state();
        // The ed25519 identity point and all-zeros are small-order (weak).
        for weak in [
            "0100000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let res = issue_standing_grant(
                State(state.clone()),
                Json(IssueStandingGrantInput {
                    capsule: "vm-agent".to_string(),
                    resource: "elastos://mail/send".to_string(),
                    action: "execute".to_string(),
                    methods: vec!["send".to_string()],
                    ttl_secs: Some(3600),
                    agent_pubkey: Some(weak.to_string()),
                    dispatch_limit: None,
                    responsible_entity: None,
                }),
            )
            .await;
            assert!(matches!(res, Err((StatusCode::BAD_REQUEST, _))), "weak key {weak} refused");
        }
        assert!(state.standing_service.list().is_empty(), "no weak-bound mandate was minted");
        // A REAL key still binds.
        let real = hex::encode(
            ed25519_dalek::SigningKey::generate(&mut rand::thread_rng())
                .verifying_key()
                .to_bytes(),
        );
        assert!(issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(real),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .is_ok());
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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

    /// G-M5 over the REAL handlers: a mandate issued through `issue_standing_grant` is still live
    /// after a registry "reboot" (a fresh service over the same snapshot file), and one revoked
    /// through `revoke_standing_grant` STAYS dead — never crash-revived.
    #[tokio::test]
    async fn mandates_survive_restart_over_the_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("standing_grants.json");
        let audit_log = std::sync::Arc::new(elastos_runtime::primitives::audit::AuditLog::new());
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        let standing_service = std::sync::Arc::new(
            capability_manager
                .standing_grant_service_with_persistence(&registry_path)
                .unwrap(),
        );
        let state = CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager: capability_manager.clone(),
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log.clone(),
            )),
            standing_service,
            intent_executor: std::sync::Arc::new(
                crate::intent_executor::MethodRegistryExecutor::production(audit_log, None),
            ),
            spend_meter: None,
            payment_ledger: None,
        };

        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://mail/send".to_string(),
                action: "execute".to_string(),
                methods: vec!["send".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .expect("issue ok")
        .0;

        // "Reboot" #1: a fresh service over the SAME snapshot — the mandate survives, live.
        let rebooted = capability_manager
            .standing_grant_service_with_persistence(&registry_path)
            .unwrap();
        assert!(
            rebooted.is_active(&out.grant_id),
            "an issued mandate survives restart LIVE"
        );

        // Kill it through the real handler, then "reboot" #2: it STAYS dead.
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
        let rebooted = capability_manager
            .standing_grant_service_with_persistence(&registry_path)
            .unwrap();
        assert!(
            !rebooted.is_active(&out.grant_id),
            "a revoked mandate is NEVER crash-revived"
        );
        assert!(
            rebooted.get(&out.grant_id).expect("still queryable").revoked,
            "the reloaded record is honestly marked revoked"
        );
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
        let intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(audit_log.clone(), None),
        );
        CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log,
            )),
            standing_service,
            intent_executor,
            spend_meter: None,
            payment_ledger: None,
        }
    }

    /// Council S32 F3: the DID validator is syntactic anti-garbage — it accepts real DIDs and
    /// rejects malformed ones (empty method, empty id, bad charset, over-long), and it NEVER
    /// authenticates (a valid-shape DID for an entity that never consented still passes — that is
    /// by design; the receipt records a DECLARATION, not consent).
    #[test]
    fn responsible_entity_validation_is_syntactic_and_fail_closed() {
        // Accepted.
        for ok in ["did:web:acme.example", "did:key:z6Mk...", "did:elastos:iabc123", "did:web:acme.example%3A8443"] {
            assert!(validate_responsible_entity(Some(ok)).unwrap().is_some(), "should accept {ok}");
        }
        // None / blank ⇒ Ok(None) (optional field).
        assert!(validate_responsible_entity(None).unwrap().is_none());
        assert!(validate_responsible_entity(Some("   ")).unwrap().is_none());
        // Rejected shapes (council F3): no method, no id, uppercase method, non-DID, bad char, long.
        for bad in [
            "did:", "did:web", "did::x", "did:WEB:x", "notadid", "did:web:a b",
            "did:web:a/path", "did:web:a<script>",
        ] {
            assert!(validate_responsible_entity(Some(bad)).is_err(), "should reject {bad:?}");
        }
        let long = format!("did:web:{}", "a".repeat(300));
        assert!(validate_responsible_entity(Some(&long)).is_err(), "over-long rejected");
    }

    /// Sprint 32 end-to-end: the responsible-entity attribution travels from the grant input,
    /// through the SIGNED CapabilityGrant chain record, into the exported portable receipt — where
    /// it verifies off-box against the pinned signer. The receipt proves WHICH entity the operator
    /// DECLARED accountable (operator-asserted, not the entity's consent — council F2). A malformed
    /// DID fails closed at the mint.
    #[tokio::test]
    async fn responsible_entity_binds_into_the_grant_and_the_portable_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());

        // A malformed responsible entity is refused at the mint (nothing is issued).
        let bad = issue_mandate(
            &state.standing_service,
            &state.capability_manager,
            IssueStandingGrantInput {
                capsule: "vm-ap-agent".into(),
                resource: "elastos://runtime/pay/acme".into(),
                action: "execute".into(),
                methods: vec!["runtime.pay".into()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: Some("acme corp (not a did)".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(bad.0, StatusCode::BAD_REQUEST);
        assert!(state.standing_service.list().is_empty(), "a bad DID mints nothing");

        // A valid entity mints, surfaces on the card, and rides the SIGNED grant record.
        let out = issue_mandate(
            &state.standing_service,
            &state.capability_manager,
            IssueStandingGrantInput {
                capsule: "vm-ap-agent".into(),
                resource: "elastos://runtime/pay/acme".into(),
                action: "execute".into(),
                methods: vec!["runtime.pay".into()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: Some("did:web:acme.example".into()),
            },
        )
        .await
        .unwrap();
        let cards =
            mandate_cards(&state.standing_service, &state.capability_manager).await;
        let card = cards.mandates.iter().find(|c| c.token_id == out.token_id).unwrap();
        assert_eq!(card.responsible_entity.as_deref(), Some("did:web:acme.example"));

        // Export the portable receipt and verify it off-box against the pinned signer — the
        // liability DID is IN the signed grant record, so a verified receipt authenticates it.
        let receipt = mandate_receipt(State(state.clone()), Path(out.token_id.clone()))
            .await
            .unwrap()
            .0;
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        assert!(
            elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer))
                .authenticated,
            "the receipt verifies off-box"
        );
        let grant_record = receipt
            .records
            .iter()
            .find(|r| matches!(
                r.event,
                elastos_runtime::primitives::audit::AuditEvent::CapabilityGrant { .. }
            ))
            .expect("the receipt carries the grant record");
        let event_json = serde_json::to_string(&grant_record.event).unwrap();
        assert!(
            event_json.contains("\"responsible_entity\":\"did:web:acme.example\""),
            "the accountable entity is in the SIGNED, verifiable grant record: {event_json}"
        );
    }

    /// Sprint 24 council F2 ratchet: on the reverse failure — the grant lands on the chain but the
    /// standing-registry persist then FAILS — `issue_mandate` must emit a COMPENSATING revoke, so
    /// the chain reads "granted, then aborted" and never an orphan live grant a verifier would trust
    /// as a never-revoked mandate. Seam: a durable audit (so grant_durable's emit lands) + a
    /// PERSISTENT registry whose atomic write is poisoned (a directory squats on the `.tmp` path, so
    /// `File::create` fails) → issue_from_token errors after the grant is already durable.
    #[tokio::test]
    async fn aborted_issue_emits_a_compensating_revoke_never_an_orphan_grant() {
        use elastos_runtime::primitives::audit::{AuditEvent, AuditLog};
        let dir = tempfile::tempdir().unwrap();
        let audit_log =
            std::sync::Arc::new(AuditLog::with_file(dir.path().join("audit.log")).unwrap());
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        // A PERSISTENT registry whose write will fail: squat a directory on the atomic temp path.
        let registry_path = dir.path().join("standing_grants.json");
        let standing_service = std::sync::Arc::new(
            capability_manager
                .standing_grant_service_with_persistence(&registry_path)
                .unwrap(),
        );
        std::fs::create_dir(registry_path.with_extension("tmp")).unwrap();
        let state = CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager: capability_manager.clone(),
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log.clone(),
            )),
            standing_service,
            intent_executor: std::sync::Arc::new(
                crate::intent_executor::MethodRegistryExecutor::production(audit_log.clone(), None),
            ),
            spend_meter: None,
            payment_ledger: None,
        };

        let res = issue_standing_grant(
            State(state),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["pay.invoke".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await;
        assert!(res.is_err(), "a registry-persist failure aborts the mint (500), no mandate issued");

        // The chain carries the grant AND a compensating revoke — not an orphan live grant.
        let receipt = audit_log
            .export_mandate_receipt()
            .expect("the durable chain exports");
        let grants = receipt
            .records
            .iter()
            .filter(|r| matches!(r.event, AuditEvent::CapabilityGrant { .. }))
            .count();
        let aborted_revokes = receipt
            .records
            .iter()
            .filter(|r| matches!(&r.event, AuditEvent::CapabilityRevoke { reason, .. } if reason.contains("issuance aborted")))
            .count();
        assert_eq!(grants, 1, "the grant did land on the chain (emit-before-issue)");
        assert_eq!(
            aborted_revokes, 1,
            "and a compensating revoke followed it — the orphan grant is neutralized (F2)"
        );
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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

    /// Sign an intent under `sk` naming the mandate; `intent_id` is unique per (method, signer) so
    /// distinct callers don't collide on the replay guard, while the SAME declaration re-submitted
    /// keeps a stable id (a true replay).
    fn signed_intent_with(
        grant_id: &str,
        method: &str,
        sk: &ed25519_dalek::SigningKey,
    ) -> IntentDeclarationV1 {
        let signer_fp = hex::encode(sk.verifying_key().to_bytes());
        IntentDeclarationV1::issue(
            sk,
            sk.verifying_key().to_bytes(),
            &format!("intent-{method}-{}", &signer_fp[..8]),
            "vm-agent",
            method,
            "cafe01",
            "elastos://pay/vendor",
            "write",
            grant_id,
        )
    }

    /// As [`signed_intent_with`] but under a fresh throwaway agent key.
    fn signed_intent(grant_id: &str, method: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        signed_intent_with(grant_id, method, &sk)
    }

    /// A test executor that faithfully performs the declared act (production ships NONE, so a test
    /// that wants a `performed` outcome injects this to stand in for a real, truthful affordance).
    struct FaithfulExecutor;
    impl crate::intent_executor::IntentExecutor for FaithfulExecutor {
        fn execute(&self, intent: &IntentDeclarationV1) -> crate::intent_executor::IntentExecution {
            crate::intent_executor::IntentExecution::Performed {
                capsule: intent.capsule.clone(),
                method_id: intent.method_id.clone(),
                input_hash: intent.input_hash.clone(),
                resource: intent.resource.clone(),
                action: intent.action.clone(),
            }
        }
    }

    /// Sprint 27 end-to-end: an agent, under a BOUND pay-mandate scoped to one payee, spends real
    /// money through the full dispatch pipeline — capped by the spend meter. A within-cap payment
    /// `performs` and the mandate receipt carries it; an over-cap payment reconciles
    /// `authorized_not_performed` and moves NO money (the signed refusal); revoking the mandate
    /// stops payments.
    #[tokio::test]
    async fn agent_pays_a_vendor_under_a_capped_bound_mandate() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        let meter = std::sync::Arc::new(elastos_runtime::primitives::spend::SpendMeter::new());
        let provider = std::sync::Arc::new(crate::intent_executor::MockPaymentProvider::default());
        meter.set_budget("vm-ap-agent", 500).unwrap(); // the operator provisions the agent's weekly cap
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            )
            .with_payments(
                meter.clone(),
                provider.clone(),
                std::sync::Arc::new(crate::payment_ledger::PaymentLedger::new()),
            ),
        );
        let payee_resource = format!("{}acme-vendor", crate::intent_executor::PAY_PREFIX);

        // A BOUND pay-mandate: this agent may pay ACME under an `execute` action.
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-ap-agent".to_string(),
                resource: payee_resource.clone(),
                action: "execute".to_string(),
                methods: vec!["runtime.pay".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let pay = |intent_id: &str, amount: &str| {
            IntentDeclarationV1::issue(
                &agent_sk, agent_sk.verifying_key().to_bytes(), intent_id, "vm-ap-agent",
                "runtime.pay", amount, &payee_resource, "execute", &out.token_id,
            )
        };

        // Within cap → performed; money moves once; the cap is debited.
        let r1 = dispatch_agent_intent(State(state.clone()), Json(pay("inv-1", "200")))
            .await
            .expect("within cap")
            .0;
        assert_eq!(r1.outcome, "performed", "the agent paid the vendor under its mandate");
        assert_eq!(meter.remaining("vm-ap-agent"), 300);
        assert_eq!(*provider.payments.lock().unwrap(), vec![("acme-vendor".to_string(), 200)]);

        // Over the remaining cap (300 left, ask 400) → authorized_not_performed; NO money moves.
        let r2 = dispatch_agent_intent(State(state.clone()), Json(pay("inv-2", "400")))
            .await
            .expect("gate authorized, cap refused")
            .0;
        assert_eq!(
            r2.outcome, "authorized_not_performed",
            "the spend cap physically refused the over-limit payment (signed refusal)"
        );
        // Council S29 F2: the EXECUTOR's decline reason reaches the dispatch response — without
        // it, a cap-refusal and an indeterminate money outcome are indistinguishable to the
        // operator, and nothing surfaces the reconciliation idempotency key.
        assert!(
            r2.reason.as_deref().is_some_and(|r| r.contains("spend cap")),
            "the dispatch response carries the executor's decline reason: {:?}",
            r2.reason
        );
        assert_eq!(meter.remaining("vm-ap-agent"), 300, "the refused payment left the cap untouched");
        assert_eq!(provider.payments.lock().unwrap().len(), 1, "still only the one real payment");

        // Revoke the mandate → the SAME payment is denied outright.
        assert!(
            revoke_standing_grant(
                State(state.clone()),
                Json(RevokeStandingGrantInput { grant_id: out.grant_id.clone() }),
            )
            .await
            .unwrap()
            .0
            .revoked
        );
        let r3 = dispatch_agent_intent(State(state.clone()), Json(pay("inv-3", "50")))
            .await
            .expect("gate denies a revoked mandate")
            .0;
        assert_eq!(r3.outcome, "denied", "a revoked pay-mandate pays nothing");
        assert_eq!(provider.payments.lock().unwrap().len(), 1);

        // The portable receipt carries the real payment (a signed, verifiable record of the act).
        let receipt = mandate_receipt(State(state.clone()), Path(out.token_id.clone()))
            .await
            .unwrap()
            .0;
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        assert!(
            elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer)).authenticated,
            "the pay-mandate's receipt verifies off-box"
        );
    }

    /// Sprint 28 end-to-end: the operator provisions a DURABLE money cap through the shell surface
    /// (`POST /api/spend-budgets`), the change is ATTESTED on the signed audit chain, the agent
    /// spends under it, and a RESTART (reopening the meter from disk) preserves the spend — the cap
    /// never refills. The provisioning surface is fail-closed: no rail ⇒ 503, a non-durable meter ⇒
    /// 503, a malformed capsule ⇒ 400.
    #[tokio::test]
    async fn operator_provisions_a_durable_money_cap_that_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let meter_path = dir.path().join("spend_meter.json");
        let mut state = test_state_with_durable_audit(dir.path());
        let meter = std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(meter_path.clone())
                .unwrap(),
        );
        let provider = std::sync::Arc::new(crate::intent_executor::MockPaymentProvider::default());
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            )
            .with_payments(
                meter.clone(),
                provider.clone(),
                std::sync::Arc::new(crate::payment_ledger::PaymentLedger::new()),
            ),
        );
        state.spend_meter = Some(meter.clone());

        // The OPERATOR provisions the cap through the surface — the same Arc the pay gate debits.
        let set = set_spend_budget(
            State(state.clone()),
            Json(SetSpendBudgetInput {
                capsule: "vm-ap-agent".to_string(),
                limit: 500,
            }),
        )
        .await
        .expect("provisioning a durable meter succeeds")
        .0;
        assert_eq!((set.limit, set.spent, set.remaining), (500, 0, 500));
        // ... and the change is attested on the audit chain (ConfigChange, old → new).
        let attested = state
            .capability_manager
            .audit_log()
            .recent_events(16)
            .into_iter()
            .any(|e| matches!(
                e,
                elastos_runtime::primitives::audit::AuditEvent::ConfigChange {
                    ref setting, ref old_value, ref new_value, ..
                } if setting == "spend_budget:vm-ap-agent"
                    && old_value == "unprovisioned" && new_value == "500"
            ));
        assert!(attested, "the budget change landed on the signed chain");

        // The agent pays 200 under a bound mandate — the operator-set cap is what gets debited.
        let payee_resource = format!("{}acme-vendor", crate::intent_executor::PAY_PREFIX);
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-ap-agent".to_string(),
                resource: payee_resource.clone(),
                action: "execute".to_string(),
                methods: vec!["runtime.pay".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let intent = IntentDeclarationV1::issue(
            &agent_sk, agent_sk.verifying_key().to_bytes(), "inv-1", "vm-ap-agent",
            "runtime.pay", "200", &payee_resource, "execute", &out.token_id,
        );
        let r = dispatch_agent_intent(State(state.clone()), Json(intent))
            .await
            .expect("within cap")
            .0;
        assert_eq!(r.outcome, "performed");

        // RESTART: a REAL restart drops the first opener before the next one starts — the meter's
        // single-opener flock (S29, council F4) enforces exactly that, so release every Arc that
        // holds the first meter (the state's spend_meter and the executor's pay closure) first. A
        // premature reopen while the first is alive is itself refused, ratcheting the flock here.
        assert!(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(meter_path.clone())
                .is_err(),
            "a second opener is refused while the first meter is still alive (single-opener)"
        );
        let mut state = state; // take over the last owner to strip the meter-holding fields
        state.spend_meter = None;
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        drop(meter);
        let reopened = std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(meter_path).unwrap(),
        );
        state.spend_meter = Some(reopened.clone()); // the post-restart state for later assertions
        let mut state2 = state.clone();
        state2.spend_meter = Some(reopened.clone());
        let snap = get_spend_budget(State(state2), Path("vm-ap-agent".to_string()))
            .await
            .expect("the reopened meter projects the surviving balance")
            .0;
        assert_eq!(
            (snap.limit, snap.spent, snap.remaining),
            (500, 200, 300),
            "a restart preserves spent — the money cap never refills"
        );
        assert!(
            matches!(
                reopened.try_debit("vm-ap-agent", 400),
                Err(elastos_runtime::primitives::spend::SpendError::Exhausted { .. })
            ),
            "an over-remaining spend is still refused across the restart"
        );

        // Fail-closed provisioning: no wired rail ⇒ 503 (a budget nothing enforces is refused).
        let mut no_rail = state.clone();
        no_rail.spend_meter = None;
        let e = set_spend_budget(
            State(no_rail),
            Json(SetSpendBudgetInput { capsule: "vm-x".into(), limit: 1 }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::SERVICE_UNAVAILABLE);
        // A NON-durable meter ⇒ 503 (a money cap that refills on restart is refused).
        let mut volatile = state.clone();
        volatile.spend_meter = Some(std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::new(),
        ));
        let e = set_spend_budget(
            State(volatile),
            Json(SetSpendBudgetInput { capsule: "vm-x".into(), limit: 1 }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::SERVICE_UNAVAILABLE);
        // A malformed capsule id ⇒ 400 (same slug bound as the executor's identities).
        let e = set_spend_budget(
            State(state.clone()),
            Json(SetSpendBudgetInput { capsule: "not a slug!".into(), limit: 1 }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
    }

    /// Council S28 F2/F7 ratchet: when the budget applies but its ATTESTATION fails, the rollback
    /// must truly restore the prior state — a FIRST-TIME provision is fully removed (GET stays 404,
    /// no provisioned-at-zero artifact the chain never granted), and a RE-provision restores the
    /// prior limit. The response is a 500 naming the rollback.
    #[tokio::test]
    async fn unattestable_budget_change_is_rolled_back_without_artifact() {
        let dir = tempfile::tempdir().unwrap();
        // An audit log whose emit always FAILS (read-only file handle, mirroring the fail-closed
        // mint ratchet) — apply-then-attest must then roll the budget back.
        let audit_path = dir.path().join("audit-ro.log");
        std::fs::File::create(&audit_path).unwrap();
        let ro = std::fs::OpenOptions::new()
            .read(true)
            .open(&audit_path)
            .unwrap();
        let audit_log = std::sync::Arc::new(
            elastos_runtime::primitives::audit::AuditLog::with_file_handle(ro),
        );
        let store = std::sync::Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics =
            std::sync::Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            std::sync::Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        let standing_service = std::sync::Arc::new(capability_manager.standing_grant_service());
        let meter = std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
            )
            .unwrap(),
        );
        let state = CapabilityState {
            pending_store: std::sync::Arc::new(PendingRequestStore::new(audit_log.clone())),
            capability_manager,
            policy_evaluator: std::sync::Arc::new(PolicyEvaluator::new(
                Box::new(elastos_runtime::capability::evaluator::ShellPassthroughVerifier),
                audit_log,
            )),
            standing_service,
            intent_executor: std::sync::Arc::new(FaithfulExecutor),
            spend_meter: Some(meter.clone()),
            payment_ledger: None,
        };

        // FIRST-TIME provision: applied, unattestable, rolled back — fully unprovisioned again.
        let e = set_spend_budget(
            State(state.clone()),
            Json(SetSpendBudgetInput { capsule: "vm-ap-agent".into(), limit: 500 }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(e.1.contains("rolled back"), "the 500 names the rollback: {}", e.1);
        let g = get_spend_budget(State(state.clone()), Path("vm-ap-agent".to_string()))
            .await
            .unwrap_err();
        assert_eq!(
            g.0,
            StatusCode::NOT_FOUND,
            "no provisioned-at-zero artifact survives a failed first-time provision"
        );
        assert_eq!(
            meter.snapshot("vm-ap-agent"),
            None,
            "the meter holds nothing the chain never granted"
        );

        // RE-provision: seed 100 directly on the meter (in force + persisted), then an
        // unattestable raise to 900 must roll back to exactly 100.
        meter.set_budget("vm-ap-agent", 100).unwrap();
        let e = set_spend_budget(
            State(state.clone()),
            Json(SetSpendBudgetInput { capsule: "vm-ap-agent".into(), limit: 900 }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR);
        let snap = meter.snapshot("vm-ap-agent").unwrap();
        assert_eq!(snap.limit, 100, "the unattestable raise was rolled back to the prior limit");
    }

    /// Sprint 30 end-to-end: an INDETERMINATE payment becomes a RECONCILABLE obligation. The
    /// dispatch keeps the reservation and lands a PENDING ledger entry; the budget surface shows
    /// held-unconfirmed units; the operator resolves it against the rail — "not charged" refunds
    /// exactly once (a second resolution is refused), "charged" confirms with no refund — and the
    /// resolution is attested on the signed chain.
    #[tokio::test]
    async fn indeterminate_payment_is_reconciled_against_the_rail() {
        struct IndeterminateRail;
        impl crate::intent_executor::PaymentProvider for IndeterminateRail {
            fn pay(
                &self,
                _payee: &str,
                _amount: u64,
                _key: &str,
            ) -> Result<String, crate::intent_executor::PayError> {
                Err(crate::intent_executor::PayError::Indeterminate(
                    "timeout after send".to_string(),
                ))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        let meter = std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
            )
            .unwrap(),
        );
        let ledger = std::sync::Arc::new(
            crate::payment_ledger::PaymentLedger::open_durable(
                dir.path().join("payment_ledger.json"),
            )
            .unwrap(),
        );
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            )
            .with_payments(meter.clone(), std::sync::Arc::new(IndeterminateRail), ledger.clone()),
        );
        state.spend_meter = Some(meter.clone());
        state.payment_ledger = Some(ledger.clone());
        meter.set_budget("vm-ap-agent", 500).unwrap();

        let payee_resource = format!("{}acme-vendor", crate::intent_executor::PAY_PREFIX);
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-ap-agent".to_string(),
                resource: payee_resource.clone(),
                action: "execute".to_string(),
                methods: vec!["runtime.pay".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let intent = IntentDeclarationV1::issue(
            &agent_sk, agent_sk.verifying_key().to_bytes(), "inv-1", "vm-ap-agent",
            "runtime.pay", "200", &payee_resource, "execute", &out.token_id,
        );
        let expected_key = format!("flint-{}", intent.signature);
        let r = dispatch_agent_intent(State(state.clone()), Json(intent))
            .await
            .expect("authorized; rail indeterminate")
            .0;
        assert_eq!(r.outcome, "authorized_not_performed");
        assert!(
            r.reason.as_deref().is_some_and(|x| x.contains("INDETERMINATE")
                && x.contains(&expected_key)
                && x.contains("recorded in the payment ledger")),
            "the response reason names the indeterminacy, the key, and the ledger custody: {:?}",
            r.reason
        );
        assert_eq!(meter.remaining("vm-ap-agent"), 300, "the reservation is HELD");

        // The budget surface shows held-unconfirmed units distinct from confirmed spend (RT-F4).
        let snap = get_spend_budget(State(state.clone()), Path("vm-ap-agent".to_string()))
            .await
            .unwrap()
            .0;
        assert_eq!((snap.pending_units, snap.pending_count), (200, 1));

        // The work list carries the obligation with the rail-lookup handle.
        let pending = list_pending_payments(State(state.clone())).await.unwrap().0;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].idempotency_key, expected_key);
        assert_eq!((pending[0].amount, pending[0].payee.as_str()), (200, "acme-vendor"));

        // Resolve against the rail: NOT charged ⇒ refund, exactly once.
        let res = reconcile_payment(
            State(state.clone()),
            Json(ReconcilePaymentInput {
                idempotency_key: expected_key.clone(),
                charged: false,
            }),
        )
        .await
        .expect("first resolution succeeds")
        .0;
        assert!(res.refunded && res.warning.is_none(), "{res:?}");
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            500,
            "the not-charged resolution refunded the reservation"
        );
        let again = reconcile_payment(
            State(state.clone()),
            Json(ReconcilePaymentInput {
                idempotency_key: expected_key.clone(),
                charged: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(again.0, StatusCode::CONFLICT, "a payment resolves EXACTLY once");
        assert_eq!(meter.remaining("vm-ap-agent"), 500, "no double refund");
        assert!(
            ledger.get(&expected_key).unwrap().refund_applied,
            "the applied refund is marked on the entry (the crash-window forensic bit, G-F3)"
        );

        // ... and the resolution is attested on the signed chain.
        let attested = state
            .capability_manager
            .audit_log()
            .recent_events(16)
            .into_iter()
            .any(|e| matches!(
                e,
                elastos_runtime::primitives::audit::AuditEvent::Custom { ref event_type, ref details }
                    if event_type == "payment_reconciled"
                        && details["idempotency_key"] == expected_key.as_str()
                        && details["charged"] == false
                        && details["refunded"] == true
            ));
        assert!(attested, "the reconciliation landed on the signed chain");

        // The CHARGED verdict: spend stands, nothing refunded.
        let agent_sk2 = agent_sk; // same mandate, fresh signed intent
        let intent2 = IntentDeclarationV1::issue(
            &agent_sk2, agent_sk2.verifying_key().to_bytes(), "inv-2", "vm-ap-agent",
            "runtime.pay", "150", &payee_resource, "execute", &out.token_id,
        );
        let key2 = format!("flint-{}", intent2.signature);
        let r2 = dispatch_agent_intent(State(state.clone()), Json(intent2))
            .await
            .unwrap()
            .0;
        assert_eq!(r2.outcome, "authorized_not_performed");
        assert_eq!(meter.remaining("vm-ap-agent"), 350);
        let res2 = reconcile_payment(
            State(state.clone()),
            Json(ReconcilePaymentInput { idempotency_key: key2, charged: true }),
        )
        .await
        .unwrap()
        .0;
        assert!(!res2.refunded);
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            350,
            "a charged resolution confirms the spend — nothing is refunded"
        );
        assert!(
            list_pending_payments(State(state)).await.unwrap().0.is_empty(),
            "no obligations remain"
        );
    }

    /// Council S31 G-F5: reconciling NOT-CHARGED while the meter is POISONED must refuse (503)
    /// BEFORE touching the ledger — the refund cannot apply, and proceeding would burn the
    /// one-shot resolve, permanently losing the refund handle. The entry stays PENDING.
    #[tokio::test]
    async fn poisoned_meter_refuses_not_charged_reconcile_before_burning_the_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let meter = std::sync::Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
            )
            .unwrap(),
        );
        let ledger = std::sync::Arc::new(crate::payment_ledger::PaymentLedger::new());
        meter.set_budget("vm-ap", 500).unwrap();
        meter.try_debit("vm-ap", 200).unwrap();
        assert!(ledger.record(
            "flint-k",
            "vm-ap",
            "acme",
            200,
            crate::payment_ledger::PaymentStatus::Pending,
            ""
        ));
        // Poison via the documented test seam (simulating a post-publish persist failure).
        meter.poison_for_tests();
        assert!(meter.is_poisoned());
        let err = reconcile_payment_core(
            &ledger,
            &meter,
            state.capability_manager.audit_log(),
            ReconcilePaymentInput { idempotency_key: "flint-k".into(), charged: false },
        )
        .unwrap_err();
        assert_eq!(err.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(err.1.contains("poisoned"), "{}", err.1);
        assert_eq!(
            ledger.get("flint-k").unwrap().status,
            crate::payment_ledger::PaymentStatus::Pending,
            "the one-shot resolve was NOT burned — reconcilable after restart"
        );
        // charged=true touches no meter and stays allowed even while poisoned.
        let ok = reconcile_payment_core(
            &ledger,
            &meter,
            state.capability_manager.audit_log(),
            ReconcilePaymentInput { idempotency_key: "flint-k".into(), charged: true },
        )
        .unwrap();
        assert!(!ok.refunded);
    }

    /// Sprint 26 — AGENT-FACING dispatch: an agent acts under a mandate BOUND to its key with NO
    /// operator/shell session. The signed intent + the binding ARE the authorization ("a mandate,
    /// not your keys").
    #[tokio::test]
    async fn agent_dispatch_acts_under_a_bound_mandate_without_operator_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let intent = IntentDeclarationV1::issue(
            &agent_sk, agent_sk.verifying_key().to_bytes(), "agent-1", "vm-agent",
            "runtime.echo", "cafe01", "elastos://pay/vendor", "write", &out.token_id,
        );
        let r = dispatch_agent_intent(State(state.clone()), Json(intent))
            .await
            .expect("the agent's own signed intent performs under its bound mandate")
            .0;
        assert_eq!(r.outcome, "performed", "the agent acted under its mandate — no operator session");
    }

    /// Sprint 26: the agent surface refuses — with a UNIFORM 403 (no existence/binding oracle) — a
    /// wrong-key intent, an UNBOUND mandate (no ambient authority, P3), and an absent grant.
    #[tokio::test]
    async fn agent_dispatch_refuses_wrong_key_unbound_and_absent_uniformly() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let mk = |agent: Option<String>| IssueStandingGrantInput {
            capsule: "vm-agent".to_string(),
            resource: "elastos://pay/vendor".to_string(),
            action: "write".to_string(),
            methods: vec!["runtime.echo".to_string()],
            ttl_secs: Some(3600),
            agent_pubkey: agent,
            dispatch_limit: None,
            responsible_entity: None,
        };
        let bound = issue_standing_grant(State(state.clone()), Json(mk(Some(agent_pub))))
            .await
            .unwrap()
            .0;
        let unbound = issue_standing_grant(State(state.clone()), Json(mk(None)))
            .await
            .unwrap()
            .0;
        let attacker = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let intent = |sk: &ed25519_dalek::SigningKey, grant: &str, id: &str| {
            IntentDeclarationV1::issue(
                sk, sk.verifying_key().to_bytes(), id, "vm-agent", "runtime.echo", "cafe01",
                "elastos://pay/vendor", "write", grant,
            )
        };
        // Wrong key on a bound mandate → 403.
        assert!(
            matches!(
                dispatch_agent_intent(State(state.clone()), Json(intent(&attacker, &bound.token_id, "a1"))).await,
                Err((StatusCode::FORBIDDEN, _))
            ),
            "an intent signed by a key the mandate is NOT bound to is refused"
        );
        // Unbound mandate → 403 (no ambient authority on the agent surface).
        assert!(
            matches!(
                dispatch_agent_intent(State(state.clone()), Json(intent(&attacker, &unbound.token_id, "a2"))).await,
                Err((StatusCode::FORBIDDEN, _))
            ),
            "an unbound mandate cannot be dispatched agent-facing"
        );
        // Absent grant → 403, same shape (no existence oracle).
        let fake = format!("{:064x}", 0xdead_u64);
        assert!(
            matches!(
                dispatch_agent_intent(State(state.clone()), Json(intent(&attacker, &fake, "a3"))).await,
                Err((StatusCode::FORBIDDEN, _))
            ),
            "an absent grant is refused with the same 403"
        );
    }

    /// Sprint 26 — CHARGE-ON-AUTHORIZED (closes the Sprint 21 victim-lockout residual): a flood of
    /// wrong-key intents naming a victim's grant never clears the agent-auth gate, so it never charges
    /// the victim's rate budget — the legit holder's budget is intact.
    #[tokio::test]
    async fn agent_dispatch_charge_on_authorized_no_victim_lockout() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        // A tightly-budgeted BOUND mandate: 1 act/window — a single wrongful charge would lock it out.
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: Some(1),
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let attacker = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        for i in 0..10 {
            let bad = IntentDeclarationV1::issue(
                &attacker, attacker.verifying_key().to_bytes(), &format!("bad-{i}"), "vm-agent",
                "runtime.echo", "cafe01", "elastos://pay/vendor", "write", &out.token_id,
            );
            assert!(matches!(
                dispatch_agent_intent(State(state.clone()), Json(bad)).await,
                Err((StatusCode::FORBIDDEN, _))
            ));
        }
        assert!(
            !state.standing_service.any_dispatch_rate_entries(),
            "no wrong-key attempt charged the mandate's budget (charge-on-authorized)"
        );
        // The legit holder still has its full 1-act budget.
        let good = IntentDeclarationV1::issue(
            &agent_sk, agent_sk.verifying_key().to_bytes(), "good-1", "vm-agent",
            "runtime.echo", "cafe01", "elastos://pay/vendor", "write", &out.token_id,
        );
        assert_eq!(
            dispatch_agent_intent(State(state.clone()), Json(good)).await.expect("still within budget").0.outcome,
            "performed",
            "the victim's budget was never burned by the attacker's flood"
        );
    }

    /// RATE BUDGET (G-M7, Sprint 21): a mandate flooding distinct fresh intents is refused with 429
    /// once it exceeds its per-window budget — BEFORE the replay-guard durable write, so the flood
    /// costs no fsync. The refusal does not burn the intent id (a later, in-budget dispatch of a
    /// fresh id still works).
    #[tokio::test]
    async fn dispatch_over_rate_budget_is_refused_429() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        // Spend the whole per-window budget on distinct fresh intents (all performed).
        let limit = elastos_runtime::capability::MANDATE_DISPATCH_LIMIT;
        for i in 0..limit {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            let intent = IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                &format!("rate-{i}"),
                "vm-agent",
                "runtime.echo",
                "cafe01",
                "elastos://pay/vendor",
                "write",
                &out.token_id,
            );
            let r = dispatch_standing_intent(State(state.clone()), Json(intent))
                .await
                .expect("within budget")
                .0;
            assert_eq!(r.outcome, "performed", "act {i} within budget performs");
        }
        // The next fresh intent under the SAME mandate is rate-refused (429).
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let over = IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "rate-over",
            "vm-agent",
            "runtime.echo",
            "cafe01",
            "elastos://pay/vendor",
            "write",
            &out.token_id,
        );
        let refused = dispatch_standing_intent(State(state.clone()), Json(over)).await;
        assert!(
            matches!(refused, Err((StatusCode::TOO_MANY_REQUESTS, _))),
            "over-budget dispatch is refused 429"
        );
    }

    /// Council Sprint 21 ratchet (guardian F1 / red-team F1+F2): a flood of DISTINCT, never-issued
    /// grant_ids must be turned away BEFORE it can create a rate-map entry or reach the durable
    /// replay-guard write. `standing_grant_id` is attacker-chosen (self-signed), so without the
    /// grant-existence check each fake id would pass the per-grant budget as a "new key" and still
    /// pay the fsync. This reproduces that exact failure: many fake grant_ids are dispatched, each
    /// must come back `denied`/`no_standing_grant`, and the rate map must stay EMPTY (no fake key
    /// ever counted → no durable cost was paid for them).
    #[tokio::test]
    async fn distinct_fake_grant_ids_are_denied_before_any_rate_entry_or_durable_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        for i in 0..200u32 {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            // A syntactically-valid but NEVER-ISSUED grant_id (distinct per request).
            let fake_grant = format!("{:064x}", 0xF1A0_0000_u64 + i as u64);
            let intent = IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                &format!("fake-intent-{i}"),
                "vm-agent",
                "runtime.echo",
                "cafe01",
                "elastos://pay/vendor",
                "write",
                &fake_grant,
            );
            let r = dispatch_standing_intent(State(state.clone()), Json(intent))
                .await
                .expect("a fake grant is a `denied` verdict, not an error")
                .0;
            assert_eq!(r.outcome, "denied", "fake grant {i} is denied");
            assert_eq!(
                r.reason.as_deref(),
                Some("no_standing_grant"),
                "denied with the honest reason, before the gate"
            );
        }
        // The KEY assertion: no fake grant_id ever created a rate entry — the existence check ran
        // before the budget, so the flood cost neither a rate entry nor a durable write.
        assert!(
            !state.standing_service.any_dispatch_rate_entries(),
            "a distinct-fake-grant_id flood created NO rate entries (and paid no durable write)"
        );
    }

    /// Sprint 22 ratchet: rate is a FIRST-CLASS grant property. A mandate minted with its own
    /// `dispatch_limit` is enforced at THAT budget end-to-end (2 acts perform, the 3rd is 429 —
    /// far below the global default of 60), the card surfaces the dial honestly
    /// (`dispatch_limit_custom`), and a zero-rate mint is refused 400 (revoke is the kill switch,
    /// not a budget).
    #[tokio::test]
    async fn mandate_minted_with_its_own_rate_budget_enforces_and_surfaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        // A zero budget is refused at the mint, fail-closed with a clear reason.
        let zero = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: Some(0),
                responsible_entity: None,
            }),
        )
        .await;
        assert!(
            matches!(zero, Err((StatusCode::BAD_REQUEST, _))),
            "a zero-rate mandate is refused at mint"
        );
        // Mint with a TIGHT custom budget: 2 acts per window.
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: Some(2),
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // The card shows the ENFORCED budget and that it was dialed on this mandate.
        let cards = mandate_cards(&state.standing_service, &state.capability_manager).await;
        let card = cards
            .mandates
            .iter()
            .find(|c| c.token_id == out.token_id)
            .expect("minted mandate is on a card");
        assert_eq!(card.dispatch_limit, 2, "the card shows the mandate's own budget");
        assert!(card.dispatch_limit_custom, "and marks it as dialed, not default");
        assert_eq!(
            card.dispatch_window_secs,
            elastos_runtime::capability::MANDATE_DISPATCH_WINDOW_SECS
        );
        // Two acts perform; the third is refused 429 at the mandate's OWN limit.
        let act = |i: u32| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                &format!("dialed-{i}"),
                "vm-agent",
                "runtime.echo",
                "cafe01",
                "elastos://pay/vendor",
                "write",
                &out.token_id,
            )
        };
        for i in 0..2 {
            let r = dispatch_standing_intent(State(state.clone()), Json(act(i)))
                .await
                .expect("within the dialed budget")
                .0;
            assert_eq!(r.outcome, "performed", "act {i} within the dialed budget");
        }
        let refused = dispatch_standing_intent(State(state.clone()), Json(act(2))).await;
        // The 429 body must report THIS mandate's resolved budget (2), never the global default
        // (60) — the message can't contradict what the gate enforced (council guardian F1, P12).
        let (code, body) = refused.expect_err("over the dialed budget");
        assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "the mandate's OWN limit (2) binds");
        assert!(
            body.contains("2 acts per 60s"),
            "the 429 reports the mandate's resolved budget, not the default: {body}"
        );
        assert!(
            !body.contains("60 acts"),
            "the 429 must not misstate the default limit for a dialed mandate: {body}"
        );
    }

    /// The ACT leg closes G-M2: a REAL executor performs a dispatched act (the built-in
    /// `runtime.echo`) and it lands as a `success=true` token-keyed CapabilityUse in the mandate's
    /// receipt; a method OUTSIDE the envelope is denied and receipted `success=false`.
    #[tokio::test]
    async fn dispatch_acts_under_the_mandate_and_the_receipt_carries_the_act() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor); // stand-in for a real affordance
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let resp = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "runtime.echo")),
        )
        .await
        .expect("dispatch ok")
        .0;
        assert_eq!(resp.outcome, "performed", "a registered executor performed it as declared");
        assert!(resp.reconciliation.is_some(), "signed reconciliation returned");

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
        assert_eq!(uses, vec![true, false], "the performed act AND the denied attempt are receipted");
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        let verdict = elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer));
        assert!(verdict.authenticated, "receipt with acts still authenticates: {verdict:?}");
    }

    /// THE FIRST SIDE-EFFECTING AFFORDANCE, full loop (Sprint 16): grant a `message` mandate for
    /// one inbox topic → the agent dispatches a signed intent → the runtime DELIVERS a real
    /// notification into the operator's Inbox store → outcome `performed`, and the mandate's
    /// portable receipt carries the successful use. Revoke, and the SAME act is denied — with
    /// NOTHING further delivered.
    #[tokio::test]
    async fn dispatch_notify_delivers_to_the_inbox_and_the_receipt_carries_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        // The PRODUCTION executor set, wired to a real notify store (same tempdir).
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            ),
        );
        let topic_resource =
            format!("{}agent-status", crate::intent_executor::INBOX_NOTIFY_PREFIX);
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: topic_resource.clone(),
                action: "message".to_string(),
                methods: vec!["runtime.notify".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let intent = IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "intent-notify-1",
            "vm-agent",
            "runtime.notify",
            "cafe01",
            &topic_resource,
            "message",
            &out.token_id,
        );
        let resp = dispatch_standing_intent(State(state.clone()), Json(intent))
            .await
            .expect("dispatch ok")
            .0;
        assert_eq!(resp.outcome, "performed", "the message was really delivered");

        // The side effect is REAL and operator-visible: the Inbox store has the row.
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(summary.unread_count, 1);
        assert!(summary.entries[0].body.contains("vm-agent"));
        assert!(summary.entries[0].body.contains(&out.token_id), "body names the mandate");

        // Kill the mandate → the SAME act is denied, and nothing further lands in the Inbox.
        let rev = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput { grant_id: out.grant_id.clone() }),
        )
        .await
        .unwrap()
        .0;
        assert!(rev.revoked);
        let sk2 = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let denied_intent = IntentDeclarationV1::issue(
            &sk2,
            sk2.verifying_key().to_bytes(),
            "intent-notify-2",
            "vm-agent",
            "runtime.notify",
            "cafe02",
            &topic_resource,
            "message",
            &out.token_id,
        );
        let denied = dispatch_standing_intent(State(state.clone()), Json(denied_intent))
            .await
            .unwrap()
            .0;
        assert_eq!(denied.outcome, "denied", "revoked mandate delivers nothing");
        let after = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(after.entries.len(), 1, "the denied act delivered NOTHING new");

        // The portable receipt carries the delivered act (success=true) AND the denied one (false).
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
        assert_eq!(uses, vec![true, false], "delivery and denial both receipted, honestly");
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        let verdict = elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer));
        assert!(verdict.authenticated, "the receipt verifies off-box: {verdict:?}");
    }

    /// FRESHNESS WINDOW (G-M7): a stale or future-dated declaration is refused at dispatch BEFORE
    /// the replay guard registers it (so it never burns an id) and BEFORE any act — a captured
    /// declaration cannot be replayed indefinitely, and a fresh one under the same mandate still acts.
    #[tokio::test]
    async fn dispatch_rejects_stale_and_future_dated_intents() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let now = elastos_common::SecureTimestamp::now().unix_secs;
        let at = |secs: u64, id: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue_at(
                &sk,
                sk.verifying_key().to_bytes(),
                id,
                "vm-agent",
                "runtime.echo",
                "cafe01",
                "elastos://pay/vendor",
                "write",
                &out.token_id,
                elastos_common::SecureTimestamp::at(secs),
            )
        };
        // Stale: declared well past the age window → 400, refused.
        let stale = dispatch_standing_intent(
            State(state.clone()),
            Json(at(now - elastos_runtime::capability::MAX_INTENT_AGE_SECS - 60, "stale-1")),
        )
        .await;
        assert!(matches!(stale, Err((StatusCode::BAD_REQUEST, _))), "stale intent refused");
        // Future-dated beyond skew → 400, refused.
        let future = dispatch_standing_intent(
            State(state.clone()),
            Json(at(now + elastos_runtime::capability::MAX_CLOCK_SKEW_SECS + 60, "future-1")),
        )
        .await;
        assert!(matches!(future, Err((StatusCode::BAD_REQUEST, _))), "future intent refused");
        // The refused ids never burned: a FRESH intent reusing "stale-1" still acts (proof the
        // freshness check runs BEFORE the replay guard).
        let fresh = dispatch_standing_intent(State(state.clone()), Json(at(now, "stale-1")))
            .await
            .expect("fresh dispatch ok")
            .0;
        assert_eq!(fresh.outcome, "performed", "a fresh intent reusing the id still acts");
    }

    /// THE SECOND SIDE-EFFECTING AFFORDANCE, full loop (Sprint 17): grant a `write` mandate for one
    /// state key → the agent dispatches a signed intent → the runtime WRITES durable, readable-back
    /// agent state → outcome `performed`, the write is observable, and a second intent OVERWRITES
    /// (version deepens). Revoke, and the SAME write is denied — the stored value is unchanged.
    #[tokio::test]
    async fn dispatch_state_put_writes_durable_state_and_revoke_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            ),
        );
        let key_resource = format!("{}cursor", crate::intent_executor::STATE_PUT_PREFIX);
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: key_resource.clone(),
                action: "write".to_string(),
                methods: vec!["runtime.state_put".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let write = |intent_id: &str, value: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                intent_id,
                "vm-agent",
                "runtime.state_put",
                value,
                &key_resource,
                "write",
                &out.token_id,
            )
        };

        let r1 = dispatch_standing_intent(State(state.clone()), Json(write("sp-1", "cafe01")))
            .await
            .expect("dispatch ok")
            .0;
        assert_eq!(r1.outcome, "performed", "the write landed");
        let got = crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
            .unwrap()
            .expect("state readable back");
        assert_eq!(got.value_hash, "cafe01");
        assert_eq!(got.version, 1);

        // A second write OVERWRITES — version deepens, last-write-wins, still attributed.
        let r2 = dispatch_standing_intent(State(state.clone()), Json(write("sp-2", "beef02")))
            .await
            .unwrap()
            .0;
        assert_eq!(r2.outcome, "performed");
        let got = crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
            .unwrap()
            .unwrap();
        assert_eq!(got.value_hash, "beef02");
        assert_eq!(got.version, 2);

        // Kill the mandate → the SAME write is denied, and the stored value is UNCHANGED.
        let rev = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput { grant_id: out.grant_id.clone() }),
        )
        .await
        .unwrap()
        .0;
        assert!(rev.revoked);
        let denied = dispatch_standing_intent(State(state.clone()), Json(write("sp-3", "dead03")))
            .await
            .unwrap()
            .0;
        assert_eq!(denied.outcome, "denied", "revoked mandate writes nothing");
        let after = crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
            .unwrap()
            .unwrap();
        assert_eq!(after.value_hash, "beef02", "denied write left the value UNCHANGED");
        assert_eq!(after.version, 2);

        // The portable receipt carries the two writes (success=true) AND the denial (false).
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
        assert_eq!(uses, vec![true, true, false], "two writes and the denial, honestly receipted");
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        assert!(
            elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer)).authenticated
        );
    }

    /// Sprint 25 end-to-end: an agent WRITES state under a write-mandate, then VERIFIES it back under
    /// a separate BOUND read-mandate (`runtime.state_get`, agent-key required — council F2). It is an
    /// ATTESTED VERIFY read, not a fetch: declaring the CORRECT value reconciles `performed` (an
    /// attested "K = V"); declaring the WRONG value reconciles `diverged` — the agent learns ONE BIT
    /// (its guess was wrong), NOT the actual value (which is never returned — council F1). Revoking
    /// the read-mandate stops reads.
    #[tokio::test]
    async fn dispatch_state_get_reads_back_attested_and_revoke_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(
            crate::intent_executor::MethodRegistryExecutor::production(
                state.capability_manager.audit_log().clone(),
                Some(dir.path().to_path_buf()),
            ),
        );
        let key_resource = format!("{}cursor", crate::intent_executor::STATE_PUT_PREFIX);

        // Seed the store: a write-mandate + a state_put dispatch.
        let w = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: key_resource.clone(),
                action: "write".to_string(),
                methods: vec!["runtime.state_put".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let put = IntentDeclarationV1::issue(
            &sk, sk.verifying_key().to_bytes(), "put-1", "vm-agent",
            "runtime.state_put", "cafe01", &key_resource, "write", &w.token_id,
        );
        assert_eq!(
            dispatch_standing_intent(State(state.clone()), Json(put)).await.unwrap().0.outcome,
            "performed"
        );

        // A separate READ-mandate for state_get on the same key — BOUND to the agent's key (F2:
        // a state_get mandate must bind). The read intents must be signed by THAT key.
        let agent_sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_pub = hex::encode(agent_sk.verifying_key().to_bytes());
        let r = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: key_resource.clone(),
                action: "read".to_string(),
                methods: vec!["runtime.state_get".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_pub),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let read = |intent_id: &str, expected: &str| {
            IntentDeclarationV1::issue(
                &agent_sk, agent_sk.verifying_key().to_bytes(), intent_id, "vm-agent",
                "runtime.state_get", expected, &key_resource, "read", &r.token_id,
            )
        };

        // Correct expected value → performed (attested K=V).
        let hit = dispatch_standing_intent(State(state.clone()), Json(read("get-1", "cafe01")))
            .await
            .unwrap()
            .0;
        assert_eq!(hit.outcome, "performed", "a correct expected-value is an attested read");

        // Wrong expected value → diverged: the agent learns ONE BIT (its guess was wrong). The
        // actual value is NOT returned — the reconciliation carries only the diverged-field name.
        let miss = dispatch_standing_intent(State(state.clone()), Json(read("get-2", "beef99")))
            .await
            .unwrap()
            .0;
        assert_eq!(miss.outcome, "diverged", "a wrong expected-value diverges (one bit, no value leak)");

        // Revoke the read-mandate → reads stop (denied), the write-mandate is untouched.
        assert!(
            revoke_standing_grant(
                State(state.clone()),
                Json(RevokeStandingGrantInput { grant_id: r.grant_id.clone() }),
            )
            .await
            .unwrap()
            .0
            .revoked
        );
        let denied = dispatch_standing_intent(State(state.clone()), Json(read("get-3", "cafe01")))
            .await
            .unwrap()
            .0;
        assert_eq!(denied.outcome, "denied", "a revoked read-mandate reads nothing");

        // The read-mandate's portable receipt carries the attested read + the divergence + denial.
        let receipt = mandate_receipt(State(state.clone()), Path(r.token_id.clone()))
            .await
            .unwrap()
            .0;
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        assert!(
            elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer)).authenticated,
            "the read-mandate's receipt verifies off-box"
        );
    }

    /// Sprint 25 council F2 ratchet: an UNBOUND (agent_pubkey None) mandate authorizing
    /// `runtime.state_get` is REFUSED at the mint — a state-read mandate must bind an agent key, so
    /// the principal's durable state can never be exposed to a mere token-id holder. A bound one, and
    /// an unbound NON-state-get mandate, both still mint.
    #[tokio::test]
    async fn unbound_state_get_mandate_is_refused_at_mint() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let mk = |methods: Vec<String>, agent: Option<String>| IssueStandingGrantInput {
            capsule: "vm-agent".to_string(),
            resource: format!("{}cursor", crate::intent_executor::STATE_PUT_PREFIX),
            action: "read".to_string(),
            methods,
            ttl_secs: Some(3600),
            agent_pubkey: agent,
            dispatch_limit: None,
            responsible_entity: None,
        };
        // Unbound state_get → 400.
        let refused = issue_standing_grant(
            State(state.clone()),
            Json(mk(vec!["runtime.state_get".to_string()], None)),
        )
        .await;
        assert!(
            matches!(refused, Err((StatusCode::BAD_REQUEST, _))),
            "an unbound state_get mandate is refused"
        );
        // Bound state_get → minted.
        let agent = hex::encode(
            ed25519_dalek::SigningKey::generate(&mut rand::thread_rng())
                .verifying_key()
                .to_bytes(),
        );
        assert!(issue_standing_grant(
            State(state.clone()),
            Json(mk(vec!["runtime.state_get".to_string()], Some(agent))),
        )
        .await
        .is_ok());
        // An unbound NON-state-get mandate is unaffected (audit_verify stays capsule-string-only).
        assert!(issue_standing_grant(
            State(state.clone()),
            Json(mk(vec!["runtime.audit_verify".to_string()], None)),
        )
        .await
        .is_ok());
    }

    /// G-M6 closed: an authorized intent whose method has NO executor is `Undelivered`, NOT a
    /// fabricated match — the reconciliation reflects that nothing performed it, and the receipt use
    /// is `success=false`. A custom executor that reports a DIFFERENT field yields `Diverged`.
    #[tokio::test]
    async fn dispatch_reconciles_unperformed_and_diverged_acts_honestly() {
        use crate::intent_executor::{IntentExecution, IntentExecutor};
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["pay.invoke".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        // No executor for "pay.invoke" ⇒ authorized_not_performed (Undelivered).
        let undel = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "pay.invoke")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(undel.outcome, "authorized_not_performed");
        assert_eq!(
            undel.reconciliation.unwrap().status,
            elastos_runtime::capability::ReconciliationStatus::Undelivered
        );

        // A custom executor that PERFORMS but on a different resource ⇒ diverged.
        struct ShiftResourceExecutor;
        impl IntentExecutor for ShiftResourceExecutor {
            fn execute(&self, intent: &IntentDeclarationV1) -> IntentExecution {
                IntentExecution::Performed {
                    capsule: intent.capsule.clone(),
                    method_id: intent.method_id.clone(),
                    input_hash: intent.input_hash.clone(),
                    resource: "elastos://pay/SOMEWHERE-ELSE".to_string(),
                    action: intent.action.clone(),
                }
            }
        }
        state.intent_executor = std::sync::Arc::new(ShiftResourceExecutor);
        let diverged = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent(&out.token_id, "pay.invoke")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(diverged.outcome, "diverged", "executor did something other than declared");

        // Both are receipted success=false — the mandate receipt never claims an act that did not
        // faithfully happen.
        let receipt = mandate_receipt(State(state), Path(out.token_id.clone())).await.unwrap().0;
        use elastos_runtime::primitives::audit::AuditEvent;
        let uses: Vec<bool> = receipt
            .records
            .iter()
            .filter_map(|r| match &r.event {
                AuditEvent::CapabilityUse { success, .. } => Some(*success),
                _ => None,
            })
            .collect();
        assert_eq!(uses, vec![false, false], "neither unperformed nor diverged is a success");
    }

    /// Sprint 9: a STATE-DEPENDENT affordance through the real handler. The SAME agent + method +
    /// declaration reconciles `performed` or `authorized_not_performed` depending on whether the
    /// runtime's audit history actually records access to the mandate's content id — the outcome
    /// tracks real state, not the declaration.
    #[tokio::test]
    async fn dispatch_content_seen_outcome_tracks_real_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        // Record that capsule "vm-agent" really OPENED one content id (state).
        state
            .capability_manager
            .audit_log()
            .content_open("s", "vm-agent", "QmSEEN", "view", "opened", "p", None)
            .unwrap();
        let check = |content_id: &str| {
            format!("{}{content_id}", crate::intent_executor::CONTENT_ACCESS_CHECK_PREFIX)
        };

        let dispatch_seen = |resource: String, intent_id: &'static str| {
            let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            let value = state.clone();
            async move {
                let out = issue_standing_grant(
                    State(value.clone()),
                    Json(IssueStandingGrantInput {
                        capsule: "vm-agent".to_string(),
                        resource: resource.clone(),
                        action: "read".to_string(),
                        methods: vec!["runtime.content_seen".to_string()],
                        ttl_secs: Some(3600),
                        agent_pubkey: Some(hex::encode(agent.verifying_key().to_bytes())),
                        dispatch_limit: None,
                        responsible_entity: None,
                    }),
                )
                .await
                .unwrap()
                .0;
                let intent = IntentDeclarationV1::issue(
                    &agent,
                    agent.verifying_key().to_bytes(),
                    intent_id,
                    "vm-agent",
                    "runtime.content_seen",
                    "",
                    &resource,
                    "read",
                    &out.token_id,
                );
                dispatch_standing_intent(State(value), Json(intent)).await.unwrap().0.outcome
            }
        };

        assert_eq!(
            dispatch_seen(check("QmSEEN"), "seen-1").await,
            "performed",
            "a content id this capsule really opened ⇒ performed"
        );
        assert_eq!(
            dispatch_seen(check("QmNEVER"), "never-1").await,
            "authorized_not_performed",
            "an unopened content id ⇒ authorized but not performed (Undelivered)"
        );
    }

    /// Sprint 7: the FIRST real affordance performs end to end through the PRODUCTION executor (no
    /// test stand-in). `runtime.audit_verify` genuinely re-verifies the runtime's own tamper-evident
    /// chain — a side-effect-free read — so a `read` mandate reconciles `performed` and the receipt
    /// records a real `success=true` use that authenticates.
    #[tokio::test]
    async fn dispatch_really_performs_the_wired_audit_verify_read() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path()); // real production executor, no override
        let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_hex = hex::encode(agent.verifying_key().to_bytes());
        let audit_resource = crate::intent_executor::AUDIT_CHAIN_RESOURCE;
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: audit_resource.to_string(),
                action: "read".to_string(),
                methods: vec!["runtime.audit_verify".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_hex),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let intent = IntentDeclarationV1::issue(
            &agent,
            agent.verifying_key().to_bytes(),
            "intent-audit-1",
            "vm-agent",
            "runtime.audit_verify",
            "", // no arguments — audit_verify consumes none
            audit_resource,
            "read",
            &out.token_id,
        );
        let resp = dispatch_standing_intent(State(state.clone()), Json(intent))
            .await
            .expect("dispatch ok")
            .0;
        assert_eq!(resp.outcome, "performed", "the runtime really re-verified its audit chain");
        assert_eq!(
            resp.reconciliation.unwrap().status,
            elastos_runtime::capability::ReconciliationStatus::Matched
        );

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
        assert_eq!(uses, vec![true], "a real performed read is receipted success=true");
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        let verdict = elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer));
        assert!(verdict.authenticated, "the performed-act receipt authenticates: {verdict:?}");
    }

    /// Guardian honesty: audit_verify reads the audit CHAIN regardless of the declared resource, so
    /// a mandate MIS-SCOPED to an unrelated resource reconciles `Diverged` (the runtime read the
    /// chain, not what was declared), never a misleading `Matched`.
    #[tokio::test]
    async fn audit_verify_under_a_misscoped_mandate_diverges() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(), // NOT the audit chain
                action: "read".to_string(),
                methods: vec!["runtime.audit_verify".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(hex::encode(agent.verifying_key().to_bytes())),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let intent = IntentDeclarationV1::issue(
            &agent,
            agent.verifying_key().to_bytes(),
            "intent-misscope",
            "vm-agent",
            "runtime.audit_verify",
            "",
            "elastos://pay/vendor",
            "read",
            &out.token_id,
        );
        let resp = dispatch_standing_intent(State(state), Json(intent)).await.unwrap().0;
        assert_eq!(resp.outcome, "diverged", "declared read of pay/vendor, actually read the chain");
    }

    /// Mirrors `elastos mandate demo` exactly, through the real handlers + production executor:
    /// grant a read mandate bound to ONE agent key → the agent performs a real audit_verify →
    /// revoke → the SAME agent is now DENIED → the receipt carries the whole story and authenticates.
    #[tokio::test]
    async fn full_demo_sequence_grant_perform_revoke_deny_prove() {
        use elastos_runtime::primitives::audit::AuditEvent;
        let dir = tempfile::tempdir().unwrap();
        let state = test_state_with_durable_audit(dir.path());
        let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let audit_resource = crate::intent_executor::AUDIT_CHAIN_RESOURCE;

        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-demo-agent".to_string(),
                resource: audit_resource.to_string(),
                action: "read".to_string(),
                methods: vec!["runtime.audit_verify".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(hex::encode(agent.verifying_key().to_bytes())),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let act = |intent_id: &'static str| {
            let intent = IntentDeclarationV1::issue(
                &agent,
                agent.verifying_key().to_bytes(),
                intent_id,
                "vm-demo-agent",
                "runtime.audit_verify",
                "",
                audit_resource,
                "read",
                &out.token_id,
            );
            dispatch_standing_intent(State(state.clone()), Json(intent))
        };

        // ACT: performed.
        assert_eq!(act("demo-act-1").await.unwrap().0.outcome, "performed");

        // REVOKE.
        let _ = revoke_standing_grant(
            State(state.clone()),
            Json(RevokeStandingGrantInput { grant_id: out.grant_id.clone() }),
        )
        .await
        .unwrap();

        // ACT AGAIN: the same agent is denied SPECIFICALLY because the mandate was revoked.
        let denied = act("demo-act-2").await.unwrap().0;
        assert_eq!(denied.outcome, "denied");
        assert_eq!(denied.reason.as_deref(), Some("revoked"), "denied for the right reason");

        // RECEIPT: grant → performed use → revoke → denied attempt.
        let receipt = mandate_receipt(State(state.clone()), Path(out.token_id.clone()))
            .await
            .unwrap()
            .0;
        let kinds: Vec<&str> = receipt
            .records
            .iter()
            .map(|r| match &r.event {
                AuditEvent::CapabilityGrant { .. } => "grant",
                AuditEvent::CapabilityUse { success: true, .. } => "use_ok",
                AuditEvent::CapabilityUse { success: false, .. } => "use_denied",
                AuditEvent::CapabilityRevoke { .. } => "revoke",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["grant", "use_ok", "revoke", "use_denied"]);
        let signer = state.capability_manager.audit_log().verifying_key_hex().unwrap();
        assert!(
            elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&signer)).authenticated
        );
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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

    /// Red-team F5: the SAME signed declaration must act at most once — a replayed intent is
    /// refused 409, no second act, no second receipt record.
    #[tokio::test]
    async fn dispatch_refuses_a_replayed_intent() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let intent = signed_intent(&out.token_id, "runtime.echo");
        let first = dispatch_standing_intent(State(state.clone()), Json(intent.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(first.outcome, "performed");
        // Byte-for-byte the same signed declaration again ⇒ 409, refused.
        let replay = dispatch_standing_intent(State(state.clone()), Json(intent)).await;
        assert!(matches!(replay, Err((StatusCode::CONFLICT, _))), "replay must be refused");
        // Exactly ONE use is receipted (the replay never acted).
        let receipt = mandate_receipt(State(state), Path(out.token_id.clone())).await.unwrap().0;
        use elastos_runtime::primitives::audit::AuditEvent;
        let uses = receipt
            .records
            .iter()
            .filter(|r| matches!(r.event, AuditEvent::CapabilityUse { .. }))
            .count();
        assert_eq!(uses, 1, "the replay must not have produced a second use");
    }

    /// Red-team F2: a mandate bound to an agent key authorizes ONLY that key. An intent signed by a
    /// different key — even one internally authentic (verify_self passes) — is denied `wrong_agent`.
    #[tokio::test]
    async fn dispatch_binds_the_authorized_agent_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state_with_durable_audit(dir.path());
        state.intent_executor = std::sync::Arc::new(FaithfulExecutor);
        let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let agent_hex = hex::encode(agent.verifying_key().to_bytes());
        let out = issue_standing_grant(
            State(state.clone()),
            Json(IssueStandingGrantInput {
                capsule: "vm-agent".to_string(),
                resource: "elastos://pay/vendor".to_string(),
                action: "write".to_string(),
                methods: vec!["runtime.echo".to_string()],
                ttl_secs: Some(3600),
                agent_pubkey: Some(agent_hex),
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // The authorized agent acts.
        let ok = dispatch_standing_intent(
            State(state.clone()),
            Json(signed_intent_with(&out.token_id, "runtime.echo", &agent)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(ok.outcome, "performed");
        // A DIFFERENT (internally authentic) key naming the same mandate is denied wrong_agent
        // (the agent check precedes the method check, so even an in-envelope method is refused).
        let impostor = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let denied = dispatch_standing_intent(
            State(state),
            Json(signed_intent_with(&out.token_id, "runtime.echo", &impostor)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(denied.outcome, "denied");
        assert_eq!(denied.reason.as_deref(), Some("wrong_agent"));
    }

    /// Guardian F1: the MASS/rotation kill (epoch advance) must deny dispatch even though it never
    /// touches the individual revocation set — the handler consults epoch validity via the envelope.
    #[tokio::test]
    async fn dispatch_denies_after_an_epoch_advance() {
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // Advance the epoch WITHOUT touching the standing registry (mirrors key rotation, which
        // does not call revoke_all on the service).
        state.capability_manager.revoke_all("key rotation");
        let resp = dispatch_standing_intent(
            State(state),
            Json(signed_intent(&out.token_id, "pay.invoke")),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.outcome, "denied");
        assert_eq!(resp.reason.as_deref(), Some("revoked"), "epoch-dead mandate denies dispatch");
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
            agent_pubkey: None,
            dispatch_limit: None,
            responsible_entity: None,
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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
                agent_pubkey: None,
                dispatch_limit: None,
                responsible_entity: None,
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
