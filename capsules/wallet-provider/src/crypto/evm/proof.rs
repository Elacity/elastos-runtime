use super::super::*;
use super::payload::hex_prefixed_bytes;
use elastos_auth::{
    ethereum_signed_message_hash, normalize_evm_address, validate_evm_address, AuthChallengeV1,
    SiweMessage,
};
use serde_json::Value;

pub(crate) fn validate_siwe_challenge_message(
    challenge: &AuthChallengeV1,
    parsed: &SiweMessage,
    message: &str,
    now: u64,
) -> Result<[u8; 32], String> {
    if challenge.schema != AuthChallengeV1::SCHEMA {
        return Err("unsupported auth challenge schema".to_string());
    }
    if challenge.expires_at <= now {
        return Err("auth challenge expired".to_string());
    }
    if parsed.domain != challenge.domain {
        return Err("SIWE domain does not match challenge".to_string());
    }
    if parsed.uri != challenge.uri {
        return Err("SIWE URI does not match challenge".to_string());
    }
    if parsed.version != "1" {
        return Err("unsupported SIWE version".to_string());
    }
    if parsed.chain_id != challenge.chain_id {
        return Err("SIWE chain ID does not match challenge".to_string());
    }
    if parsed.nonce != challenge.nonce {
        return Err("SIWE nonce does not match challenge".to_string());
    }
    if normalize_evm_address(&parsed.address) != normalize_evm_address(&challenge.address) {
        return Err("SIWE address does not match challenge".to_string());
    }
    if parsed.issued_at > now.saturating_add(300) {
        return Err("SIWE issued-at is in the future".to_string());
    }
    let Some(expires_at) = parsed.expires_at else {
        return Err("SIWE expiration-time is required".to_string());
    };
    if expires_at <= now {
        return Err("SIWE proof expired".to_string());
    }
    if expires_at > challenge.expires_at {
        return Err("SIWE proof outlives challenge".to_string());
    }
    if parsed.resources != challenge.resources {
        return Err("SIWE resources do not match challenge".to_string());
    }
    if !parsed
        .resources
        .iter()
        .any(|resource| resource == &challenge.challenge_resource())
    {
        return Err("SIWE resources missing challenge binding".to_string());
    }
    if message != challenge.siwe_message() {
        return Err("SIWE message does not match challenge".to_string());
    }
    Ok(ethereum_signed_message_hash(message.as_bytes()))
}

pub(crate) fn validate_erc1271_chain_proof(
    proof: &Value,
    parsed: &SiweMessage,
    message_hash: &[u8; 32],
    signature: &str,
) -> Result<(), String> {
    if proof.get("schema").and_then(Value::as_str) != Some("elastos.chain.erc1271_proof/v1") {
        return Err("unsupported ERC-1271 proof schema".to_string());
    }
    if proof.get("valid").and_then(Value::as_bool) != Some(true) {
        return Err("ERC-1271 signature was not accepted by the contract".to_string());
    }
    let chain_id = proof
        .get("chain_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "ERC-1271 proof missing chain_id".to_string())?;
    if chain_id != parsed.chain_id {
        return Err("ERC-1271 proof chain_id does not match SIWE message".to_string());
    }
    let contract = proof
        .get("contract")
        .and_then(Value::as_str)
        .ok_or_else(|| "ERC-1271 proof missing contract".to_string())?;
    validate_evm_address(contract)?;
    if normalize_evm_address(contract) != parsed.address {
        return Err("ERC-1271 proof contract does not match SIWE address".to_string());
    }
    let expected_message_hash = format!("0x{}", hex::encode(message_hash));
    if proof.get("message_hash").and_then(Value::as_str) != Some(expected_message_hash.as_str()) {
        return Err("ERC-1271 proof message_hash mismatch".to_string());
    }
    let signature_bytes = hex_prefixed_bytes(signature, None, "signature")?;
    if signature_bytes.is_empty() {
        return Err("signature must not be empty".to_string());
    }
    let expected_signature_hash = bytes_hash(&signature_bytes);
    if proof.get("signature_hash").and_then(Value::as_str) != Some(expected_signature_hash.as_str())
    {
        return Err("ERC-1271 proof signature_hash mismatch".to_string());
    }
    Ok(())
}
