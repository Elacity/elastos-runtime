//! Wallet-owned native send flow and chain-provider broadcast helpers.

use super::*;

pub(in crate::api::gateway) async fn wallet_app_send_transaction(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<WalletSendTransactionRequest>,
) -> Response {
    let launch =
        match require_home_launch_token_binding(&state.data_dir, &headers, &[WALLET_CAPSULE_ID]) {
            Ok(launch) => launch,
            Err(err) => return system_error_response(err),
        };
    let authority = match runtime_wallet_authority(&launch) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    let context = launch.context.clone();
    if let Err(err) = consume_passkey_step_up_token(
        &state.data_dir,
        &input.step_up_token,
        &launch,
        180,
        "wallet.send",
        &serde_json::json!({
            "account_id": input.account_id,
            "chain_namespace": input.chain_namespace,
            "to": input.to,
            "amount": input.amount,
        }),
    ) {
        return system_error_response(err);
    }
    let audit_id = wallet_send_request_id(&input.account_id);
    let _ = append_wallet_approval_audit(
        &state.data_dir,
        WalletApprovalAuditInput {
            capsule_id: WALLET_CAPSULE_ID,
            event_type: "wallet.transaction.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &audit_id,
            result: "requested",
            reason: "Wallet requested a native EVM transaction send",
        },
    );
    match wallet_send_transaction(&state, &context, &authority, &input, &audit_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => {
            let _ = append_wallet_approval_audit(
                &state.data_dir,
                WalletApprovalAuditInput {
                    capsule_id: WALLET_CAPSULE_ID,
                    event_type: "wallet.transaction.failed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: &audit_id,
                    result: "failed",
                    reason: &message,
                },
            );
            (status, message).into_response()
        }
    }
}

pub(in crate::api::gateway) async fn wallet_send_transaction(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    input: &WalletSendTransactionRequest,
    audit_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let Some(network) = wallet_chain_namespace_network(&input.chain_namespace) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Wallet send currently supports EVM accounts on ESC and Base".to_string(),
        ));
    };
    validate_wallet_evm_address(&input.to, "to")?;
    let value = native_amount_to_hex_quantity(&input.amount, 18)?;
    let accounts = system_wallet_accounts_summary(state, authority).await;
    let Some(account) = accounts.accounts.iter().find(|account| {
        account.account_id == input.account_id
            && account.chain_namespace.starts_with("eip155:")
            && input.chain_namespace.starts_with("eip155:")
    }) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Wallet send account is not linked to this Runtime principal".to_string(),
        ));
    };
    if !account.signing_available || !is_managed_wallet_proof_type(&account.proof_type) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Wallet send currently requires a passkey-managed EVM account".to_string(),
        ));
    }
    validate_wallet_evm_address(&account.address, "from")?;

    let intent = wallet_chain_provider_data(
        state,
        serde_json::json!({
            "op": "prepare_transaction",
            "network": network,
            "from": account.address,
            "to": input.to,
            "value": value,
            "data": "0x",
        }),
    )
    .await?;
    if intent.get("schema").and_then(|value| value.as_str())
        != Some("elastos.chain.unsigned_transaction_intent/v1")
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned an unsupported transaction intent".to_string(),
        ));
    }

    let chain_broadcast_resource = format!("elastos://chain/{network}/broadcast_transaction");
    let request_data = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::RequestApproval {
            account_id: account.account_id.clone(),
            chain_namespace: input.chain_namespace.clone(),
            intent: "transaction_intent".to_string(),
            resource: chain_broadcast_resource,
            reason: format!("Wallet sends {} native units on {}", input.amount, network),
            payload: intent,
            expires_at: now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    let approval_request = request_data
        .get("approval_request")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider returned an invalid approval response".to_string(),
            )
        })?;
    let request_id = approval_request
        .get("request_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider approval response is missing request id".to_string(),
            )
        })?;
    let outcome = approve_managed_wallet_request(
        state,
        &state.data_dir,
        context,
        authority,
        request_id,
        "Approved in Wallet send flow",
        WALLET_CAPSULE_ID,
    )
    .await
    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    let signed_transaction = outcome.signed_transaction.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "built-in wallet did not return a signed transaction".to_string(),
        )
    })?;
    let receipt = wallet_chain_provider_data(
        state,
        serde_json::json!({
            "op": "broadcast_transaction",
            "network": network,
            "signed_transaction": signed_transaction,
        }),
    )
    .await?;
    let transaction_hash = receipt
        .get("transaction_hash")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "chain provider broadcast receipt is missing transaction hash".to_string(),
            )
        })?;
    let recorded_approval = crate::api::auth_gateway::wallet_provider_data(
        state,
        serde_json::json!({
            "op": "record_transaction_hash",
            "principal_id": context.principal_id,
            "request_id": request_id,
            "transaction_hash": transaction_hash,
        }),
    )
    .await
    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    let completed_approval_request = recorded_approval
        .get("approval_request")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| approval_request.clone());
    append_wallet_approval_audit(
        &state.data_dir,
        WalletApprovalAuditInput {
            capsule_id: WALLET_CAPSULE_ID,
            event_type: "wallet.transaction.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: audit_id,
            result: "completed",
            reason: "Wallet signed and broadcasted a native EVM transaction",
        },
    )
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(serde_json::json!({
        "schema": "elastos.wallet.send-transaction-result/v1",
        "request_id": request_id,
        "transaction_hash": transaction_hash,
        "approval_request": completed_approval_request,
        "signed_result": recorded_approval
            .get("approval_request")
            .and_then(|value| value.get("signed_result"))
            .cloned()
            .or(outcome.signed_result),
        "receipt": receipt,
    }))
}

pub(in crate::api::gateway) fn wallet_chain_namespace_network(
    chain_namespace: &str,
) -> Option<&'static str> {
    match chain_namespace {
        "eip155:20" => Some("esc-mainnet"),
        "eip155:8453" => Some("base-mainnet"),
        _ => None,
    }
}

pub(in crate::api::gateway) fn wallet_send_request_id(account_id: &str) -> String {
    let safe_account: String = account_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == ':' || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("wallet-send:{safe_account}:{timestamp}")
}

pub(in crate::api::gateway) fn validate_wallet_evm_address(
    address: &str,
    label: &str,
) -> Result<(), (StatusCode, String)> {
    let raw = address.strip_prefix("0x").ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("{label} address must start with 0x"),
        )
    })?;
    if raw.len() != 40 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{label} address must be a 20-byte EVM address"),
        ));
    }
    Ok(())
}

pub(in crate::api::gateway) fn native_amount_to_hex_quantity(
    amount: &str,
    decimals: u32,
) -> Result<String, (StatusCode, String)> {
    let value = amount.trim();
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return Err((
            StatusCode::BAD_REQUEST,
            "amount must be a positive decimal value".to_string(),
        ));
    }
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() > 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            "amount must be a decimal value".to_string(),
        ));
    }
    let whole = parts[0];
    let fraction = parts.get(1).copied().unwrap_or("");
    if (whole.is_empty() && fraction.is_empty())
        || !whole.chars().all(|ch| ch.is_ascii_digit())
        || !fraction.chars().all(|ch| ch.is_ascii_digit())
        || fraction.len() > decimals as usize
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("amount supports at most {decimals} decimal places"),
        ));
    }
    let scale = 10_u128.checked_pow(decimals).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "amount precision is unsupported".to_string(),
        )
    })?;
    let whole_value = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<u128>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "amount is too large".to_string()))?
    };
    let fraction_padded = format!("{fraction:0<width$}", width = decimals as usize);
    let fraction_value = if fraction_padded.is_empty() {
        0
    } else {
        fraction_padded
            .parse::<u128>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "amount is too precise".to_string()))?
    };
    let raw = whole_value
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction_value))
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "amount is too large".to_string()))?;
    if raw == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "amount must be greater than zero".to_string(),
        ));
    }
    Ok(format!("0x{raw:x}"))
}

pub(in crate::api::gateway) async fn wallet_chain_provider_data(
    state: &GatewayState,
    request: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let registry = state.provider_registry.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider unavailable".to_string(),
        )
    })?;
    let response = registry.send_raw("chain", &request).await.map_err(|err| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("chain provider unavailable: {err}"),
        )
    })?;
    if let Some(message) = gateway_browser::provider_response_error_message(&response) {
        return Err((StatusCode::BAD_REQUEST, message));
    }
    gateway_browser::provider_response_data(&response).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "chain provider returned malformed response".to_string(),
        )
    })
}

pub(in crate::api::gateway) fn is_managed_wallet_proof_type(proof_type: &str) -> bool {
    matches!(proof_type, "managed_evm" | "managed_btc_p2wpkh")
}
