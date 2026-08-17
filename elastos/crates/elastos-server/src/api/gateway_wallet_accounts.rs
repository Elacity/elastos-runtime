use super::*;

/// `pub(in crate::api)`, not the narrower `pub(in crate::api::gateway)` most of this module's
/// siblings use: `viewer_open::prepare_owned_grant` (a sibling of `gateway`, not a descendant of
/// it) needs to build the same summary the wallet-connector handlers use, to resolve the
/// principal's default EVM connector for the wallet delegation signature. Keep everything else in
/// this file at the narrower default — only this one entry point needed widening.
pub(in crate::api) async fn system_wallet_accounts_summary(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
) -> SystemWalletAccountsSummary {
    let response = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    )
    .await;
    wallet_accounts_summary_from_response(response)
}

// `pub(in crate::api)`, not the narrower `pub(in crate::api::gateway)` most callers use:
// `viewer_grant_sign` (a sibling of `gateway`, not a descendant of it) dispatches its own
// `RequestApproval`/`ListApprovals` wallet-provider operations directly through this generic
// primitive for the dDRM delegation-sign approval flow, the same way `viewer_open::prepare_owned_grant`
// already reaches `system_wallet_accounts_summary` at this width.
pub(in crate::api) async fn runtime_wallet_data(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    operation: elastos_wallet_contract::WalletProviderOperationV2,
) -> anyhow::Result<serde_json::Value> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("wallet provider unavailable"))?;
    match RuntimeWalletAdapter::new(registry, authority)
        .invoke(operation)
        .await?
        .result
    {
        elastos_wallet_contract::WalletResultV2::Ok { data } => Ok(data),
        elastos_wallet_contract::WalletResultV2::Error { message, .. } => anyhow::bail!(message),
    }
}

pub(in crate::api::gateway) async fn runtime_wallet_data_with_request_id(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: String,
    operation: elastos_wallet_contract::WalletProviderOperationV2,
) -> anyhow::Result<serde_json::Value> {
    let registry = state
        .provider_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("wallet provider unavailable"))?;
    match RuntimeWalletAdapter::new(registry, authority)
        .invoke_with_request_id(request_id, operation)
        .await?
        .result
    {
        elastos_wallet_contract::WalletResultV2::Ok { data } => Ok(data),
        elastos_wallet_contract::WalletResultV2::Error { message, .. } => anyhow::bail!(message),
    }
}

fn wallet_accounts_summary_from_response(
    response: anyhow::Result<serde_json::Value>,
) -> SystemWalletAccountsSummary {
    match response {
        Ok(data) => {
            let accounts = data
                .get("accounts")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(system_wallet_account_summary)
                .collect::<Vec<_>>();
            let default_accounts = data
                .get("default_accounts")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(system_wallet_default_summary)
                .collect::<Vec<_>>();
            SystemWalletAccountsSummary {
                available: true,
                linked_count: accounts.len(),
                accounts,
                default_accounts,
                note: None,
            }
        }
        Err(err) => SystemWalletAccountsSummary {
            available: false,
            note: Some(err.to_string()),
            ..SystemWalletAccountsSummary::default()
        },
    }
}

pub(in crate::api::gateway) fn system_wallet_account_summary(
    value: &serde_json::Value,
) -> Option<SystemWalletAccountSummary> {
    Some(SystemWalletAccountSummary {
        account_id: value.get("account_id")?.as_str()?.to_string(),
        chain_namespace: value.get("chain_namespace")?.as_str()?.to_string(),
        address: value.get("address")?.as_str()?.to_string(),
        proof_type: value.get("proof_type")?.as_str()?.to_string(),
        signing_available: value
            .get("signing_available")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        signing_status: value
            .get("signing_status")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        label: value
            .get("label")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        connector_id: value
            .get("connector_id")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        linked_at: value.get("linked_at")?.as_u64()?,
    })
}

pub(in crate::api::gateway) fn system_wallet_default_summary(
    value: &serde_json::Value,
) -> Option<SystemWalletDefaultSummary> {
    Some(SystemWalletDefaultSummary {
        chain_namespace: value.get("chain_namespace")?.as_str()?.to_string(),
        intent: value.get("intent")?.as_str()?.to_string(),
        account_id: value.get("account_id")?.as_str()?.to_string(),
        set_at: value.get("set_at")?.as_u64()?,
    })
}

pub(in crate::api::gateway) async fn system_wallet_managed_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<SystemWalletManagedCreateRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();
    create_managed_wallet_accounts(&state, &context, &authority, input, SYSTEM_CAPSULE_ID).await
}

pub(in crate::api::gateway) async fn create_managed_wallet_accounts(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    input: SystemWalletManagedCreateRequest,
    capsule_id: &'static str,
) -> Response {
    let chain_namespaces = input
        .chain_namespace
        .map(|namespace| vec![namespace])
        .unwrap_or_else(|| {
            MANAGED_WALLET_CHAIN_NAMESPACES
                .iter()
                .map(|namespace| (*namespace).to_string())
                .collect()
        });
    for chain_namespace in chain_namespaces {
        let label = input
            .label
            .clone()
            .unwrap_or_else(|| managed_wallet_label(&chain_namespace));
        if let Err(err) = runtime_wallet_data(
            state,
            authority,
            elastos_wallet_contract::WalletProviderOperationV2::CreateManagedAccount {
                chain_namespace,
                label: Some(label),
                create_new: input.create_new,
            },
        )
        .await
        {
            return system_error_response(err);
        }
    }
    let _ = append_wallet_approval_audit(
        &state.data_dir,
        WalletApprovalAuditInput {
            capsule_id,
            event_type: "wallet.managed.created",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: "wallet-managed",
            result: "ok",
            reason: if capsule_id == SYSTEM_CAPSULE_ID {
                "Built-in managed wallet accounts created through System"
            } else {
                "Built-in managed wallet accounts created through Wallet"
            },
        },
    );
    Json(system_wallet_accounts_summary(state, authority).await).into_response()
}

pub(in crate::api::gateway) async fn system_wallet_default_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<SystemWalletDefaultRequest>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();
    update_default_wallet_account(&state, &context, &authority, input, SYSTEM_CAPSULE_ID).await
}

pub(in crate::api::gateway) async fn update_default_wallet_account(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    input: SystemWalletDefaultRequest,
    capsule_id: &'static str,
) -> Response {
    let account_id = input.account_id.clone();
    let chain_namespace = input.chain_namespace.clone();
    let intent = input.intent.clone();
    let mirror_browser_default =
        chain_namespace.starts_with("eip155:") && intent == "transaction_intent";
    match runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::SetDefaultAccount {
            chain_namespace: chain_namespace.clone(),
            intent: intent.clone(),
            account_id: account_id.clone(),
        },
    )
    .await
    {
        Ok(_) => {
            if mirror_browser_default {
                if let Err(err) = runtime_wallet_data(
                    state,
                    authority,
                    elastos_wallet_contract::WalletProviderOperationV2::SetDefaultAccount {
                        chain_namespace: chain_namespace.clone(),
                        intent: "browser_connect".to_string(),
                        account_id: account_id.clone(),
                    },
                )
                .await
                {
                    return system_error_response(err);
                }
            }
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id,
                    event_type: "wallet.default.updated",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: "wallet-default",
                    result: "ok",
                    reason: if capsule_id == SYSTEM_CAPSULE_ID {
                        "Default wallet account updated through System"
                    } else {
                        "Default wallet account updated through Wallet"
                    },
                },
            );
            Json(system_wallet_accounts_summary(state, authority).await).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_app_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let authority = match require_wallet_app_launch_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let (wallet_accounts, wallet_approvals) = tokio::join!(
        system_wallet_accounts_summary(&state, &authority),
        system_wallet_approvals_summary(&state, &authority, true)
    );
    Json(serde_json::json!({
        "app": {
            "id": WALLET_CAPSULE_ID,
            "title": "Wallet",
        },
        "approval_methods": wallet_approval_methods_summary(&state.data_dir),
        "wallet_accounts": wallet_accounts,
        "wallet_approvals": wallet_approvals,
    }))
    .into_response()
}

fn wallet_approval_methods_summary(data_dir: &FsPath) -> serde_json::Value {
    let walletconnect_available =
        ensure_wallet_connector_configured(data_dir, WALLET_WALLETCONNECT_CAPSULE_ID).is_ok();
    serde_json::json!({
        "schema": "elastos.wallet.approval-methods/v1",
        "metamask": {
            "available": true,
            "connector_id": WALLET_METAMASK_CAPSULE_ID,
        },
        "unisat": {
            "available": true,
            "connector_id": WALLET_UNISAT_CAPSULE_ID,
        },
        "walletconnect": {
            "available": walletconnect_available,
            "connector_id": WALLET_WALLETCONNECT_CAPSULE_ID,
            "requires_pinned_config": !walletconnect_available,
        },
    })
}
