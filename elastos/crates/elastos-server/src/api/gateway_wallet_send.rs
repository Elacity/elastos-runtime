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
    let step_up_request = serde_json::json!({
        "account_id": input.account_id,
        "chain_namespace": input.chain_namespace,
        "to": input.to,
        "amount": input.amount,
    });
    let step_up = match consume_or_recover_passkey_step_up_effect(
        &state.data_dir,
        &input.step_up_token,
        &launch,
        180,
        "wallet.send",
        &step_up_request,
    ) {
        Ok(step_up) => step_up,
        Err(err) => return system_error_response(err),
    };
    let effect_id = match runtime_transaction_effect_id(
        NATIVE_TRANSACTION_SOURCE,
        &authority,
        &serde_json::json!({
            "step_up_id": step_up.step_up_id,
            "request_sha256": step_up.request_sha256,
        }),
    ) {
        Ok(effect_id) => effect_id,
        Err(err) => return system_error_response(err),
    };
    match wallet_send_transaction(&state, &context, &authority, &input, &effect_id, &step_up).await
    {
        Ok(payload) => Json(payload).into_response(),
        Err((status, message)) => (status, message).into_response(),
    }
}

pub(in crate::api::gateway) async fn wallet_send_transaction(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    input: &WalletSendTransactionRequest,
    effect_id: &str,
    step_up: &PasskeyStepUpEffectIdentity,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let Some(network) = wallet_chain_namespace_network(&input.chain_namespace) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Wallet send currently supports EVM accounts on ESC and Base".to_string(),
        ));
    };
    validate_wallet_evm_address(&input.to, "to")?;
    let value = native_amount_to_hex_quantity(&input.amount, 18)?;
    let approval = if step_up.recovered {
        resume_runtime_native_transaction_approval(
            state,
            authority,
            effect_id,
            &step_up.step_up_id,
            &step_up.request_sha256,
        )
        .await?
    } else {
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

        ensure_runtime_transaction_approval(
            state,
            authority,
            RuntimeTransactionRequest {
                source: NATIVE_TRANSACTION_SOURCE,
                effect_id: effect_id.to_string(),
                request_sha256: step_up.request_sha256.clone(),
                account_id: account.account_id.clone(),
                address: account.address.clone(),
                chain_namespace: input.chain_namespace.clone(),
                network: network.to_string(),
                to: input.to.clone(),
                value,
                data: "0x".to_string(),
                approval_reason: format!(
                    "Wallet sends {} native units on {}",
                    input.amount, network
                ),
                metadata: serde_json::json!({}),
            },
        )
        .await?
    };
    let completion = complete_runtime_transaction_effect(
        state,
        authority,
        RuntimeTransactionLookup::EffectId(&approval.effect_id),
        Some(RuntimeManagedTransactionApproval {
            context,
            reason: "Approved in Wallet send flow",
            capsule_id: WALLET_CAPSULE_ID,
        }),
    )
    .await?;
    Ok(serde_json::json!({
        "schema": "elastos.wallet.send-transaction-result/v1",
        "request_id": completion.approval_request_id,
        "transaction_hash": completion.transaction_hash,
        "approval_request": completion.approval_request,
        "signed_result": completion.signed_result,
        "receipt": completion.receipt,
        "completion_status": if completion.completion_pending { "pending" } else { "complete" },
        "completion_error": completion.completion_error,
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
