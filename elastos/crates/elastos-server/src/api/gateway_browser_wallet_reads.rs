//! Runtime-mediated Browser wallet read helpers.

use super::*;

pub(super) async fn browser_wallet_read(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    input: BrowserWalletReadRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let request_id = browser_effect_request_id("chain-read", input.method.trim());
    append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type: "browser.chain_read.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            method: input.method.trim(),
            resource: &input.chain_namespace,
            page_url: &input.page_url,
            origin: input.origin.as_deref(),
            decision: "standing_read_policy",
        },
    )?;
    let result = browser_wallet_read_inner(state, context, &input).await;
    let (event_type, result_label, decision) = match &result {
        Ok(_) => (
            "browser.chain_read.completed",
            "allowed",
            "provider_mediated_typed_read",
        ),
        Err(_) => ("browser.chain_read.completed", "denied", "fail_closed"),
    };
    append_browser_effect_audit_or_500(
        &state.data_dir,
        BrowserEffectAuditInput {
            event_type,
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: result_label,
            method: input.method.trim(),
            resource: &input.chain_namespace,
            page_url: &input.page_url,
            origin: input.origin.as_deref(),
            decision,
        },
    )?;
    result
}

async fn browser_wallet_read_inner(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    input: &BrowserWalletReadRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let method = input.method.trim();
    let params = input.params.as_array().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser wallet read params must be an array".to_string(),
        )
    })?;
    if browser_url_to_stream_target(&input.page_url).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid browser page URL".to_string(),
        ));
    }
    let Some(network) = browser_chain_namespace_network(&input.chain_namespace) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser chain reads require a supported eip155 chain".to_string(),
        ));
    };
    let accounts = system_wallet_accounts_summary(state, &context.principal_id).await;
    if !accounts
        .accounts
        .iter()
        .any(browser_wallet_account_is_signable_evm)
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Browser chain read requires a linked EVM account for this Runtime principal"
                .to_string(),
        ));
    }
    let data = match method {
        "eth_blockNumber" => {
            let call = browser_provider_resource_call(
                "chain",
                "block_number",
                format!("elastos://chain/{network}/block_number"),
                serde_json::json!({ "network": network }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("block_number_hex")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider block_number response is missing block_number_hex"
                            .to_string(),
                    )
                })?
        }
        "eth_getBalance" => {
            let address = params
                .first()
                .and_then(|value| value.as_str())
                .or(input.address.as_deref())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_getBalance requires an address parameter".to_string(),
                    )
                })?;
            let block = params
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("latest");
            let call = browser_provider_resource_call(
                "chain",
                "balance",
                format!("elastos://chain/{network}/balance"),
                serde_json::json!({
                    "network": network,
                    "address": address,
                    "block": block,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("balance_hex")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider balance response is missing balance_hex".to_string(),
                    )
                })?
        }
        "eth_call" => {
            let call = params
                .first()
                .and_then(|value| value.as_object())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_call requires a call object".to_string(),
                    )
                })?;
            let to = call
                .get("to")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_call requires a to address".to_string(),
                    )
                })?;
            let data = call
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or("0x");
            if data.len() > 256 * 1024 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "eth_call data is too large for Browser chain reads".to_string(),
                ));
            }
            let block = params
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("latest");
            let call = browser_provider_resource_call(
                "chain",
                "contract_call",
                format!("elastos://chain/{network}/contract_call"),
                serde_json::json!({
                    "network": network,
                    "to": to,
                    "data": data,
                    "block": block,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("result")
                .cloned()
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider contract_call response is missing result".to_string(),
                    )
                })?
        }
        "eth_estimateGas" => {
            let tx = params
                .first()
                .and_then(|value| value.as_object())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_estimateGas requires a transaction object".to_string(),
                    )
                })?;
            let from = tx
                .get("from")
                .and_then(|value| value.as_str())
                .or(input.address.as_deref())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_estimateGas requires a from address".to_string(),
                    )
                })?;
            if !input
                .address
                .as_deref()
                .is_some_and(|address| address.eq_ignore_ascii_case(from))
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "eth_estimateGas from address must match selected Browser wallet account"
                        .to_string(),
                ));
            }
            let to = tx
                .get("to")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_estimateGas requires a to address".to_string(),
                    )
                })?;
            let value = tx
                .get("value")
                .and_then(|value| value.as_str())
                .unwrap_or("0x0");
            let data = tx
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or("0x");
            if data.len() > 256 * 1024 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "eth_estimateGas data is too large for Browser chain reads".to_string(),
                ));
            }
            let call = browser_provider_resource_call(
                "chain",
                "estimate_gas",
                format!("elastos://chain/{network}/estimate_gas"),
                serde_json::json!({
                    "network": network,
                    "from": from,
                    "to": to,
                    "value": value,
                    "data": data,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("gas_limit")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider estimate_gas response is missing gas_limit".to_string(),
                    )
                })?
        }
        "eth_getTransactionCount" => {
            let address = params
                .first()
                .and_then(|value| value.as_str())
                .or(input.address.as_deref())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_getTransactionCount requires an address parameter".to_string(),
                    )
                })?;
            let block = params
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("pending");
            let call = browser_provider_resource_call(
                "chain",
                "transaction_count",
                format!("elastos://chain/{network}/transaction_count"),
                serde_json::json!({
                    "network": network,
                    "address": address,
                    "block": block,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("nonce")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider transaction_count response is missing nonce".to_string(),
                    )
                })?
        }
        "eth_gasPrice" => {
            let call = browser_provider_resource_call(
                "chain",
                "gas_price",
                format!("elastos://chain/{network}/gas_price"),
                serde_json::json!({ "network": network }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("gas_price")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider gas_price response is missing gas_price".to_string(),
                    )
                })?
        }
        "eth_feeHistory" => {
            let block_count = browser_hex_quantity_param(params.first()).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "eth_feeHistory requires a block count".to_string(),
                )
            })?;
            let newest_block = params
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("latest");
            let reward_percentiles = params
                .get(2)
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let call = browser_provider_resource_call(
                "chain",
                "fee_history",
                format!("elastos://chain/{network}/fee_history"),
                serde_json::json!({
                    "network": network,
                    "block_count": block_count,
                    "newest_block": newest_block,
                    "reward_percentiles": reward_percentiles,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("history")
                .cloned()
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider fee_history response is missing history".to_string(),
                    )
                })?
        }
        "eth_getCode" => {
            let address = params
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_getCode requires an address parameter".to_string(),
                    )
                })?;
            let block = params
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("latest");
            let call = browser_provider_resource_call(
                "chain",
                "code",
                format!("elastos://chain/{network}/code"),
                serde_json::json!({
                    "network": network,
                    "address": address,
                    "block": block,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("code")
                .and_then(|value| value.as_str())
                .map(|value| serde_json::Value::String(value.to_string()))
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider code response is missing code".to_string(),
                    )
                })?
        }
        "eth_getLogs" => {
            let filter = params.first().cloned().ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "eth_getLogs requires a filter object".to_string(),
                )
            })?;
            let call = browser_provider_resource_call(
                "chain",
                "logs",
                format!("elastos://chain/{network}/logs"),
                serde_json::json!({
                    "network": network,
                    "filter": filter,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("logs")
                .cloned()
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider logs response is missing logs".to_string(),
                    )
                })?
        }
        "eth_getTransactionByHash" => {
            let hash = params
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_getTransactionByHash requires a transaction hash".to_string(),
                    )
                })?;
            let call = browser_provider_resource_call(
                "chain",
                "transaction",
                format!("elastos://chain/{network}/transaction"),
                serde_json::json!({
                    "network": network,
                    "hash": hash,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("transaction")
                .cloned()
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider transaction response is missing transaction".to_string(),
                    )
                })?
        }
        "eth_getTransactionReceipt" => {
            let hash = params
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        "eth_getTransactionReceipt requires a transaction hash".to_string(),
                    )
                })?;
            let call = browser_provider_resource_call(
                "chain",
                "receipt",
                format!("elastos://chain/{network}/receipt"),
                serde_json::json!({
                    "network": network,
                    "hash": hash,
                }),
            )?;
            let response = browser_provider_resource_response(state, call).await?;
            provider_response_data_or_bad_request(&response)?
                .get("receipt")
                .cloned()
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "chain provider receipt response is missing receipt".to_string(),
                    )
                })?
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{method} is not a supported Browser chain read"),
            ));
        }
    };
    Ok(serde_json::json!({
        "schema": "elastos.browser.wallet-read-result/v1",
        "method": method,
        "chain_namespace": input.chain_namespace,
        "result": data,
        "requires_approval": false,
        "authority": "runtime_chain_provider",
    }))
}

fn browser_hex_quantity_param(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(value)
            if value.starts_with("0x") && value[2..].chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            Some(value.to_string())
        }
        serde_json::Value::Number(value) => value.as_u64().map(|value| format!("0x{value:x}")),
        _ => None,
    }
}
