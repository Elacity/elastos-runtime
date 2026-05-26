use super::super::*;
use super::signature::{sign_evm_message, sign_evm_prehash};
use super::typed_data::eip712_payload_hash;
use super::validation::{
    validate_browser_personal_sign_payload, validate_browser_typed_data_sign_payload,
};
use k256::ecdsa::SigningKey;
use serde_json::{json, Value};

pub(crate) fn sign_browser_personal_sign_approval(
    signing_key: &SigningKey,
    request: &WalletApprovalRequest,
) -> Result<(String, Value), String> {
    validate_browser_personal_sign_payload(
        &request.payload,
        &LinkedAccount {
            account_id: request.account_id.clone(),
            principal_id: request.principal_id.clone(),
            proof_binding_id: request.proof_binding_id.clone(),
            chain_namespace: request.chain_namespace.clone(),
            address: request.address.clone(),
            proof_type: MANAGED_EVM_PROOF_TYPE.to_string(),
            connector_id: None,
            label: None,
            linked_at: 0,
            revoked_at: None,
        },
    )?;
    let message = request
        .payload
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser wallet signature payload missing message".to_string())?;
    let signature = sign_evm_message(signing_key, &browser_personal_sign_message_bytes(message)?)?;
    Ok((
        signature.clone(),
        browser_personal_sign_result(request, &signature).unwrap_or_else(|| json!({})),
    ))
}

pub(crate) fn browser_personal_sign_result(
    request: &WalletApprovalRequest,
    signature: &str,
) -> Option<Value> {
    if request.intent != "browser_personal_sign" {
        return None;
    }
    Some(json!({
        "schema": "elastos.browser.personal-sign-result/v1",
        "request_id": request.request_id,
        "method": "personal_sign",
        "signature": signature,
        "signer": request.address,
        "chain_namespace": request.chain_namespace,
        "page_url": request.payload.get("page_url").cloned().unwrap_or(Value::Null),
        "origin": request.payload.get("origin").cloned().unwrap_or(Value::Null),
        "payload_hash": request.payload_hash,
    }))
}

pub(crate) fn browser_personal_sign_message_bytes(message: &str) -> Result<Vec<u8>, String> {
    let Some(raw_hex) = message
        .strip_prefix("0x")
        .or_else(|| message.strip_prefix("0X"))
    else {
        return Ok(message.as_bytes().to_vec());
    };
    if raw_hex.len() % 2 != 0 || !raw_hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(message.as_bytes().to_vec());
    }
    hex::decode(raw_hex).map_err(|err| format!("invalid Browser personal_sign hex: {err}"))
}

pub(crate) fn sign_browser_typed_data_approval(
    signing_key: &SigningKey,
    request: &WalletApprovalRequest,
) -> Result<(String, Value), String> {
    validate_browser_typed_data_sign_payload(
        &request.payload,
        &LinkedAccount {
            account_id: request.account_id.clone(),
            principal_id: request.principal_id.clone(),
            proof_binding_id: request.proof_binding_id.clone(),
            chain_namespace: request.chain_namespace.clone(),
            address: request.address.clone(),
            proof_type: MANAGED_EVM_PROOF_TYPE.to_string(),
            connector_id: None,
            label: None,
            linked_at: 0,
            revoked_at: None,
        },
    )?;
    let hash = eip712_payload_hash(&request.payload)?;
    let signature = sign_evm_prehash(signing_key, &hash)?;
    Ok((
        signature.clone(),
        browser_typed_data_sign_result(request, &signature).unwrap_or_else(|| json!({})),
    ))
}

pub(crate) fn browser_typed_data_sign_result(
    request: &WalletApprovalRequest,
    signature: &str,
) -> Option<Value> {
    if request.intent != "browser_typed_data_sign" {
        return None;
    }
    Some(json!({
        "schema": "elastos.browser.typed-data-sign-result/v1",
        "request_id": request.request_id,
        "method": request.payload.get("method").cloned().unwrap_or_else(|| json!("eth_signTypedData_v4")),
        "signature": signature,
        "signer": request.address,
        "chain_namespace": request.chain_namespace,
        "page_url": request.payload.get("page_url").cloned().unwrap_or(Value::Null),
        "origin": request.payload.get("origin").cloned().unwrap_or(Value::Null),
        "payload_hash": request.payload_hash,
    }))
}
