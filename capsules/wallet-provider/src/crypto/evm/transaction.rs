use super::super::*;
use super::payload::{
    payload_evm_address_bytes, payload_hex_bytes, payload_quantity_bytes, payload_u64,
    trim_integer_bytes,
};
use super::validation::validate_eip155_transaction_intent_payload;
use k256::ecdsa::SigningKey;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

pub(crate) fn external_transaction_result(
    request: &WalletApprovalRequest,
    transaction_hash: &str,
) -> Value {
    json!({
        "schema": "elastos.wallet.external-transaction-result/v1",
        "request_id": request.request_id,
        "method": "eth_sendTransaction",
        "transaction_hash": transaction_hash,
        "signer": request.address,
        "chain_namespace": request.chain_namespace,
        "payload_hash": request.payload_hash,
    })
}

pub(crate) fn sign_eip155_legacy_transaction(
    signing_key: &SigningKey,
    request: &WalletApprovalRequest,
) -> Result<(String, Value), String> {
    validate_eip155_transaction_intent_payload(
        &request.payload,
        &LinkedAccount {
            account_id: request.account_id.clone(),
            principal_id: request.principal_id.clone(),
            proof_binding_id: request.proof_binding_id.clone(),
            chain_namespace: request.chain_namespace.clone(),
            address: request.address.clone(),
            proof_type: request.proof_type.clone(),
            connector_id: request.connector_id.clone(),
            label: None,
            linked_at: request.created_at,
            revoked_at: None,
        },
    )?;
    let chain_id = payload_u64(&request.payload, "chain_id")?;
    let nonce = payload_quantity_bytes(&request.payload, "nonce")?;
    let gas_price = payload_quantity_bytes(&request.payload, "gas_price")?;
    let gas_limit = payload_quantity_bytes(&request.payload, "gas_limit")?;
    let to = payload_evm_address_bytes(&request.payload, "to")?;
    let value = payload_quantity_bytes(&request.payload, "value")?;
    let data = payload_hex_bytes(&request.payload, "data")?;
    let signing_items = vec![
        rlp_bytes(&nonce),
        rlp_bytes(&gas_price),
        rlp_bytes(&gas_limit),
        rlp_bytes(&to),
        rlp_bytes(&value),
        rlp_bytes(&data),
        rlp_u64(chain_id),
        rlp_bytes(&[]),
        rlp_bytes(&[]),
    ];
    let signing_payload = rlp_list(&signing_items);
    let signing_hash = Keccak256::digest(&signing_payload);
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&signing_hash)
        .map_err(|err| err.to_string())?;
    let signature_bytes = signature.to_bytes();
    let v = chain_id
        .checked_mul(2)
        .and_then(|value| value.checked_add(35))
        .and_then(|value| value.checked_add(u64::from(recovery_id.to_byte())))
        .ok_or_else(|| "transaction chain_id is too large".to_string())?;
    let signed_items = vec![
        rlp_bytes(&nonce),
        rlp_bytes(&gas_price),
        rlp_bytes(&gas_limit),
        rlp_bytes(&to),
        rlp_bytes(&value),
        rlp_bytes(&data),
        rlp_u64(v),
        rlp_bytes(trim_integer_bytes(&signature_bytes[..32])),
        rlp_bytes(trim_integer_bytes(&signature_bytes[32..])),
    ];
    let signed_transaction_bytes = rlp_list(&signed_items);
    let signed_transaction = format!("0x{}", hex::encode(&signed_transaction_bytes));
    let transaction_hash = format!(
        "0x{}",
        hex::encode(Keccak256::digest(&signed_transaction_bytes))
    );
    Ok((
        signed_transaction,
        json!({
            "schema": "elastos.wallet.signed_transaction/v1",
            "transaction_type": "eip155_legacy",
            "request_id": request.request_id.clone(),
            "chain_namespace": request.chain_namespace.clone(),
            "signer": request.address.clone(),
            "payload_hash": request.payload_hash.clone(),
            "transaction_hash": transaction_hash,
        }),
    ))
}

pub(crate) fn rlp_u64(value: u64) -> Vec<u8> {
    if value == 0 {
        return rlp_bytes(&[]);
    }
    rlp_bytes(trim_integer_bytes(&value.to_be_bytes()))
}

pub(crate) fn rlp_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }
    let mut encoded = rlp_length_prefix(bytes.len(), 0x80);
    encoded.extend_from_slice(bytes);
    encoded
}

pub(crate) fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload_len = items.iter().map(Vec::len).sum::<usize>();
    let mut encoded = rlp_length_prefix(payload_len, 0xc0);
    for item in items {
        encoded.extend_from_slice(item);
    }
    encoded
}

pub(crate) fn rlp_length_prefix(len: usize, offset: u8) -> Vec<u8> {
    if len < 56 {
        return vec![offset + len as u8];
    }
    let raw_len = len.to_be_bytes();
    let len_bytes = trim_integer_bytes(&raw_len);
    let mut encoded = vec![offset + 55 + len_bytes.len() as u8];
    encoded.extend_from_slice(len_bytes);
    encoded
}
