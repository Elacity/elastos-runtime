use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{
    AffordanceApprovalMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleRole, CapsuleType, ReachDescriptorV1,
};
use elastos_runtime::capability::Action;
use serde::{Deserialize, Serialize};

use super::*;

const CAPSULE_CATALOG_SCHEMA: &str = "elastos.capsules.catalog/v1";
const CAPSULE_INTERFACE_REGISTRY_SCHEMA: &str = "elastos.capsules.interfaces/v1";
const CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA: &str = "elastos.capsules.invoke-result/v1";
const CAPSULE_AFFORDANCE_CONSENT_PENDING_SCHEMA: &str =
    "elastos.capsules.affordance-consent-pending/v1";

/// The 202 body returned when a consent-gated affordance is invoked: a pending
/// consent request the user must approve in the shell. Token-INCAPABLE by type —
/// there is no token/output field, so the gateway can never leak a capability
/// here. NOTE (W2): the eventual grant is scoped to `(resource, action,
/// gateway-attach session)`; `principal_id` and the argument `input_hash` are
/// recorded for audit and future per-principal/per-argument binding but are NOT
/// yet enforced by the runtime at grant time, so a shell MUST NOT imply stronger
/// per-invocation or per-principal consent than the token actually carries.
#[derive(Debug, Serialize)]
struct AffordanceConsentPending {
    schema: String,
    status: String,
    request_id: String,
    resource: String,
    action: String,
    risk: AffordanceRisk,
    approval: AffordanceApprovalMode,
    capsule: String,
    interface: String,
    method: String,
    principal_id: String,
}

pub(super) async fn capsule_catalog(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(capsule_catalog_summary(&state.data_dir)).into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(super) async fn capsule_interfaces(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(capsule_interface_registry_summary(&state.data_dir)).into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(super) async fn capsule_interface_invoke(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<CapsuleInterfaceInvokeRequest>,
) -> Response {
    let resolved = match resolve_capsule_affordance(&state.data_dir, &request) {
        Ok(resolved) => resolved,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    };

    let allowed_app = resolved.capsule.clone();
    let (caller_app, context) = match require_home_launch_token_for_any_app_context(
        &state.data_dir,
        &headers,
        &[allowed_app.as_str()],
    ) {
        Ok(value) => value,
        Err(err) => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    };

    let request_id = format!(
        "capsule-affordance:{}:{}:{}",
        resolved.capsule,
        resolved.method.id,
        now_ts()
    );
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            reason: &format!(
                "{} requested {} through {}",
                caller_app, resolved.method.id, resolved.interface_id
            ),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_failed",
            &err.to_string(),
        );
    }

    let gate = enforce_affordance_invocation_policy(&resolved);
    let output = match plan_affordance_dispatch(gate, request.consent_token.is_some()) {
        AffordanceDispatchPlan::DispatchDirect => {
            match dispatch_capsule_affordance(&state, &context, &resolved, &request).await {
                Ok(output) => output,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            }
        }
        AffordanceDispatchPlan::RaiseConsent => {
            // Consent-gated affordance, first call (no token): derive the
            // (resource, action) scope, raise a consent request through the
            // runtime, return 202 + request_id. Never a token and never dispatch;
            // every path in this arm diverges (returns).
            let (resource, action, risk) = match affordance_consent_descriptor(&resolved) {
                Ok(value) => value,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.consent_failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            };
            let input_hash = elastos_common::canonical_input_hash(&request.input);
            let action_str = action.to_string();
            // Bind consent to the CANONICAL capsule identity ("vm-{name}", the
            // G-ID convention every gate validates against), not the bare manifest
            // name. The eventual single-use token (W2 step 6) is minted at this
            // identity, so it validates at the same domain the carrier/HTTP gates
            // use — affordance tokens live in the ONE identity domain, not a second
            // one (the anti-pattern G-ID already eliminated).
            let bound_capsule = format!("vm-{}", resolved.capsule);
            let consent_request_id = match request_affordance_consent(
                &state.data_dir,
                &resource,
                &action_str,
                &bound_capsule,
                &context.principal_id,
                &resolved.method.id,
                &input_hash,
            )
            .await
            {
                Ok(id) => id,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.consent_failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            };
            let _ = append_provider_effect_audit(
                &state.data_dir,
                ProviderEffectAuditInput {
                    capsule_id: &resolved.capsule,
                    event_type: "capsule.affordance.consent_requested",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: &request_id,
                    result: "approval_pending",
                    reason: &format!(
                        "{} requires user approval for {} ({}, {})",
                        resolved.capsule, resolved.method.id, resource, action_str
                    ),
                },
            );
            return (
                StatusCode::ACCEPTED,
                Json(AffordanceConsentPending {
                    schema: CAPSULE_AFFORDANCE_CONSENT_PENDING_SCHEMA.to_string(),
                    status: "approval_pending".to_string(),
                    request_id: consent_request_id,
                    resource,
                    action: action_str,
                    risk,
                    approval: resolved.method.approval.clone(),
                    capsule: resolved.capsule.clone(),
                    interface: resolved.interface_id.clone(),
                    method: resolved.method.id.clone(),
                    principal_id: context.principal_id.clone(),
                }),
            )
                .into_response();
        }
        AffordanceDispatchPlan::RedeemThenDispatch => {
            // Consent-gated retry: a consent token was presented. Derive the same
            // (resource, action) scope, redeem the token via validate-and-consume
            // (forwarding the caller's own authorization so the runtime checks it
            // as the bound capsule), and ONLY THEN dispatch — gated by the witness.
            let (resource, action, _risk) = match affordance_consent_descriptor(&resolved) {
                Ok(value) => value,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.consent_failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            };
            let action_str = action.to_string();
            let app_authorization = headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let consent_token = request.consent_token.as_deref().unwrap_or_default();
            let grant = match redeem_affordance_grant(
                &state.data_dir,
                app_authorization,
                consent_token,
                &resolved,
                &resource,
                &action_str,
                &request.input,
            )
            .await
            {
                Ok(grant) => grant,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.consent_redeem_failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            };
            match dispatch_consented_affordance(&state, &context, &resolved, &request, grant).await
            {
                Ok(output) => output,
                Err((status, code, message)) => {
                    let _ = append_provider_effect_audit(
                        &state.data_dir,
                        ProviderEffectAuditInput {
                            capsule_id: &resolved.capsule,
                            event_type: "capsule.affordance.failed",
                            principal_id: &context.principal_id,
                            session_id: &context.session_id,
                            request_id: &request_id,
                            result: "failed",
                            reason: &message,
                        },
                    );
                    return capsule_invoke_error(&resolved, status, code, &message);
                }
            }
        }
    };

    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "completed",
            reason: &format!("Runtime completed {}", resolved.method.id),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_failed",
            &err.to_string(),
        );
    }

    Json(CapsuleInterfaceInvokeResponse {
        schema: CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA.to_string(),
        status: "ok".to_string(),
        capsule: resolved.capsule,
        interface: resolved.interface_id,
        method: resolved.method.id,
        request_id,
        output,
    })
    .into_response()
}

pub(super) fn require_capsule_catalog_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_context(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID, MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    )
}

pub(super) fn capsule_catalog_summary(data_dir: &std::path::Path) -> CapsuleCatalogResponse {
    let launch_targets = home_targets(data_dir)
        .into_iter()
        .map(|target| (target.target.clone(), target))
        .collect::<BTreeMap<_, _>>();
    let components = load_capsule_components(data_dir);
    let active_components = crate::api::capsule_inventory::active_component_names(data_dir);
    let installed_names = installed_capsule_names(data_dir, active_components.as_ref());

    let mut capsules = crate::api::capsule_inventory::list_capsule_manifests(data_dir)
        .into_iter()
        .map(|manifest| {
            catalog_capsule_summary(manifest, &launch_targets, &components, &installed_names)
        })
        .collect::<Vec<_>>();
    capsules.sort_by(|left, right| {
        capsule_category_order(&left.category)
            .cmp(&capsule_category_order(&right.category))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut counts = CapsuleCatalogCounts {
        total: capsules.len(),
        ..Default::default()
    };
    for capsule in &capsules {
        match capsule.role.as_str() {
            "app" => counts.apps += 1,
            "viewer" => counts.viewers += 1,
            "provider" => counts.providers += 1,
            "content" => counts.content += 1,
            "shell" => counts.shell += 1,
            _ => {}
        }
        counts.interfaces += capsule.interfaces.len();
        counts.methods += capsule
            .interfaces
            .iter()
            .map(|interface| interface.methods.len())
            .sum::<usize>();
        if capsule.launchable {
            counts.launchable += 1;
        }
        if capsule.installed {
            counts.installed += 1;
        }
    }

    CapsuleCatalogResponse {
        schema: CAPSULE_CATALOG_SCHEMA.to_string(),
        counts,
        capsules,
        policy: CapsuleCatalogPolicy {
            install_state: "signed-app-install-pending".to_string(),
            install_note: "Marketplace can open installed apps now. Installing new apps will require verified app signatures, receipts, and provider policy.".to_string(),
            payment_state: "provider-rail-required".to_string(),
            payment_note: "Paid apps and services must use wallet/payment provider receipts, not embedded payment SDKs.".to_string(),
            drm_state: "provider-rail-required".to_string(),
            drm_note: "Protected apps and content must use rights, key, and decrypt providers for dDRM enforcement.".to_string(),
        },
    }
}

pub(super) fn capsule_interface_registry_summary(
    data_dir: &std::path::Path,
) -> CapsuleInterfaceRegistryResponse {
    let catalog = capsule_catalog_summary(data_dir);
    let mut interfaces = Vec::new();
    for capsule in catalog.capsules {
        for interface in capsule.interfaces {
            interfaces.push(CapsuleInterfaceSummary {
                capsule: capsule.name.clone(),
                capsule_version: capsule.version.clone(),
                title: capsule.title.clone(),
                role: capsule.role.clone(),
                capsule_type: capsule.capsule_type.clone(),
                cid: capsule.cid.clone(),
                trust_state: capsule.trust_state.clone(),
                interface,
            });
        }
    }

    let mut counts = CapsuleInterfaceRegistryCounts {
        capsules: interfaces
            .iter()
            .map(|interface| interface.capsule.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        ..Default::default()
    };
    counts.interfaces = interfaces.len();
    counts.methods = interfaces
        .iter()
        .map(|summary| summary.interface.methods.len())
        .sum();

    CapsuleInterfaceRegistryResponse {
        schema: CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
        counts,
        interfaces,
        policy: CapsuleInterfaceRegistryPolicy {
            descriptor_state: "manifest-declared".to_string(),
            descriptor_note: "Interfaces describe callable affordances declared by installed apps and providers. They are not authority grants; Runtime approval, expiry, and audit still govern invocation.".to_string(),
            invocation_state: "runtime-gated".to_string(),
            invocation_note: "0.4.0 executes low-risk Runtime Marketplace bindings and fails closed for high-risk or user-approval methods until approval/provider binding is complete.".to_string(),
        },
    }
}

fn resolve_capsule_affordance(
    data_dir: &std::path::Path,
    request: &CapsuleInterfaceInvokeRequest,
) -> anyhow::Result<ResolvedCapsuleAffordance> {
    let capsule_name = request.capsule.trim();
    let interface_id = request.interface.trim();
    let method_id = request.method.trim();
    if capsule_name.is_empty() || interface_id.is_empty() || method_id.is_empty() {
        anyhow::bail!("capsule, interface, and method are required");
    }

    let catalog = capsule_catalog_summary(data_dir);
    let capsule = catalog
        .capsules
        .iter()
        .find(|candidate| candidate.name == capsule_name)
        .ok_or_else(|| anyhow::anyhow!("capsule not found: {}", capsule_name))?;
    let interface = capsule
        .interfaces
        .iter()
        .find(|candidate| candidate.id == interface_id)
        .ok_or_else(|| anyhow::anyhow!("interface not declared: {}", interface_id))?;
    let method = interface
        .methods
        .iter()
        .find(|candidate| candidate.id == method_id)
        .ok_or_else(|| anyhow::anyhow!("method not declared: {}", method_id))?;

    Ok(ResolvedCapsuleAffordance {
        capsule: capsule.name.clone(),
        interface_id: interface.id.clone(),
        method: method.clone(),
    })
}

/// Whether an affordance invocation dispatches directly or must first pass a
/// user-consent round-trip. The flat 403 is gone: consent-gated methods now raise
/// a consent request instead of dead-rejecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvocationGate {
    /// Low-risk, runtime-policy method — dispatch directly.
    Direct,
    /// `AffordanceApprovalMode::User` or a high-risk class — require user consent.
    Consent,
}

fn enforce_affordance_invocation_policy(resolved: &ResolvedCapsuleAffordance) -> InvocationGate {
    if resolved.method.approval == AffordanceApprovalMode::User {
        return InvocationGate::Consent;
    }
    if matches!(
        resolved.method.risk,
        AffordanceRisk::Payment
            | AffordanceRisk::Rights
            | AffordanceRisk::Actuator
            | AffordanceRisk::Privileged
    ) {
        return InvocationGate::Consent;
    }
    InvocationGate::Direct
}

/// What to do with an affordance invocation, given the policy gate and whether
/// the caller presented a consent token. Pure and total so the routing decision
/// is unit-testable without any I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AffordanceDispatchPlan {
    /// Low-risk: dispatch directly, no consent needed.
    DispatchDirect,
    /// Consent-gated, no token yet: raise a consent request (HTTP 202).
    RaiseConsent,
    /// Consent-gated and a token was presented: redeem it, then dispatch.
    RedeemThenDispatch,
}

/// Decide the dispatch plan. A consent-gated method dispatches ONLY when a
/// consent token is presented (which is then redeemed); otherwise it raises a
/// consent request. A low-risk method always dispatches directly.
fn plan_affordance_dispatch(
    gate: InvocationGate,
    has_consent_token: bool,
) -> AffordanceDispatchPlan {
    match (gate, has_consent_token) {
        (InvocationGate::Direct, _) => AffordanceDispatchPlan::DispatchDirect,
        (InvocationGate::Consent, false) => AffordanceDispatchPlan::RaiseConsent,
        (InvocationGate::Consent, true) => AffordanceDispatchPlan::RedeemThenDispatch,
    }
}

/// Unforgeable proof that an affordance-consent token was redeemed via
/// validate-and-consume (W2 step 8). The field is private to this module, so a
/// value can ONLY be produced by [`redeem_affordance_grant`] on a SUCCESSFUL
/// redemption — there is no other constructor. Requiring one by value in
/// [`dispatch_consented_affordance`] makes "no consent-gated dispatch without a
/// live, consumed grant" a property the COMPILER enforces, not a convention.
struct ValidatedAffordanceGrant {
    /// The affordance method the grant authorises (re-asserted at dispatch).
    method_id: String,
}

/// Redeem a consent token against the runtime's `validate-and-consume`, returning
/// the witness ONLY on a successful single-use consume. The APP's own
/// authorization is forwarded verbatim, so the runtime authenticates the
/// redemption as the calling capsule (`vm-{name}`) — i.e. the identity the token
/// is bound to — rather than as the shell. Any non-success fails closed: no
/// witness, therefore no dispatch.
async fn redeem_affordance_grant(
    data_dir: &std::path::Path,
    app_authorization: &str,
    consent_token: &str,
    resolved: &ResolvedCapsuleAffordance,
    resource: &str,
    action: &str,
    input: &serde_json::Value,
) -> Result<ValidatedAffordanceGrant, (StatusCode, &'static str, String)> {
    if app_authorization.trim().is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            "redeem_no_authorization",
            "no caller authorization to redeem the consent grant".to_string(),
        ));
    }
    let coords = load_live_runtime_coords(data_dir).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "runtime_unavailable",
        "local runtime is not running".to_string(),
    ))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "redeem_client_failed",
                err.to_string(),
            )
        })?;
    let response = client
        .post(format!(
            "{}/api/capability/validate-and-consume",
            coords.api_url
        ))
        .header(AUTHORIZATION, app_authorization)
        .json(&serde_json::json!({
            "token": consent_token,
            "method_id": resolved.method.id,
            "resource": resource,
            "action": action,
            "input": input,
        }))
        .send()
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                "redeem_post_failed",
                err.to_string(),
            )
        })?;

    if !response.status().is_success() {
        // Keep the runtime's refusal in the gateway audit only; never forward raw
        // runtime prose (avoids leaking internal resource topology).
        let runtime_status = response.status();
        let _ = response.text().await;
        return Err((
            StatusCode::FORBIDDEN,
            "consent_redeem_rejected",
            format!("runtime refused the redemption ({runtime_status})"),
        ));
    }

    let body: serde_json::Value = response.json().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            "redeem_parse_failed",
            err.to_string(),
        )
    })?;
    match body.get("status").and_then(|s| s.as_str()) {
        Some("consumed") => Ok(ValidatedAffordanceGrant {
            method_id: resolved.method.id.clone(),
        }),
        other => Err((
            StatusCode::BAD_GATEWAY,
            "redeem_unexpected_status",
            format!("unexpected redeem status: {}", other.unwrap_or("<missing>")),
        )),
    }
}

/// Dispatch a CONSENT-GATED affordance. Requires a [`ValidatedAffordanceGrant`]
/// BY VALUE: it is a compile error to reach here without a redeemed grant. Also
/// re-asserts the grant authorises THIS method (defence in depth) and records an
/// audit entry linking the dispatch to the consumed grant.
async fn dispatch_consented_affordance(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    resolved: &ResolvedCapsuleAffordance,
    request: &CapsuleInterfaceInvokeRequest,
    grant: ValidatedAffordanceGrant,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    if grant.method_id != resolved.method.id {
        return Err((
            StatusCode::FORBIDDEN,
            "grant_method_mismatch",
            "redeemed grant does not authorise this method".to_string(),
        ));
    }
    let _ = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.consented_dispatch",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &format!(
                "affordance-consent:{}:{}",
                resolved.capsule, grant.method_id
            ),
            result: "consented",
            reason: &format!(
                "dispatching {} under a redeemed consent grant",
                grant.method_id
            ),
        },
    );
    // Defense-in-depth on the human-consent-gated path: charge the SAME shared budget the carrier
    // act paths use BEFORE the dispatch, fail-closed.
    charge_affordance_spend(state, &resolved.capsule)?;
    dispatch_capsule_affordance(state, context, resolved, request).await
}

/// Debit the shared act-spend budget for one affordance dispatch, keyed on the canonical `vm-{name}`
/// (the SAME key the carrier paths debit — so the budget is unified, not per-plane). Fail-closed:
/// returns a 429 `budget_exhausted` when the budget is gone. Unmetered gateway (`None`) ⇒ no charge.
///
/// A flat 1 unit/act: the affordance result carries no provider-reported cost, and unlike the
/// carrier path there is no `DidNotAct` taxonomy here, so the charge stands on any post-debit
/// outcome (conservative — errs toward charging, never over-refunds a human-approved act).
fn charge_affordance_spend(
    state: &GatewayState,
    capsule: &str,
) -> Result<(), (StatusCode, &'static str, String)> {
    if let Some(policy) = &state.spend_policy {
        let key = format!("vm-{capsule}");
        policy.meter.ensure_budget(&key, policy.default_budget);
        if let Err(e) = policy.meter.try_debit(&key, 1) {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "budget_exhausted",
                format!("spend budget exhausted for {key}: {e}"),
            ));
        }
    }
    Ok(())
}

/// Narrowest [`Action`] implied by a declared [`AffordanceRisk`] class. Total and
/// exhaustive so the first real high-risk affordance forces a compile-time review
/// of this table. NOTE: risk alone NEVER yields `Admin` — because the eventual
/// grant is scoped purely to this `(resource, action)` today, an `Admin` token on
/// a wildcard resource would be an over-grant; `Admin` is reachable only via an
/// authoritative admin/manage operation (see [`affordance_consent_descriptor`]).
fn action_from_risk(risk: &AffordanceRisk) -> Action {
    match risk {
        AffordanceRisk::Read => Action::Read,
        AffordanceRisk::Write => Action::Write,
        AffordanceRisk::Launch => Action::Execute,
        AffordanceRisk::Payment => Action::Execute,
        AffordanceRisk::Rights => Action::Execute,
        AffordanceRisk::Actuator => Action::Write,
        AffordanceRisk::Privileged => Action::Execute,
    }
}

/// Map a declared method `operation` to an [`Action`] by substring, first match
/// wins. `None` means the operation maps to no known action, which the caller
/// treats as a fail-closed `operation_unmapped` (never a default).
fn action_from_operation(operation: &str) -> Option<Action> {
    let op = operation.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| op.contains(n));
    if has(&["admin", "manage"]) {
        Some(Action::Admin)
    } else if has(&["delete", "remove"]) {
        Some(Action::Delete)
    } else if has(&["execute", "invoke", "launch", "call", "run"]) {
        Some(Action::Execute)
    } else if has(&["send", "post", "message"]) {
        Some(Action::Message)
    } else if has(&["write", "create", "update", "set", "put"]) {
        Some(Action::Write)
    } else if has(&["read", "get", "list", "query"]) {
        Some(Action::Read)
    } else {
        None
    }
}

/// Privilege rank used to take the stronger of two actions. Higher = more power.
fn action_rank(action: &Action) -> u8 {
    match action {
        Action::Read => 0,
        Action::Message => 1,
        Action::Write => 2,
        Action::Delete => 3,
        Action::Execute => 4,
        Action::Admin => 5,
    }
}

/// The stronger (higher-privilege) of two actions.
fn max_privilege(a: Action, b: Action) -> Action {
    if action_rank(&a) >= action_rank(&b) {
        a
    } else {
        b
    }
}

/// Derive the `(resource, action, risk)` a consent-gated affordance must request,
/// fail-closed. `resource` comes from the method's DECLARED resource (no default);
/// `action` is the narrowest action implied by the operation and risk. `Admin` is
/// reachable ONLY when the operation is authoritatively admin/manage — risk alone
/// is capped at `Execute`, because today this `(resource, action)` pair is the
/// entire scope of the token the user later approves.
fn affordance_consent_descriptor(
    resolved: &ResolvedCapsuleAffordance,
) -> Result<(String, Action, AffordanceRisk), (StatusCode, &'static str, String)> {
    let resource = resolved
        .method
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or((
            StatusCode::BAD_REQUEST,
            "descriptor_resource_missing",
            "consent-gated affordance declares no resource; cannot scope consent".to_string(),
        ))?
        .to_string();

    let risk_action = action_from_risk(&resolved.method.risk);
    let action = match resolved.method.operation.as_deref() {
        Some(operation) => {
            let op_action = action_from_operation(operation).ok_or((
                StatusCode::BAD_REQUEST,
                "operation_unmapped",
                format!("method operation '{operation}' does not map to a known action"),
            ))?;
            let operation_is_admin = matches!(op_action, Action::Admin);
            let combined = max_privilege(risk_action, op_action);
            // Operation is the only authority that may mint Admin; risk may only
            // tighten within non-Admin.
            if matches!(combined, Action::Admin) && !operation_is_admin {
                Action::Execute
            } else {
                combined
            }
        }
        None => match risk_action {
            Action::Admin => Action::Execute,
            other => other,
        },
    };

    Ok((resource, action, resolved.method.risk.clone()))
}

/// Raise a user-consent request through the runtime for a consent-gated
/// affordance, returning the runtime's `request_id`. NEVER returns or holds a
/// token: the gateway is a thin adapter; the runtime (key holder) owns minting.
/// Mirrors the inbox approve/deny seam (load coords -> attach shell token ->
/// Bearer POST). All four binding fields are sent together (the runtime rejects a
/// partial set 400); the status is default-deny (only "pending" yields an id).
async fn request_affordance_consent(
    data_dir: &std::path::Path,
    resource: &str,
    action: &str,
    capsule: &str,
    principal_id: &str,
    method_id: &str,
    input_hash: &str,
) -> Result<String, (StatusCode, &'static str, String)> {
    // The runtime stores binding fields verbatim with no emptiness check; an empty
    // field would silently weaken the binding, so reject before the POST.
    for (label, value) in [
        ("capsule", capsule),
        ("principal_id", principal_id),
        ("method_id", method_id),
        ("input_hash", input_hash),
    ] {
        if value.trim().is_empty() {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "binding_field_empty",
                format!("consent binding field '{label}' is empty"),
            ));
        }
    }

    let coords = load_live_runtime_coords(data_dir).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "runtime_unavailable",
        "local runtime is not running".to_string(),
    ))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "consent_client_failed",
                err.to_string(),
            )
        })?;
    let shell_token = home_attach_shell(&client, &coords.api_url, &coords.attach_secret)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "consent_attach_failed",
                err.to_string(),
            )
        })?;
    let response = client
        .post(format!("{}/api/capability/request", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {shell_token}"))
        .json(&serde_json::json!({
            "resource": resource,
            "action": action,
            "capsule": capsule,
            "principal_id": principal_id,
            "method_id": method_id,
            "input_hash": input_hash,
        }))
        .send()
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                "consent_post_failed",
                err.to_string(),
            )
        })?;

    if !response.status().is_success() {
        // Keep the runtime refusal in the gateway audit only; never forward raw
        // runtime prose to the caller (avoids leaking internal resource topology).
        let runtime_status = response.status();
        let runtime_body = response.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            "consent_rejected",
            format!("runtime rejected consent request ({runtime_status}): {runtime_body}"),
        ));
    }

    let body: serde_json::Value = response.json().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            "consent_parse_failed",
            err.to_string(),
        )
    })?;

    // Default-deny: only an explicit "pending" status yields a request_id; every
    // other status (including a token-bearing "granted") fails closed.
    match body.get("status").and_then(|s| s.as_str()) {
        Some("pending") => body
            .get("request_id")
            .and_then(|r| r.as_str())
            .map(str::to_string)
            .ok_or((
                StatusCode::BAD_GATEWAY,
                "consent_missing_request_id",
                "runtime returned pending without a request_id".to_string(),
            )),
        Some("granted") => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected_grant",
            "runtime auto-granted a consent-gated affordance".to_string(),
        )),
        Some("denied") => Err((
            StatusCode::FORBIDDEN,
            "consent_denied",
            "runtime denied the consent request".to_string(),
        )),
        other => Err((
            StatusCode::BAD_GATEWAY,
            "unexpected_status",
            format!(
                "unexpected consent status: {}",
                other.unwrap_or("<missing>")
            ),
        )),
    }
}

async fn dispatch_capsule_affordance(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    resolved: &ResolvedCapsuleAffordance,
    request: &CapsuleInterfaceInvokeRequest,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    let resource = resolved.method.resource.as_deref().unwrap_or_default();
    let operation = resolved.method.operation.as_deref().unwrap_or_default();
    match (resource, operation) {
        ("elastos://capsules/*", "list") => {
            serde_json::to_value(capsule_catalog_summary(&state.data_dir))
                .map(|catalog| serde_json::json!({ "catalog": catalog }))
                .map_err(|err| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "serialization_failed",
                        err.to_string(),
                    )
                })
        }
        ("elastos://capsules/*", "launch") => {
            dispatch_capsule_launch_affordance(state, context, request).await
        }
        _ => Err((
            StatusCode::NOT_IMPLEMENTED,
            "affordance_not_bound",
            format!(
                "{} is declared but not yet bound to a Runtime/provider handler",
                resolved.method.id
            ),
        )),
    }
}

async fn dispatch_capsule_launch_affordance(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request: &CapsuleInterfaceInvokeRequest,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    let target = request
        .input
        .get("target")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if target.is_empty() || target == HOME_CAPSULE_ID {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "invalid Home target".to_string(),
        ));
    }

    let catalog = capsule_catalog_summary(&state.data_dir);
    let catalog_target = catalog
        .capsules
        .iter()
        .find(|capsule| capsule.name == target || capsule.launch_target.as_deref() == Some(target))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "target_not_found",
                "launch target not found".to_string(),
            )
        })?;
    if !catalog_target.launchable {
        return Err((
            StatusCode::FORBIDDEN,
            "target_not_launchable",
            "target is not installed and launchable".to_string(),
        ));
    }

    let target_name = catalog_target
        .launch_target
        .as_deref()
        .unwrap_or(catalog_target.name.as_str());
    let target_summary = home_launch_target(&state.data_dir, target_name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "target_not_found",
            "Home launch target not found".to_string(),
        )
    })?;
    let launch =
        launch_runtime_backed_home_target(&state.data_dir, target_summary.target.as_str(), context)
            .await;
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
        &BTreeMap::new(),
        context,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "launch_token_failed",
            err.to_string(),
        )
    })?;

    Ok(serde_json::json!({
        "target": target_summary.target,
        "title": target_summary.title,
        "route": route,
        "attach_kind": target_summary.attach_kind,
        "role": target_summary.role,
        "target_kind": target_summary.target_kind,
        "launch_status": launch.as_ref().map(|summary| summary.status.clone()),
        "launch_detail": launch.as_ref().and_then(|summary| summary.detail.clone()),
        "capsule_id": launch.and_then(|summary| summary.capsule_id),
    }))
}

fn capsule_invoke_error(
    resolved: &ResolvedCapsuleAffordance,
    status: StatusCode,
    code: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "schema": CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA,
            "status": "error",
            "code": code,
            "message": message,
            "capsule": resolved.capsule,
            "interface": resolved.interface_id,
            "method": resolved.method.id,
        })),
    )
        .into_response()
}

fn catalog_capsule_summary(
    manifest: CapsuleManifest,
    launch_targets: &BTreeMap<String, HomeTargetSummary>,
    components: &BTreeMap<String, CapsuleComponentInfo>,
    installed_names: &BTreeSet<String>,
) -> CapsuleSummary {
    let target = launch_targets.get(&manifest.name);
    let component = components.get(&manifest.name);
    let installed = installed_names.contains(&manifest.name);
    let name = manifest.name.clone();
    let role = manifest.role.clone();
    let capsule_type = manifest.capsule_type.clone();
    let category = capsule_category(&role);

    // W0b: stamp the CORE-DERIVED reach onto each declared affordance, computed
    // here where the full manifest (isolation type + ENFORCED permissions) is in
    // hand, and projected as a parallel view so the manifest descriptor itself
    // stays a pure declaration. The shell renders its blast-radius halo from this,
    // not from the self-declared `risk`.
    let mut affordance_reach: Vec<AffordanceReachView> = Vec::new();
    for interface in &manifest.interfaces {
        for method in &interface.methods {
            let reach = ReachDescriptorV1::derive(
                manifest.capsule_type.clone(),
                &manifest.permissions,
                method.resource.as_deref(),
                method.operation.as_deref(),
            );
            let declared_understates_reach = reach.declared_understates_reach(&method.risk);
            affordance_reach.push(AffordanceReachView {
                interface_id: interface.id.clone(),
                method_id: method.id.clone(),
                risk: method.risk.clone(),
                reach,
                declared_understates_reach,
            });
        }
    }
    let launchable = target.is_some() && role.is_shell_launchable();
    let signature_state = if manifest
        .signature
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        "manifest-signature-declared"
    } else {
        "no-manifest-signature"
    };
    let cid = component
        .and_then(|entry| entry.cid.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    let cid_state = if cid.is_some() {
        "cid-published"
    } else {
        "local-only"
    };

    CapsuleSummary {
        name: name.clone(),
        version: manifest.version,
        title: target
            .map(|target| target.title.clone())
            .unwrap_or_else(|| capsule_title(&name)),
        description: target
            .map(|target| target.description.clone())
            .or_else(|| manifest.description.clone())
            .unwrap_or_else(|| "Capsule metadata available through Runtime.".to_string()),
        author: manifest.author,
        role,
        capsule_type,
        category: category.to_string(),
        state: if installed { "installed" } else { "bundled" }.to_string(),
        installed,
        launchable,
        launch_target: target.map(|target| target.target.clone()),
        route: target.map(|target| target.route.clone()),
        provides: manifest.provides,
        requires: manifest
            .requires
            .into_iter()
            .map(|requirement| CapsuleRequirementSummary {
                name: requirement.name,
                kind: format!("{:?}", requirement.kind).to_ascii_lowercase(),
            })
            .collect(),
        capabilities: manifest.capabilities,
        interfaces: manifest.interfaces,
        affordance_reach,
        viewer: manifest.viewer,
        cid,
        cid_state: cid_state.to_string(),
        signature_state: signature_state.to_string(),
        trust_state: capsule_trust_state(signature_state, cid_state).to_string(),
        payment_state: capsule_payment_state(&name).to_string(),
        drm_state: capsule_drm_state(&name).to_string(),
        source: if installed {
            "installed"
        } else {
            "runtime-bundle"
        }
        .to_string(),
        install_path: component.and_then(|entry| entry.install_path.clone()),
        release_path: component.and_then(|entry| entry.release_path.clone()),
        repository: component.and_then(|entry| entry.repository.clone()),
    }
}

fn capsule_category(role: &CapsuleRole) -> &'static str {
    match role {
        CapsuleRole::Shell => "Shells",
        CapsuleRole::App => "Apps",
        CapsuleRole::Viewer => "Viewers",
        CapsuleRole::Provider => "Providers",
        CapsuleRole::Content => "Content",
    }
}

fn capsule_title(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capsule_category_order(category: &str) -> u8 {
    match category {
        "Apps" => 0,
        "Viewers" => 1,
        "Content" => 2,
        "Providers" => 3,
        "Shells" => 4,
        _ => 9,
    }
}

fn capsule_trust_state(signature_state: &str, cid_state: &str) -> &'static str {
    match (signature_state, cid_state) {
        ("manifest-signature-declared", "cid-published") => "cid-with-manifest-signature",
        ("manifest-signature-declared", _) => "local-manifest-signature",
        (_, "cid-published") => "cid-without-manifest-signature",
        _ => "local-dev",
    }
}

fn capsule_payment_state(name: &str) -> &'static str {
    if name.contains("wallet") {
        "provider"
    } else {
        "not-declared"
    }
}

fn capsule_drm_state(name: &str) -> &'static str {
    if matches!(
        name,
        "drm-provider" | "rights-provider" | "key-provider" | "decrypt-provider"
    ) {
        "provider"
    } else {
        "not-declared"
    }
}

#[derive(Clone, Debug, Default)]
struct CapsuleComponentInfo {
    cid: Option<String>,
    install_path: Option<String>,
    release_path: Option<String>,
    repository: Option<String>,
}

fn load_capsule_components(data_dir: &std::path::Path) -> BTreeMap<String, CapsuleComponentInfo> {
    let Ok(bytes) = std::fs::read(data_dir.join("components.json")) else {
        return BTreeMap::new();
    };
    let Ok(manifest) = serde_json::from_slice::<crate::setup::ComponentsManifest>(&bytes) else {
        return BTreeMap::new();
    };
    let mut entries = BTreeMap::new();
    for (name, capsule) in manifest.capsules {
        entries.insert(
            name,
            CapsuleComponentInfo {
                cid: Some(capsule.cid),
                install_path: None,
                release_path: None,
                repository: capsule.repository,
            },
        );
    }
    for (name, component) in manifest.external {
        let platform = component
            .platforms
            .get("*")
            .or_else(|| component.platforms.values().next());
        entries
            .entry(name)
            .and_modify(|entry| {
                if entry.cid.as_deref().unwrap_or("").is_empty() {
                    entry.cid = platform.and_then(|platform| platform.cid.clone());
                }
                if entry.install_path.is_none() {
                    entry.install_path = component.install_path.clone();
                }
                if entry.release_path.is_none() {
                    entry.release_path =
                        platform.and_then(|platform| platform.release_path.clone());
                }
                if entry.repository.is_none() {
                    entry.repository = component.repository.clone();
                }
            })
            .or_insert_with(|| CapsuleComponentInfo {
                cid: platform.and_then(|platform| platform.cid.clone()),
                repository: component.repository,
                install_path: component.install_path,
                release_path: platform.and_then(|platform| platform.release_path.clone()),
            });
    }
    entries
}

fn installed_capsule_names(
    data_dir: &std::path::Path,
    active_components: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let root = data_dir.join("capsules");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return names;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if crate::api::capsule_inventory::installed_capsule_is_inactive(
            data_dir,
            &dir,
            name,
            active_components,
        ) {
            continue;
        }
        if crate::api::capsule_inventory::load_capsule_manifest(&dir, name).is_some() {
            names.insert(name.to_string());
        }
    }
    names
}

#[derive(Serialize)]
pub(super) struct CapsuleCatalogResponse {
    schema: String,
    counts: CapsuleCatalogCounts,
    capsules: Vec<CapsuleSummary>,
    policy: CapsuleCatalogPolicy,
}

#[derive(Default, Serialize)]
struct CapsuleCatalogCounts {
    total: usize,
    installed: usize,
    launchable: usize,
    interfaces: usize,
    methods: usize,
    apps: usize,
    viewers: usize,
    providers: usize,
    content: usize,
    shell: usize,
}

#[derive(Serialize)]
struct CapsuleCatalogPolicy {
    install_state: String,
    install_note: String,
    payment_state: String,
    payment_note: String,
    drm_state: String,
    drm_note: String,
}

/// Shell-facing, core-DERIVED reach for one declared affordance (W0b). Projected
/// alongside the pure manifest descriptor; the `reach` is computed from the
/// capsule's enforced capability, NOT the self-declared `risk` (which is carried
/// here only so the shell can show the declaration next to the truth).
#[derive(Debug, Clone, Serialize)]
struct AffordanceReachView {
    interface_id: String,
    method_id: String,
    /// The capsule's self-declared risk (advisory).
    risk: AffordanceRisk,
    /// The core-computed reach (authoritative).
    reach: ReachDescriptorV1,
    /// True when the declaration reads low but the observed reach is far.
    declared_understates_reach: bool,
}

#[derive(Serialize)]
struct CapsuleSummary {
    name: String,
    version: String,
    title: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    role: CapsuleRole,
    #[serde(rename = "type")]
    capsule_type: CapsuleType,
    category: String,
    state: String,
    installed: bool,
    launchable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provides: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires: Vec<CapsuleRequirementSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    interfaces: Vec<CapsuleInterfaceDescriptor>,
    /// W0b: core-DERIVED reach per declared affordance (the blast-radius halo
    /// data), projected ALONGSIDE the pure manifest `interfaces` so the shell can
    /// render reach from real state. Keyed by (interface_id, method_id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    affordance_reach: Vec<AffordanceReachView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
    cid_state: String,
    signature_state: String,
    trust_state: String,
    payment_state: String,
    drm_state: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
}

#[derive(Serialize)]
struct CapsuleRequirementSummary {
    name: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapsuleInterfaceInvokeRequest {
    capsule: String,
    interface: String,
    method: String,
    #[serde(default)]
    input: serde_json::Value,
    /// A granted affordance-consent token (base64), presented on the RETRY of a
    /// consent-gated invocation (W2 step 8). Absent on the first call (which
    /// raises a consent request). When present, the gateway redeems it via
    /// validate-and-consume before dispatching — no token, no consented dispatch.
    #[serde(default)]
    consent_token: Option<String>,
}

#[derive(Serialize)]
struct CapsuleInterfaceInvokeResponse {
    schema: String,
    status: String,
    capsule: String,
    interface: String,
    method: String,
    request_id: String,
    output: serde_json::Value,
}

#[derive(Debug)]
struct ResolvedCapsuleAffordance {
    capsule: String,
    interface_id: String,
    method: CapsuleAffordanceDescriptor,
}

#[derive(Serialize)]
pub(super) struct CapsuleInterfaceRegistryResponse {
    schema: String,
    counts: CapsuleInterfaceRegistryCounts,
    interfaces: Vec<CapsuleInterfaceSummary>,
    policy: CapsuleInterfaceRegistryPolicy,
}

#[derive(Default, Serialize)]
struct CapsuleInterfaceRegistryCounts {
    capsules: usize,
    interfaces: usize,
    methods: usize,
}

#[derive(Serialize)]
struct CapsuleInterfaceSummary {
    capsule: String,
    capsule_version: String,
    title: String,
    role: CapsuleRole,
    #[serde(rename = "type")]
    capsule_type: CapsuleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
    trust_state: String,
    interface: CapsuleInterfaceDescriptor,
}

#[derive(Serialize)]
struct CapsuleInterfaceRegistryPolicy {
    descriptor_state: String,
    descriptor_note: String,
    invocation_state: String,
    invocation_note: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plan_affordance_dispatch_routes_each_case() {
        // Low-risk always dispatches directly, with or without a token.
        assert_eq!(
            plan_affordance_dispatch(InvocationGate::Direct, false),
            AffordanceDispatchPlan::DispatchDirect
        );
        assert_eq!(
            plan_affordance_dispatch(InvocationGate::Direct, true),
            AffordanceDispatchPlan::DispatchDirect
        );
        // Consent-gated, no token -> raise a consent request (never dispatch).
        assert_eq!(
            plan_affordance_dispatch(InvocationGate::Consent, false),
            AffordanceDispatchPlan::RaiseConsent
        );
        // Consent-gated, token presented -> redeem then dispatch.
        assert_eq!(
            plan_affordance_dispatch(InvocationGate::Consent, true),
            AffordanceDispatchPlan::RedeemThenDispatch
        );
    }

    fn write_capsule(data_dir: &std::path::Path, name: &str, role: &str, capsule_type: &str) {
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
        let entrypoint = match capsule_type {
            "wasm" => format!("{name}.wasm"),
            "microvm" => "rootfs.ext4".to_string(),
            _ => "index.html".to_string(),
        };
        fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": format!("{name} test capsule"),
                "author": "elastos",
                "role": role,
                "type": capsule_type,
                "entrypoint": entrypoint,
                "signature": "test-signature"
            }))
            .unwrap(),
        )
        .unwrap();
        if capsule_type == "wasm" {
            fs::write(dir.join(format!("{name}.wasm")), b"\0asm").unwrap();
            fs::create_dir_all(dir.join("browser")).unwrap();
            fs::write(dir.join("browser/index.html"), "<!doctype html>").unwrap();
        } else {
            fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        }
    }

    fn write_capsule_json(data_dir: &std::path::Path, name: &str, manifest: serde_json::Value) {
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        if manifest
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|capsule_type| capsule_type == "wasm")
        {
            fs::write(dir.join(format!("{name}.wasm")), b"\0asm").unwrap();
            fs::create_dir_all(dir.join("browser")).unwrap();
            fs::write(dir.join("browser").join("index.html"), "<!doctype html>").unwrap();
        } else {
            fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        }
    }

    #[test]
    fn capsule_catalog_lists_roles_and_launchable_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule(data_dir.path(), "marketplace", "app", "wasm");
        write_capsule(data_dir.path(), "documents", "viewer", "wasm");
        write_capsule(data_dir.path(), "object-provider", "provider", "microvm");

        let catalog = capsule_catalog_summary(data_dir.path());

        assert_eq!(catalog.schema, CAPSULE_CATALOG_SCHEMA);
        assert!(catalog.counts.total >= 3);
        assert!(catalog.counts.apps >= 1);
        assert!(catalog.counts.viewers >= 1);
        assert!(catalog.counts.providers >= 1);
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert!(marketplace.launchable);
        assert_eq!(marketplace.trust_state, "local-manifest-signature");
        let provider = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "object-provider")
            .unwrap();
        assert!(!provider.launchable);
        assert!(provider.repository.is_none());
    }

    #[test]
    fn capsule_interface_registry_lists_declared_affordances() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "marketplace",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace test capsule",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm",
                "signature": "test-signature",
                "interfaces": [{
                    "id": "elastos.marketplace.catalog",
                    "version": "0.1.0",
                    "methods": [
                        {
                            "id": "catalog.list",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://capsules/*",
                            "operation": "list"
                        },
                        {
                            "id": "capsule.open",
                            "risk": "launch",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://capsules/*",
                            "operation": "launch"
                        }
                    ]
                }]
            }),
        );
        // capsule_roots() also scans DEV_CAPSULES_ROOT (the repo's shipped
        // capsules/), which since the Flint merge declare their own typed
        // affordances. Global catalog/registry counts therefore reflect the whole
        // shipped surface, not just the seeded capsule — so assert on the
        // marketplace capsule's OWN contribution (what this test verifies),
        // immune to how many other capsules ship affordances.
        let catalog = capsule_catalog_summary(data_dir.path());
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert_eq!(marketplace.interfaces.len(), 1);
        assert_eq!(marketplace.interfaces[0].id, "elastos.marketplace.catalog");

        let registry = capsule_interface_registry_summary(data_dir.path());
        assert_eq!(registry.schema, CAPSULE_INTERFACE_REGISTRY_SCHEMA);
        let marketplace_entry = registry
            .interfaces
            .iter()
            .find(|entry| entry.capsule == "marketplace")
            .expect("marketplace interface listed in the registry");
        assert_eq!(marketplace_entry.interface.methods.len(), 2);
        assert_eq!(marketplace_entry.interface.methods[1].id, "capsule.open");
        assert_eq!(registry.policy.invocation_state, "runtime-gated");
    }

    #[test]
    fn catalog_projects_core_derived_reach_per_affordance() {
        use elastos_common::{EgressReach, IsolationTier, ResourceScope, Reversibility};
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "reachy",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "reachy",
                "version": "0.1.0",
                "description": "reach projection test",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "reachy.wasm",
                "signature": "test-signature",
                "interfaces": [{
                    "id": "iface.reach",
                    "version": "0.1.0",
                    "methods": [
                        {
                            "id": "scan.all",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://*",
                            "operation": "read"
                        },
                        {
                            "id": "open.one",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://rights/film-x",
                            "operation": "read"
                        }
                    ]
                }]
            }),
        );

        let catalog = capsule_catalog_summary(data_dir.path());
        let reachy = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "reachy")
            .expect("reachy capsule in the catalog");

        // Every declared affordance carries a core-derived reach view.
        let scan = reachy
            .affordance_reach
            .iter()
            .find(|view| view.method_id == "scan.all")
            .expect("reach projected for scan.all");
        assert_eq!(scan.reach.isolation, IsolationTier::Wasm);
        assert_eq!(scan.reach.egress, EgressReach::None);
        assert_eq!(scan.reach.scope, ResourceScope::System); // scheme-level wildcard
        assert_eq!(scan.reach.reversibility, Reversibility::Reversible);
        assert!(scan.reach.observed);
        // Declared "read" but the reach is system-wide → the lie is flagged on the
        // projection the shell renders.
        assert!(
            scan.declared_understates_reach,
            "declared-low + system-scope must be flagged on the projection"
        );

        let open = reachy
            .affordance_reach
            .iter()
            .find(|view| view.method_id == "open.one")
            .expect("reach projected for open.one");
        assert_eq!(open.reach.scope, ResourceScope::Object);
        assert!(
            !open.declared_understates_reach,
            "a genuinely contained read is not flagged"
        );
    }

    #[test]
    fn capsule_affordance_resolves_declared_descriptor() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "marketplace",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace test capsule",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm",
                "signature": "test-signature",
                "interfaces": [{
                    "id": "elastos.marketplace.catalog",
                    "version": "0.1.0",
                    "methods": [{
                        "id": "catalog.list",
                        "risk": "read",
                        "approval": "runtime_policy",
                        "audit": "event",
                        "resource": "elastos://capsules/*",
                        "operation": "list"
                    }]
                }]
            }),
        );

        let request = CapsuleInterfaceInvokeRequest {
            capsule: "marketplace".to_string(),
            interface: "elastos.marketplace.catalog".to_string(),
            method: "catalog.list".to_string(),
            input: serde_json::json!({}),
            consent_token: None,
        };
        let resolved = resolve_capsule_affordance(data_dir.path(), &request).unwrap();
        assert_eq!(resolved.capsule, "marketplace");
        assert_eq!(resolved.interface_id, "elastos.marketplace.catalog");
        assert_eq!(resolved.method.id, "catalog.list");
    }

    #[test]
    fn capsule_interface_invoke_request_rejects_hidden_authority_fields() {
        let err = serde_json::from_value::<CapsuleInterfaceInvokeRequest>(serde_json::json!({
            "capsule": "marketplace",
            "interface": "elastos.marketplace.catalog",
            "method": "catalog.list",
            "input": {},
            "principal_id": "person:other",
            "_runtime_invocation": {
                "schema": "forged"
            }
        }))
        .unwrap_err();

        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn affordance_dispatch_is_metered_on_the_unified_vm_name_budget() {
        use elastos_runtime::primitives::spend::SpendMeter;
        use std::sync::OnceLock;

        let dir = tempfile::tempdir().unwrap();
        let meter = Arc::new(SpendMeter::new());
        let metered = GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(OnceLock::new()),
            cache_dir: dir.path().to_path_buf(),
            data_dir: dir.path().to_path_buf(),
            audit_log: Arc::new(OnceLock::new()),
            spend_policy: Some(crate::carrier_bridge::SpendPolicy {
                meter: meter.clone(),
                default_budget: 1,
            }),
        };

        // First affordance act fits budget 1 and debits the CANONICAL vm-{name} key — the same key
        // the carrier paths debit, so the budget is unified, not a separate per-plane meter.
        assert!(charge_affordance_spend(&metered, "viewer").is_ok());
        assert_eq!(
            meter.remaining("vm-viewer"),
            0,
            "the affordance debit lands on the unified vm-{{name}} budget"
        );

        // The over-budget second act is refused fail-closed (429), before any dispatch.
        let err = charge_affordance_spend(&metered, "viewer").unwrap_err();
        assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.1, "budget_exhausted");

        // An unmetered gateway (no policy) never charges — affordance flows unchanged.
        let unmetered = GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(OnceLock::new()),
            cache_dir: dir.path().to_path_buf(),
            data_dir: dir.path().to_path_buf(),
            audit_log: Arc::new(OnceLock::new()),
            spend_policy: None,
        };
        assert!(charge_affordance_spend(&unmetered, "viewer").is_ok());
    }

    #[test]
    fn capsule_affordance_policy_routes_consent_vs_direct() {
        // High-risk (Payment) now routes to consent instead of a flat 403.
        let high_risk = ResolvedCapsuleAffordance {
            capsule: "wallet".to_string(),
            interface_id: "elastos.wallet.payment".to_string(),
            method: CapsuleAffordanceDescriptor {
                id: "payment.send".to_string(),
                description: None,
                risk: AffordanceRisk::Payment,
                approval: AffordanceApprovalMode::RuntimePolicy,
                audit: elastos_common::AffordanceAuditMode::Full,
                resource: Some("elastos://wallet/*".to_string()),
                operation: Some("send".to_string()),
                input_schema: None,
                output_schema: None,
            },
        };
        assert_eq!(
            enforce_affordance_invocation_policy(&high_risk),
            InvocationGate::Consent
        );

        // AffordanceApprovalMode::User routes to consent regardless of (low) risk.
        let user_gated = resolved_with(AffordanceRisk::Read, Some("elastos://x"), Some("read"));
        assert_eq!(
            enforce_affordance_invocation_policy(&user_gated),
            InvocationGate::Consent
        );

        // Low-risk + RuntimePolicy dispatches directly (no consent).
        let direct = ResolvedCapsuleAffordance {
            capsule: "viewer".to_string(),
            interface_id: "elastos.viewer.media".to_string(),
            method: CapsuleAffordanceDescriptor {
                id: "catalog.list".to_string(),
                description: None,
                risk: AffordanceRisk::Read,
                approval: AffordanceApprovalMode::RuntimePolicy,
                audit: elastos_common::AffordanceAuditMode::Summary,
                resource: Some("elastos://capsules/*".to_string()),
                operation: Some("list".to_string()),
                input_schema: None,
                output_schema: None,
            },
        };
        assert_eq!(
            enforce_affordance_invocation_policy(&direct),
            InvocationGate::Direct
        );
    }

    fn resolved_with(
        risk: AffordanceRisk,
        resource: Option<&str>,
        operation: Option<&str>,
    ) -> ResolvedCapsuleAffordance {
        ResolvedCapsuleAffordance {
            capsule: "viewer".to_string(),
            interface_id: "elastos.viewer.media".to_string(),
            method: CapsuleAffordanceDescriptor {
                id: "open".to_string(),
                description: None,
                risk,
                approval: AffordanceApprovalMode::User,
                audit: elastos_common::AffordanceAuditMode::Full,
                resource: resource.map(str::to_string),
                operation: operation.map(str::to_string),
                input_schema: None,
                output_schema: None,
            },
        }
    }

    #[test]
    fn consent_descriptor_fails_closed_without_resource() {
        let none = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Payment,
            None,
            Some("send"),
        ));
        assert_eq!(none.unwrap_err().1, "descriptor_resource_missing");
        let blank = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Payment,
            Some("   "),
            Some("send"),
        ));
        assert_eq!(blank.unwrap_err().1, "descriptor_resource_missing");
    }

    #[test]
    fn consent_descriptor_fails_closed_on_unmapped_operation() {
        let err = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Write,
            Some("elastos://content/x"),
            Some("frobnicate"),
        ))
        .unwrap_err();
        assert_eq!(err.1, "operation_unmapped");
    }

    #[test]
    fn consent_descriptor_never_grants_admin_from_risk_alone() {
        // Privileged risk with no operation must NOT yield Admin (capped at Execute).
        let (_, action, _) = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Privileged,
            Some("elastos://sys/*"),
            None,
        ))
        .unwrap();
        assert_eq!(action, Action::Execute);
        // Privileged risk with a non-admin operation also stays capped at Execute.
        let (_, action2, _) = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Privileged,
            Some("elastos://sys/*"),
            Some("read"),
        ))
        .unwrap();
        assert_eq!(action2, Action::Execute);
        // Admin is reachable ONLY via an authoritative admin/manage operation.
        let (_, action3, _) = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Privileged,
            Some("elastos://sys/*"),
            Some("admin.reset"),
        ))
        .unwrap();
        assert_eq!(action3, Action::Admin);
    }

    #[test]
    fn consent_descriptor_maps_resource_and_action() {
        let (resource, action, risk) = affordance_consent_descriptor(&resolved_with(
            AffordanceRisk::Read,
            Some("elastos://content/film-x"),
            Some("get"),
        ))
        .unwrap();
        assert_eq!(resource, "elastos://content/film-x");
        assert_eq!(action, Action::Read);
        assert_eq!(risk, AffordanceRisk::Read);
    }
}
