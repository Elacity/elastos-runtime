use super::*;
use k256::ecdsa::SigningKey;

mod bitcoin;
mod evm;

pub(super) use bitcoin::*;
pub(super) use evm::*;

pub(super) fn account_id(chain_namespace: &str, address: &str) -> String {
    format!("wallet:{chain_namespace}:{address}")
}

pub(super) fn managed_address_for_signing_key(
    signing_key: &SigningKey,
    chain_namespace: &str,
) -> Result<String, String> {
    match managed_proof_type(chain_namespace)? {
        MANAGED_EVM_PROOF_TYPE => Ok(evm_address_for_signing_key(signing_key)),
        MANAGED_BTC_P2WPKH_PROOF_TYPE => btc_p2wpkh_address_for_signing_key(signing_key),
        _ => Err("unsupported managed wallet proof type".to_string()),
    }
}

pub(super) fn managed_signature_envelope(request: &WalletApprovalRequest) -> Value {
    json!({
        "schema": "elastos.wallet.managed_signature_payload/v1",
        "request_id": request.request_id.clone(),
        "principal_id": request.principal_id.clone(),
        "account_id": request.account_id.clone(),
        "chain_namespace": request.chain_namespace.clone(),
        "address": request.address.clone(),
        "intent": request.intent.clone(),
        "capsule_id": request.requested_by_actor.clone(),
        "resource": request.resource.clone(),
        "reason": request.reason.clone(),
        "payload_hash": request.payload_hash.clone(),
        "payload": request.payload.clone(),
    })
}

fn browser_account_access_attestation(request: &WalletApprovalRequest) -> Value {
    json!({
        "schema": "elastos.browser.account-access-attestation/v1",
        "request_id": request.request_id,
        "principal_id": request.principal_id,
        "account_id": request.account_id,
        "chain_namespace": request.chain_namespace,
        "address": request.address,
        "intent": request.intent,
        "requested_by_actor": request.requested_by_actor,
        "resource": request.resource,
        "reason": request.reason,
        "payload_hash": request.payload_hash,
        "payload": request.payload,
    })
}

pub(super) struct ManagedSignatureOutput {
    pub(super) kind: ManagedSignatureKind,
    pub(super) authority: String,
    pub(super) payload: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedSignatureKind {
    Message,
    Transaction,
}

pub(super) fn sign_managed_approval(
    signing_key: &SigningKey,
    request: &WalletApprovalRequest,
) -> Result<ManagedSignatureOutput, String> {
    if request.intent == "bitcoin_bip322_proof" {
        let (signature, payload) = sign_bip322_simple_p2wpkh_approval(signing_key, request)?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Message,
            authority: signature,
            payload,
        });
    }
    if request.chain_namespace.starts_with("bip122:") {
        return Err(
            "managed Bitcoin accounts only support bitcoin_bip322_proof signing".to_string(),
        );
    }
    if request.intent == "browser_account_access" {
        let payload = browser_account_access_attestation(request);
        let payload_bytes = serde_json::to_vec(&payload).map_err(|err| err.to_string())?;
        let signature = sign_evm_message(signing_key, &payload_bytes)?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Message,
            authority: signature,
            payload,
        });
    }
    if request.intent == "browser_personal_sign" {
        let (signature, payload) = sign_browser_personal_sign_approval(signing_key, request)?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Message,
            authority: signature,
            payload,
        });
    }
    if request.intent == "browser_typed_data_sign" {
        let (signature, payload) = sign_browser_typed_data_approval(signing_key, request)?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Message,
            authority: signature,
            payload,
        });
    }
    if request.intent == "transaction_intent" {
        let (signed_transaction, payload) = sign_eip155_legacy_transaction(signing_key, request)?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Transaction,
            authority: signed_transaction,
            payload,
        });
    }

    let envelope = managed_signature_envelope(request);
    let envelope_bytes = serde_json::to_vec(&envelope).map_err(|err| err.to_string())?;
    let signature = sign_evm_message(signing_key, &envelope_bytes)?;
    Ok(ManagedSignatureOutput {
        kind: ManagedSignatureKind::Message,
        authority: signature,
        payload: envelope,
    })
}

pub(super) fn external_wallet_handoff(request: &WalletApprovalRequest) -> Result<Value, String> {
    if request.intent == "transaction_intent" {
        let chain_id = payload_u64(&request.payload, "chain_id")?;
        let mut transaction = json!({
            "from": payload_str(&request.payload, "from")?,
            "to": payload_str(&request.payload, "to")?,
            "value": payload_str(&request.payload, "value")?,
            "data": payload_str(&request.payload, "data")?,
            "chainId": format!("0x{chain_id:x}"),
        });
        // gas / gasPrice / nonce are forwarded ONLY when the intent supplies them. When
        // omitted, the external wallet (MetaMask) estimates them — exactly PC2's EOA mint,
        // which sends only to/data/value/chainId. This avoids dictating a too-low gas limit
        // (out-of-gas reverts) or a wrong gas price (inflated fees) to the user's wallet.
        let tx_obj = transaction.as_object_mut().expect("transaction is an object");
        if let Ok(gas) = payload_str(&request.payload, "gas_limit") {
            tx_obj.insert("gas".to_string(), Value::String(gas.to_string()));
        }
        if let Ok(gas_price) = payload_str(&request.payload, "gas_price") {
            tx_obj.insert("gasPrice".to_string(), Value::String(gas_price.to_string()));
        }
        if let Ok(nonce) = payload_str(&request.payload, "nonce") {
            tx_obj.insert("nonce".to_string(), Value::String(nonce.to_string()));
        }
        return Ok(json!({
            "schema": "elastos.wallet.webconnect_handoff/v1",
            "request_id": request.request_id,
            "intent": request.intent,
            "payload_hash": request.payload_hash,
            "signer": request.address,
            "transaction": transaction,
            "status": "awaiting_wallet_transaction"
        }));
    }
    let message = external_signature_message(request)?;
    Ok(json!({
        "schema": "elastos.wallet.webconnect_handoff/v1",
        "request_id": request.request_id,
        "intent": request.intent,
        "payload_hash": request.payload_hash,
        "signer": request.address,
        "message": message,
        "signature_type": if request.intent == "bitcoin_bip322_proof" {
            bitcoin_signature_type_for_proof_type(&request.proof_type)
        } else {
            "personal_sign"
        },
        "status": "awaiting_wallet_signature"
    }))
}

pub(super) fn managed_signed_result(
    request: &WalletApprovalRequest,
    signed: &ManagedSignatureOutput,
) -> Option<Value> {
    if request.intent == "browser_account_access" {
        return Some(json!({
            "schema": "elastos.browser.account-access-result/v1",
            "request_id": request.request_id,
            "permission": "eth_accounts",
            "principal_id": request.payload.get("principal_id").cloned().unwrap_or(Value::Null),
            "session_id": request.payload.get("session_id").cloned().unwrap_or(Value::Null),
            "launch_id": request.payload.get("launch_id").cloned().unwrap_or(Value::Null),
            "proof_binding_id": request.payload.get("proof_binding_id").cloned().unwrap_or(Value::Null),
            "origin": request.payload.get("origin").cloned().unwrap_or(Value::Null),
            "page_url": request.payload.get("page_url").cloned().unwrap_or(Value::Null),
            "account_id": request.account_id,
            "requested_chain_namespace": request.payload.get("requested_chain_namespace").cloned().unwrap_or(Value::Null),
            "chain_namespaces": request.payload.get("chain_namespaces").cloned().unwrap_or(Value::Null),
            "address": request.address,
            "grant_expires_at": request.payload.get("grant_expires_at").cloned().unwrap_or(Value::Null),
            "payload_hash": request.payload_hash,
        }));
    }
    if request.intent == "browser_personal_sign" {
        return browser_personal_sign_result(request, &signed.authority);
    }
    if request.intent == "browser_typed_data_sign" {
        return browser_typed_data_sign_result(request, &signed.authority);
    }
    if request.intent == "transaction_intent" && signed.kind == ManagedSignatureKind::Transaction {
        return Some(json!({
            "schema": "elastos.wallet.signed-transaction-result/v1",
            "request_id": request.request_id,
            "method": "eth_sendTransaction",
            "signed_transaction": signed.authority,
            "transaction_hash": signed.payload.get("transaction_hash").cloned().unwrap_or(Value::Null),
            "signer": request.address,
            "chain_namespace": request.chain_namespace,
            "payload_hash": request.payload_hash,
            "page_url": request.payload.get("page_url").cloned().unwrap_or(Value::Null),
            "origin": request.payload.get("origin").cloned().unwrap_or(Value::Null),
        }));
    }
    None
}

pub(super) fn external_signature_message(
    request: &WalletApprovalRequest,
) -> Result<String, String> {
    // `ddrm_delegation_sign` (the connector-brokered dDRM delegation signature, minted by
    // `viewer_grant_sign::create_delegation_sign_request`) shares the EXACT same connector
    // `personal_sign` shape as `browser_personal_sign` — a `payload.message` string the connector
    // EIP-191 signs verbatim — so it must be extracted and hashed identically, not fall through to
    // the generic `ElastOS Wallet Approval...` canned message below (which would never match what
    // the wallet actually signed).
    if request.intent == "browser_personal_sign" || request.intent == "ddrm_delegation_sign" {
        return request
            .payload
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| "Browser wallet signature payload missing message".to_string());
    }
    if request.intent == "browser_typed_data_sign" {
        return request
            .payload
            .get("typed_data_canonical")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| "Browser typed-data payload missing canonical typed data".to_string());
    }
    if request.intent == "bitcoin_bip322_proof" {
        return request
            .payload
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| "Bitcoin BIP-322 payload missing message".to_string());
    }
    Ok(format!(
        "ElastOS Wallet Approval\n\nRequest: {}\nIntent: {}\nCapsule: {}\nResource: {}\nReason: {}\nPayload SHA-256: {}\nAccount: {}\nExpires At: {}",
        request.request_id,
        request.intent,
        request.requested_by_actor,
        request.resource,
        request.reason,
        request.payload_hash,
        request.address,
        request.expires_at
    ))
}
