use super::*;

pub(in crate::api::gateway) async fn wallet_app_managed_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<SystemWalletManagedCreateRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    create_managed_wallet_accounts(&state, &context, &authority, input, WALLET_CAPSULE_ID).await
}

pub(in crate::api::gateway) async fn wallet_app_default_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<SystemWalletDefaultRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    update_default_wallet_account(&state, &context, &authority, input, WALLET_CAPSULE_ID).await
}

pub(in crate::api::gateway) async fn wallet_app_account_delete(
    State(state): State<GatewayState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<WalletAccountDeleteRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    if let Err(err) = consume_fresh_passkey_home_token(
        &state.data_dir,
        &input.home_token,
        &context,
        WALLET_CAPSULE_ID,
        180,
        "wallet.account.delete",
        &serde_json::json!({ "account_id": account_id }),
    ) {
        return system_error_response(err);
    }
    match runtime_wallet_data(
        &state,
        &authority,
        elastos_wallet_contract::WalletProviderOperationV2::RevokeAccount { account_id },
    )
    .await
    {
        Ok(_) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id: WALLET_CAPSULE_ID,
                    event_type: "wallet.account.deleted",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: "wallet-account-delete",
                    result: "ok",
                    reason:
                        "Wallet account deleted through Wallet after fresh passkey verification",
                },
            );
            Json(system_wallet_accounts_summary(&state, &authority).await).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_account_rename(
    State(state): State<GatewayState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<WalletAccountRenameRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    match runtime_wallet_data(
        &state,
        &authority,
        elastos_wallet_contract::WalletProviderOperationV2::RenameAccount {
            account_id,
            label: input.label,
        },
    )
    .await
    {
        Ok(_) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id: WALLET_CAPSULE_ID,
                    event_type: "wallet.account.renamed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: "wallet-account-rename",
                    result: "ok",
                    reason: "Wallet account renamed through Wallet",
                },
            );
            Json(system_wallet_accounts_summary(&state, &authority).await).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_account_recovery_key(
    State(state): State<GatewayState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<WalletAccountRecoveryKeyRequest>,
) -> Response {
    let context = match require_wallet_app_launch_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    if let Err(err) = consume_fresh_passkey_home_token(
        &state.data_dir,
        &input.home_token,
        &context,
        WALLET_CAPSULE_ID,
        180,
        "wallet.recovery-key.export",
        &serde_json::json!({ "account_id": account_id }),
    ) {
        return system_error_response(err);
    }
    match crate::api::auth_gateway::wallet_provider_data(
        &state,
        serde_json::json!({
            "op": "export_managed_secret",
            "principal_id": context.principal_id.clone(),
            "account_id": account_id,
        }),
    )
    .await
    {
        Ok(data) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id: WALLET_CAPSULE_ID,
                    event_type: "wallet.recovery_key.viewed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: "wallet-recovery-key",
                    result: "ok",
                    reason: "Wallet recovery key viewed after fresh passkey verification",
                },
            );
            Json::<serde_json::Value>(data).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_account_import_recovery_key(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<WalletAccountImportRecoveryKeyRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    if let Err(err) = consume_fresh_passkey_home_token(
        &state.data_dir,
        &input.home_token,
        &context,
        WALLET_CAPSULE_ID,
        180,
        "wallet.recovery-key.import",
        &serde_json::json!({
            "recovery_key": input.recovery_key,
            "label": input.label,
        }),
    ) {
        return system_error_response(err);
    }
    match crate::api::auth_gateway::wallet_provider_data(
        &state,
        serde_json::json!({
            "op": "import_managed_secret",
            "principal_id": context.principal_id.clone(),
            "recovery_key": input.recovery_key,
            "label": input.label,
        }),
    )
    .await
    {
        Ok(_) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id: WALLET_CAPSULE_ID,
                    event_type: "wallet.recovery_key.imported",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: "wallet-recovery-key-import",
                    result: "ok",
                    reason: "Wallet recovery key imported after fresh passkey verification",
                },
            );
            Json(system_wallet_accounts_summary(&state, &authority).await).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_approval_reject(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalRejectRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
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
        WALLET_CAPSULE_ID,
    )
    .await
}

pub(in crate::api::gateway) async fn wallet_app_managed_approval_approve(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalApproveRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
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
        WALLET_CAPSULE_ID,
    )
    .await
}

pub(in crate::api::gateway) async fn wallet_app_external_approval_approve(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalApproveRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    let reason = input
        .reason
        .unwrap_or_else(|| "Approved in Wallet".to_string());
    match approve_external_wallet_request(
        &state,
        &state.data_dir,
        &context,
        &authority,
        &request_id,
        &reason,
        WALLET_CAPSULE_ID,
    )
    .await
    {
        Ok(outcome) => {
            let mut summary = system_wallet_approvals_summary(&state, &authority, false).await;
            summary.note = Some(outcome.message);
            summary.handoff = outcome.handoff;
            Json(summary).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_external_approval_complete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<WalletApprovalCompleteRequest>,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = authority.home_launch_context();
    match complete_external_wallet_approval(
        &state,
        &context,
        &authority,
        &request_id,
        input,
        WALLET_CAPSULE_ID,
        "External wallet signature completed through Wallet",
    )
    .await
    {
        Ok(mut summary) => {
            summary.note = Some("Signed by Wallet.".to_string());
            Json(summary).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) fn managed_wallet_label(chain_namespace: &str) -> String {
    match chain_namespace {
        "eip155:20" => "ELA Wallet".to_string(),
        "eip155:8453" => "Spending".to_string(),
        "bip122:000000000019d6689c085ae165831e93" => "Savings".to_string(),
        value => value.to_string(),
    }
}
