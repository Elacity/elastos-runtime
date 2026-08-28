use super::*;
use k256::ecdsa::SigningKey;

mod bitcoin;
mod evm;

pub(super) use bitcoin::*;
pub(super) use evm::*;

const PROTECTED_CONTENT_RIGHTS_APPROVAL_PAYLOAD_SCHEMA: &str =
    "elastos.wallet.protected-content-rights-approval/v1";

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
    if request.intent == PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT {
        let (rights_request, canonical_bytes) =
            protected_content_rights_request_from_payload(&request.payload)?;
        let signature = sign_evm_message(signing_key, &canonical_bytes)?;
        let signed_request = WalletSignedRightsRequestV1::new(
            rights_request,
            canonicalize_evm_signature(&signature)
                .map_err(|err| format!("invalid managed rights signature: {err}"))?,
        )
        .map_err(|err| err.to_string())?;
        let payload = serde_json::to_value(
            ProtectedContentRightsSignatureResultV1::new(
                request.account_id.clone(),
                request.address.clone(),
                hex::encode(
                    signed_request
                        .canonical_bytes()
                        .map_err(|err| err.to_string())?,
                ),
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
        return Ok(ManagedSignatureOutput {
            kind: ManagedSignatureKind::Message,
            authority: signature,
            payload,
        });
    }
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
        let transaction = json!({
            "from": payload_str(&request.payload, "from")?,
            "to": payload_str(&request.payload, "to")?,
            "value": payload_str(&request.payload, "value")?,
            "data": payload_str(&request.payload, "data")?,
            "gas": payload_str(&request.payload, "gas_limit")?,
            "gasPrice": payload_str(&request.payload, "gas_price")?,
            "nonce": payload_str(&request.payload, "nonce")?,
            "chainId": format!("0x{chain_id:x}"),
        });
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
    if request.intent == PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT {
        let (_, canonical_bytes) = protected_content_rights_request_from_payload(&request.payload)?;
        return Ok(json!({
            "schema": "elastos.wallet.webconnect_handoff/v1",
            "request_id": request.request_id,
            "intent": request.intent,
            "payload_hash": request.payload_hash,
            "signer": request.address,
            "message": format!("0x{}", hex::encode(canonical_bytes)),
            "signature_type": "personal_sign",
            "status": "awaiting_wallet_signature"
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
    if request.intent == PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT {
        return Some(signed.payload.clone());
    }
    None
}

pub(super) fn protected_content_rights_approval_payload(
    canonical_rights_request_hex: &str,
) -> Value {
    json!({
        "schema": PROTECTED_CONTENT_RIGHTS_APPROVAL_PAYLOAD_SCHEMA,
        "canonical_rights_request_hex": canonical_rights_request_hex,
    })
}

pub(super) fn protected_content_rights_request_from_hex(
    canonical_rights_request_hex: &str,
) -> Result<(RightsRequestV1, Vec<u8>), String> {
    let canonical_rights_request_hex = canonical_rights_request_hex.trim();
    if canonical_rights_request_hex.is_empty() {
        return Err("protected-content rights request hex is required".to_string());
    }
    if canonical_rights_request_hex.len() > MAX_PROTECTED_CONTENT_RIGHTS_WIRE_BYTES * 2 {
        return Err(format!(
            "protected-content rights request exceeds the Wallet wire limit of {MAX_PROTECTED_CONTENT_RIGHTS_WIRE_BYTES} bytes"
        ));
    }
    if !canonical_rights_request_hex.len().is_multiple_of(2)
        || !canonical_rights_request_hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(
            "protected-content rights request must be lowercase hex without a prefix".to_string(),
        );
    }
    let canonical_bytes = hex::decode(canonical_rights_request_hex)
        .map_err(|err| format!("invalid protected-content rights request hex: {err}"))?;
    if canonical_bytes.is_empty() {
        return Err("protected-content rights request hex is required".to_string());
    }
    if canonical_bytes.len() > MAX_PROTECTED_CONTENT_RIGHTS_WIRE_BYTES {
        return Err(format!(
            "protected-content rights request exceeds the Wallet wire limit of {MAX_PROTECTED_CONTENT_RIGHTS_WIRE_BYTES} bytes"
        ));
    }
    let request = RightsRequestV1::from_canonical_bytes(&canonical_bytes)
        .map_err(|err| format!("invalid protected-content rights request: {err}"))?;
    Ok((request, canonical_bytes))
}

pub(super) fn protected_content_rights_request_from_payload(
    payload: &Value,
) -> Result<(RightsRequestV1, Vec<u8>), String> {
    let object = payload.as_object().ok_or_else(|| {
        "protected-content rights approval payload must be a JSON object".to_string()
    })?;
    if object.len() != 2
        || !object.contains_key("schema")
        || !object.contains_key("canonical_rights_request_hex")
    {
        return Err("protected-content rights approval payload has unsupported fields".to_string());
    }
    if object.get("schema").and_then(Value::as_str)
        != Some(PROTECTED_CONTENT_RIGHTS_APPROVAL_PAYLOAD_SCHEMA)
    {
        return Err("protected-content rights approval payload has unsupported schema".to_string());
    }
    let canonical_rights_request_hex = object
        .get("canonical_rights_request_hex")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "protected-content rights approval payload missing canonical_rights_request_hex"
                .to_string()
        })?;
    protected_content_rights_request_from_hex(canonical_rights_request_hex)
}

pub(super) fn validate_protected_content_rights_request_account(
    rights_request: &RightsRequestV1,
    account: &LinkedAccount,
    now: u64,
) -> Result<(), String> {
    if !account.chain_namespace.starts_with("eip155:") {
        return Err("protected-content rights signing requires an EVM account".to_string());
    }
    if now.saturating_add(RIGHTS_CLOCK_SKEW_SECS) < rights_request.issued_at() {
        return Err("protected-content rights request is not yet valid".to_string());
    }
    if now >= rights_request.expires_at() {
        return Err("protected-content rights request expired".to_string());
    }
    let bound_wallet = wallet_address_hex(rights_request.binding().wallet());
    if normalize_evm_address(&bound_wallet) != normalize_evm_address(&account.address) {
        return Err(
            "protected-content rights request Wallet does not match the selected account"
                .to_string(),
        );
    }
    Ok(())
}

fn wallet_address_hex(wallet: WalletAddress) -> String {
    format!("0x{}", hex::encode(wallet.as_bytes()))
}

pub(super) fn validate_protected_content_rights_result_for_request(
    result: &ProtectedContentRightsSignatureResultV1,
    request: &WalletApprovalRequest,
) -> Result<(), String> {
    result.validate().map_err(|err| err.to_string())?;
    if result.account_id != request.account_id {
        return Err(
            "protected-content rights signature result account_id does not match the approval"
                .to_string(),
        );
    }
    if !result.signer.eq_ignore_ascii_case(&request.address) {
        return Err(
            "protected-content rights signature result signer does not match the approval"
                .to_string(),
        );
    }
    let signed_bytes = hex::decode(&result.wallet_signed_rights_request_hex)
        .map_err(|err| format!("invalid stored protected-content rights result hex: {err}"))?;
    let signed = WalletSignedRightsRequestV1::from_canonical_bytes(&signed_bytes)
        .map_err(|err| format!("invalid stored protected-content signed request: {err}"))?;
    let (expected_request, expected_bytes) =
        protected_content_rights_request_from_payload(&request.payload)?;
    if signed.request() != &expected_request {
        return Err(
            "protected-content rights signed request does not match the approved request"
                .to_string(),
        );
    }
    if !result
        .signer
        .eq_ignore_ascii_case(&wallet_address_hex(signed.request().binding().wallet()))
    {
        return Err(
            "protected-content rights signature result signer does not match the bound Wallet"
                .to_string(),
        );
    }
    if signed
        .request()
        .canonical_bytes()
        .map_err(|err| err.to_string())?
        != expected_bytes
    {
        return Err(
            "protected-content rights signed request bytes do not match the approved request"
                .to_string(),
        );
    }
    let mut replay_check = ProtectedContentRightsResultReplayCheck;
    signed
        .verify(
            &RightsVerificationContextV1::new(
                expected_request.binding().clone(),
                expected_request.action(),
                expected_request.recipient().clone(),
                expected_request.issued_at(),
            ),
            &mut replay_check,
        )
        .map_err(|err| format!("invalid stored protected-content signed request: {err}"))?;
    Ok(())
}

struct ProtectedContentRightsResultReplayCheck;

impl AtomicReplayClaimer for ProtectedContentRightsResultReplayCheck {
    fn claim(
        &mut self,
        _key: ReplayClaimKeyV1,
        _expires_at: u64,
        _now: u64,
    ) -> Result<(), ReplayClaimError> {
        Ok(())
    }
}

pub(super) fn external_signature_message(
    request: &WalletApprovalRequest,
) -> Result<String, String> {
    if request.intent == "browser_personal_sign" {
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
