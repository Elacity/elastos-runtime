use super::super::*;
use super::payload::{
    payload_evm_address_bytes, payload_hex_bytes, payload_quantity_bytes, payload_str, payload_u64,
};
use super::typed_data::validate_eip712_typed_data_shape;
use elastos_auth::{normalize_evm_address, validate_evm_address};
use serde_json::Value;

pub(crate) fn validate_browser_personal_sign_payload(
    payload: &Value,
    account: &LinkedAccount,
) -> Result<(), String> {
    if payload.get("schema").and_then(Value::as_str)
        != Some("elastos.browser.wallet-signature-request/v1")
    {
        return Err("Browser wallet signature payload has unsupported schema".to_string());
    }
    if payload.get("method").and_then(Value::as_str) != Some("personal_sign") {
        return Err("Browser wallet signature method must be personal_sign".to_string());
    }
    let chain_namespace = payload
        .get("chain_namespace")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing chain_namespace".to_string())?;
    if !chain_namespaces_compatible(&account.chain_namespace, chain_namespace) {
        return Err("Browser wallet signature chain does not match account".to_string());
    }
    if !account.chain_namespace.starts_with("eip155:") || !chain_namespace.starts_with("eip155:") {
        return Err("Browser personal_sign requires an EVM account".to_string());
    }
    let address = payload
        .get("address")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing address".to_string())?;
    validate_evm_address(address)?;
    if normalize_evm_address(address) != normalize_evm_address(&account.address) {
        return Err("Browser wallet signature address does not match account".to_string());
    }
    let account_id = payload
        .get("account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing account_id".to_string())?;
    if account_id != account.account_id {
        return Err("Browser wallet signature account_id does not match account".to_string());
    }
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing message".to_string())?;
    if message.is_empty() || message.len() > MAX_APPROVAL_PAYLOAD_BYTES {
        return Err("Browser wallet signature message size is invalid".to_string());
    }
    if message.chars().any(char::is_control) {
        return Err("Browser wallet signature message contains control characters".to_string());
    }
    let page_url = payload
        .get("page_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing page_url".to_string())?;
    if !(page_url.starts_with("https://") || page_url.starts_with("http://")) {
        return Err("Browser wallet signature page_url must be http or https".to_string());
    }
    if payload
        .get("requires_wallet_approval")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("Browser wallet signature must require wallet approval".to_string());
    }
    Ok(())
}

pub(crate) fn validate_browser_typed_data_sign_payload(
    payload: &Value,
    account: &LinkedAccount,
) -> Result<(), String> {
    if payload.get("schema").and_then(Value::as_str)
        != Some("elastos.browser.wallet-signature-request/v1")
    {
        return Err("Browser typed-data payload has unsupported schema".to_string());
    }
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing method".to_string())?;
    if !matches!(
        method,
        "eth_signTypedData" | "eth_signTypedData_v3" | "eth_signTypedData_v4"
    ) {
        return Err("Browser typed-data method is unsupported".to_string());
    }
    let chain_namespace = payload
        .get("chain_namespace")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing chain_namespace".to_string())?;
    if !chain_namespaces_compatible(&account.chain_namespace, chain_namespace) {
        return Err("Browser typed-data chain does not match account".to_string());
    }
    if !account.chain_namespace.starts_with("eip155:") || !chain_namespace.starts_with("eip155:") {
        return Err("Browser typed-data signatures require an EVM account".to_string());
    }
    let address = payload
        .get("address")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing address".to_string())?;
    validate_evm_address(address)?;
    if normalize_evm_address(address) != normalize_evm_address(&account.address) {
        return Err("Browser typed-data address does not match account".to_string());
    }
    let account_id = payload
        .get("account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing account_id".to_string())?;
    if account_id != account.account_id {
        return Err("Browser typed-data account_id does not match account".to_string());
    }
    let canonical = payload
        .get("typed_data_canonical")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing canonical typed data".to_string())?;
    if canonical.is_empty() || canonical.len() > MAX_APPROVAL_PAYLOAD_BYTES {
        return Err("Browser typed-data payload size is invalid".to_string());
    }
    let typed_data: Value =
        serde_json::from_str(canonical).map_err(|err| format!("invalid typed data: {err}"))?;
    validate_eip712_typed_data_shape(&typed_data)?;
    let page_url = payload
        .get("page_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser typed-data payload missing page_url".to_string())?;
    if !(page_url.starts_with("https://") || page_url.starts_with("http://")) {
        return Err("Browser typed-data page_url must be http or https".to_string());
    }
    if payload
        .get("requires_wallet_approval")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("Browser typed-data signature must require wallet approval".to_string());
    }
    Ok(())
}

pub(crate) fn validate_eip155_transaction_intent_payload(
    payload: &Value,
    account: &LinkedAccount,
) -> Result<(), String> {
    if payload.get("schema").and_then(Value::as_str)
        != Some("elastos.chain.unsigned_transaction_intent/v1")
    {
        return Err("transaction intent payload has unsupported schema".to_string());
    }
    if payload.get("transaction_type").and_then(Value::as_str) != Some("eip155_legacy") {
        return Err("transaction intent payload must be eip155_legacy".to_string());
    }
    if payload.get("wallet_intent").and_then(Value::as_str) != Some("transaction_intent") {
        return Err("transaction intent payload has mismatched wallet_intent".to_string());
    }
    if payload
        .get("requires_wallet_approval")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("transaction intent must require wallet approval".to_string());
    }
    let chain_id = payload_u64(payload, "chain_id")?;
    if !account.chain_namespace.starts_with("eip155:") {
        return Err("transaction intent requires an EVM account".to_string());
    }
    if chain_id == 0 {
        return Err("transaction intent chain_id must be non-zero".to_string());
    }
    let from = payload_str(payload, "from")?;
    validate_evm_address(from)?;
    if normalize_evm_address(from) != normalize_evm_address(&account.address) {
        return Err("transaction intent from address does not match account".to_string());
    }
    payload_evm_address_bytes(payload, "to")?;
    payload_quantity_bytes(payload, "nonce")?;
    payload_quantity_bytes(payload, "gas_price")?;
    payload_quantity_bytes(payload, "gas_limit")?;
    payload_quantity_bytes(payload, "value")?;
    payload_hex_bytes(payload, "data")?;
    Ok(())
}
