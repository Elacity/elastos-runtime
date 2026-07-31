use super::*;

pub(in crate::api::gateway) async fn system_wallet_approvals(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    Json(system_wallet_approvals_summary(&state, &authority, false).await).into_response()
}

pub(in crate::api::gateway) async fn system_wallet_approval_reject(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalRejectRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();
    reject_wallet_approval_request(
        &state,
        &context,
        &authority,
        &request_id,
        input,
        SYSTEM_CAPSULE_ID,
    )
    .await
}

pub(in crate::api::gateway) async fn reject_wallet_approval_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    input: WalletApprovalRejectRequest,
    capsule_id: &'static str,
) -> Response {
    match runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::RejectApproval {
            request_id: request_id.to_string(),
            reason: input.reason.unwrap_or_else(|| {
                if capsule_id == SYSTEM_CAPSULE_ID {
                    "Rejected in System".to_string()
                } else {
                    "Rejected in Wallet".to_string()
                }
            }),
        },
    )
    .await
    {
        Ok(_) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id,
                    event_type: "wallet.approval.rejected",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id,
                    result: "rejected",
                    reason: "Wallet approval rejected through Runtime authority",
                },
            );
            Json(system_wallet_approvals_summary(state, authority, false).await).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn system_wallet_approval_approve(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalApproveRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();
    approve_wallet_managed_request(
        &state,
        &context,
        &authority,
        &request_id,
        input,
        SYSTEM_CAPSULE_ID,
    )
    .await
}

pub(in crate::api::gateway) async fn approve_wallet_managed_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    input: WalletApprovalApproveRequest,
    capsule_id: &'static str,
) -> Response {
    let Some(home_token) = input.home_token.as_deref() else {
        return system_error_response(anyhow::anyhow!(
            "fresh passkey verification is required to sign with a built-in wallet"
        ));
    };
    if let Err(err) = consume_fresh_passkey_home_token(
        &state.data_dir,
        home_token,
        context,
        capsule_id,
        180,
        "wallet.approve",
        &serde_json::json!({
            "request_id": request_id,
            "reason": input.reason,
        }),
    ) {
        return system_error_response(err);
    }
    let reason = input.reason.unwrap_or_else(|| {
        if capsule_id == SYSTEM_CAPSULE_ID {
            "Approved in System".to_string()
        } else {
            "Approved in Wallet".to_string()
        }
    });
    match approve_managed_wallet_request(
        state,
        &state.data_dir,
        context,
        authority,
        request_id,
        &reason,
        capsule_id,
    )
    .await
    {
        Ok(outcome) => {
            let mut summary = system_wallet_approvals_summary(state, authority, false).await;
            summary.note = Some(outcome.message);
            Json(summary).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) struct WalletApprovalReviewOutcome {
    pub(in crate::api::gateway) message: String,
    pub(in crate::api::gateway) handoff: Option<serde_json::Value>,
    pub(in crate::api::gateway) signed_result: Option<serde_json::Value>,
    pub(in crate::api::gateway) signed_transaction: Option<String>,
}

pub(in crate::api::gateway) async fn approve_managed_wallet_request(
    state: &GatewayState,
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    reason: &str,
    capsule_id: &'static str,
) -> anyhow::Result<WalletApprovalReviewOutcome> {
    let request = pending_wallet_approval_request(state, authority, request_id).await?;
    if !is_managed_wallet_proof_type(&request.proof_type) {
        anyhow::bail!("Open the approval method to approve external wallet requests");
    }
    let signed = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: request_id.to_string(),
            reason: reason.to_string(),
        },
    )
    .await?;
    append_wallet_approval_audit(
        data_dir,
        WalletApprovalAuditInput {
            capsule_id,
            event_type: "wallet.approval.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "completed",
            reason: "Built-in managed wallet signed after Runtime approval",
        },
    )?;
    Ok(WalletApprovalReviewOutcome {
        message: "Approved and signed by built-in wallet.".to_string(),
        handoff: None,
        signed_result: signed.get("approval_request").and_then(|request| {
            request
                .get("signed_result")
                .filter(|value| value.is_object())
                .cloned()
        }),
        signed_transaction: signed
            .get("signed_transaction")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    })
}

pub(in crate::api::gateway) async fn approve_external_wallet_request(
    state: &GatewayState,
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    reason: &str,
    capsule_id: &str,
) -> anyhow::Result<WalletApprovalReviewOutcome> {
    let request = pending_wallet_approval_request(state, authority, request_id).await?;
    if is_managed_wallet_proof_type(&request.proof_type) {
        anyhow::bail!("Use System to approve built-in wallet requests");
    }
    if request.connector_id.as_deref() != Some(capsule_id) {
        anyhow::bail!("wallet approval belongs to a different connector");
    }
    let data = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: request_id.to_string(),
            reason: reason.to_string(),
        },
    )
    .await?;
    let handoff = data.get("handoff").cloned();
    append_wallet_approval_audit(
        data_dir,
        WalletApprovalAuditInput {
            capsule_id,
            event_type: "wallet.approval.approved",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "approved",
            reason: "External wallet approval reviewed through approval method",
        },
    )?;
    Ok(WalletApprovalReviewOutcome {
        message: format!(
            "Approved. Continue in {}.",
            wallet_connector_label(capsule_id)
        ),
        handoff,
        signed_result: None,
        signed_transaction: None,
    })
}

pub(in crate::api::gateway) async fn complete_external_wallet_approval(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    input: WalletApprovalCompleteRequest,
    capsule_id: &str,
    audit_reason: &str,
) -> anyhow::Result<SystemWalletApprovalsSummary> {
    runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id: request_id.to_string(),
            payload_hash: input.payload_hash,
            signature: input.signature,
            signature_type: input.signature_type,
            public_key: input.public_key,
            signer: input.signer,
            transaction_hash: input.transaction_hash,
        },
    )
    .await?;
    append_wallet_approval_audit(
        &state.data_dir,
        WalletApprovalAuditInput {
            capsule_id,
            event_type: "wallet.approval.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "completed",
            reason: audit_reason,
        },
    )?;
    Ok(system_wallet_approvals_summary(state, authority, false).await)
}

pub(in crate::api::gateway) async fn pending_wallet_approval_request(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> anyhow::Result<SystemWalletApprovalSummary> {
    let summary = system_wallet_approvals_summary(state, authority, false).await;
    if !summary.available {
        anyhow::bail!(
            "{}",
            summary
                .note
                .unwrap_or_else(|| "wallet approvals unavailable".to_string())
        );
    }
    summary
        .approval_requests
        .into_iter()
        .find(|request| request.request_id == request_id)
        .ok_or_else(|| anyhow::anyhow!("wallet approval request not found"))
}

pub(in crate::api::gateway) async fn system_wallet_approvals_summary(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    include_resolved: bool,
) -> SystemWalletApprovalsSummary {
    let response = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ListApprovals { include_resolved },
    )
    .await;
    match response {
        Ok(data) => {
            let approval_requests = data
                .get("approval_requests")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(system_wallet_approval_summary)
                .collect::<Vec<_>>();
            let pending_count = approval_requests
                .iter()
                .filter(|request| request.status == "pending")
                .count();
            SystemWalletApprovalsSummary {
                available: true,
                pending_count,
                approval_requests,
                handoff: None,
                note: None,
            }
        }
        Err(err) => SystemWalletApprovalsSummary {
            available: false,
            note: Some(err.to_string()),
            ..SystemWalletApprovalsSummary::default()
        },
    }
}

pub(in crate::api::gateway) struct WalletApprovalAuditInput<'a> {
    pub(in crate::api::gateway) capsule_id: &'a str,
    pub(in crate::api::gateway) event_type: &'a str,
    pub(in crate::api::gateway) principal_id: &'a str,
    pub(in crate::api::gateway) session_id: &'a str,
    pub(in crate::api::gateway) request_id: &'a str,
    pub(in crate::api::gateway) result: &'a str,
    pub(in crate::api::gateway) reason: &'a str,
}

pub(in crate::api::gateway) struct ProviderEffectAuditInput<'a> {
    pub(in crate::api::gateway) capsule_id: &'a str,
    pub(in crate::api::gateway) event_type: &'a str,
    pub(in crate::api::gateway) principal_id: &'a str,
    pub(in crate::api::gateway) session_id: &'a str,
    pub(in crate::api::gateway) request_id: &'a str,
    pub(in crate::api::gateway) result: &'a str,
    pub(in crate::api::gateway) reason: &'a str,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct ChainLifecycleEffectAudit {
    pub(in crate::api::gateway) request_id: String,
    pub(in crate::api::gateway) network: String,
    pub(in crate::api::gateway) action: String,
}

pub(in crate::api::gateway) fn chain_lifecycle_effect_audit(
    scheme: &str,
    op: &str,
    request: &serde_json::Value,
) -> Option<ChainLifecycleEffectAudit> {
    if scheme != "chain" || op != "node_lifecycle" {
        return None;
    }
    let network = request.get("network")?.as_str()?.trim();
    let action = request.get("action")?.as_str()?.trim();
    if network.is_empty() || action.is_empty() || action == "status" {
        return None;
    }
    Some(ChainLifecycleEffectAudit {
        request_id: format!("chain-node-lifecycle:{network}:{action}:{}", now_ts()),
        network: network.to_string(),
        action: action.to_string(),
    })
}

pub(in crate::api::gateway) fn append_provider_effect_audit(
    data_dir: &FsPath,
    input: ProviderEffectAuditInput<'_>,
) -> anyhow::Result<()> {
    let now = now_ts();
    crate::auth::append_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!(
                "audit:provider-effect:{}:{}:{now}",
                input.event_type, input.request_id
            ),
            event_type: input.event_type.to_string(),
            principal_id: Some(input.principal_id.to_string()),
            proof_binding_id: None,
            session_id: Some(input.session_id.to_string()),
            challenge_id: Some(input.request_id.to_string()),
            capsule_id: Some(input.capsule_id.to_string()),
            result: input.result.to_string(),
            reason: input.reason.to_string(),
            occurred_at: now,
            signer_did: None,
            signature: None,
        },
    )
}

pub(in crate::api::gateway) fn append_wallet_approval_audit(
    data_dir: &FsPath,
    input: WalletApprovalAuditInput<'_>,
) -> anyhow::Result<()> {
    let now = now_ts();
    crate::auth::append_audit_event(
        data_dir,
        RuntimeAuditEventV1 {
            schema: RuntimeAuditEventV1::SCHEMA.to_string(),
            event_id: format!(
                "audit:wallet-approval:{}:{}:{now}",
                input.event_type, input.request_id
            ),
            event_type: input.event_type.to_string(),
            principal_id: Some(input.principal_id.to_string()),
            proof_binding_id: None,
            session_id: Some(input.session_id.to_string()),
            challenge_id: Some(input.request_id.to_string()),
            capsule_id: Some(input.capsule_id.to_string()),
            result: input.result.to_string(),
            reason: input.reason.to_string(),
            occurred_at: now,
            signer_did: None,
            signature: None,
        },
    )
}

pub(in crate::api::gateway) fn system_wallet_approval_summary(
    value: &serde_json::Value,
) -> Option<SystemWalletApprovalSummary> {
    let signed_result = value.get("signed_result").filter(|value| value.is_object());
    let signature_receipt = value
        .get("signature_receipt")
        .filter(|value| value.is_object());
    Some(SystemWalletApprovalSummary {
        request_id: value.get("request_id")?.as_str()?.to_string(),
        status: value.get("status")?.as_str()?.to_string(),
        intent: value.get("intent")?.as_str()?.to_string(),
        capsule_id: value
            .get("requested_by_actor")
            .or_else(|| value.get("capsule_id"))?
            .as_str()?
            .to_string(),
        resource: value.get("resource")?.as_str()?.to_string(),
        reason: value.get("reason")?.as_str()?.to_string(),
        account_id: value.get("account_id")?.as_str()?.to_string(),
        address: value.get("address")?.as_str()?.to_string(),
        proof_type: value
            .get("proof_type")
            .and_then(|value| value.as_str())
            .unwrap_or("external")
            .to_string(),
        connector_id: value
            .get("connector_id")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        created_at: value.get("created_at")?.as_u64()?,
        expires_at: value.get("expires_at")?.as_u64()?,
        completed_at: value
            .get("completed_at")
            .and_then(|value| value.as_u64())
            .or_else(|| {
                signature_receipt
                    .and_then(|value| value.get("completed_at"))
                    .and_then(|value| value.as_u64())
            }),
        transaction_hash: signed_result
            .and_then(|value| value.get("transaction_hash"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    })
}
