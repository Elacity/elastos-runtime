use std::future::Future;

use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{
    CanonicalContract, Digest32, NodePublicKey, RightsEvaluationEvidenceV1,
    RuntimeOperationIssuerKeyV1, SignedRuntimeReleaseOperationV1,
};
use elastos_protected_content_provider_contracts::{
    RightsProviderRequestV1, RightsProviderResponseV1, ValidatedRightsProviderRequestV1,
};
use serde_json::Value;

use crate::{evaluate_validated_rights_with_evidence_at, RightsEvaluationErrorV1};

pub const CHAIN_PROVIDER_ID: &str = "chain";
pub const CHAIN_RIGHTS_EVIDENCE_OP: &str = "protected_content_rights_evidence";
const CHAIN_RIGHTS_EVIDENCE_SCHEMA: &str = "elastos.chain.protected-content-rights-evidence/v1";

const ALLOWED_CHAIN_EVIDENCE_FIELDS: &[&str] = &[
    "schema",
    "chain_id",
    "finalized_block_number",
    "finalized_block_hash",
    "rights_evaluation_evidence",
    "rights_evaluation_evidence_hash",
];

pub fn chain_rights_evidence_request(
    signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
) -> Result<Value, RightsEvaluationErrorV1> {
    let hex = format!(
        "0x{}",
        hex::encode(signed_runtime_release_operation.canonical_bytes()?)
    );
    let request = serde_json::json!({
        "op": CHAIN_RIGHTS_EVIDENCE_OP,
        "signed_runtime_release_operation": hex,
    });
    reject_injected_chain_request_fields(&request)?;
    Ok(request)
}

pub fn parse_chain_rights_evidence_data(
    data: &Value,
) -> Result<RightsEvaluationEvidenceV1, RightsEvaluationErrorV1> {
    let object = data
        .as_object()
        .ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
    for key in object.keys() {
        if !ALLOWED_CHAIN_EVIDENCE_FIELDS.contains(&key.as_str()) {
            return Err(RightsEvaluationErrorV1::ChainEvidence);
        }
    }
    if object.get("schema").and_then(Value::as_str) != Some(CHAIN_RIGHTS_EVIDENCE_SCHEMA) {
        return Err(RightsEvaluationErrorV1::ChainEvidence);
    }
    let evidence_hex = object
        .get("rights_evaluation_evidence")
        .and_then(Value::as_str)
        .ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
    let evidence_bytes =
        decode_0x_hex(evidence_hex).ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
    let evidence = RightsEvaluationEvidenceV1::from_canonical_bytes(&evidence_bytes)
        .map_err(|_| RightsEvaluationErrorV1::ChainEvidence)?;
    if let Some(chain_id) = optional_u64(object.get("chain_id"))? {
        if chain_id != evidence.observed_chain_id() {
            return Err(RightsEvaluationErrorV1::ChainEvidence);
        }
    }
    if let Some(finalized_block_number) = optional_u64(object.get("finalized_block_number"))? {
        if finalized_block_number != evidence.finalized_block_number() {
            return Err(RightsEvaluationErrorV1::ChainEvidence);
        }
    }
    if let Some(finalized_block_hash) = optional_digest32(object.get("finalized_block_hash"))? {
        if finalized_block_hash != evidence.finalized_block_hash() {
            return Err(RightsEvaluationErrorV1::ChainEvidence);
        }
    }
    if let Some(evidence_hash) = optional_digest32(object.get("rights_evaluation_evidence_hash"))? {
        if evidence_hash
            != evidence
                .canonical_hash()
                .map_err(|_| RightsEvaluationErrorV1::ChainEvidence)?
        {
            return Err(RightsEvaluationErrorV1::ChainEvidence);
        }
    }
    Ok(evidence)
}

pub async fn evaluate_rights_via_chain<F, Fut>(
    node_signing_key: SigningKey,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    request: &RightsProviderRequestV1,
    now_unix_seconds: u64,
    invoke_chain: F,
) -> Result<RightsProviderResponseV1, RightsEvaluationErrorV1>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = Result<Value, RightsEvaluationErrorV1>>,
{
    let node_public_key = NodePublicKey::new(node_signing_key.verifying_key().to_bytes())
        .map_err(|_| RightsEvaluationErrorV1::InvalidNodeSigningKey)?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RightsEvaluationErrorV1::Contract)?;
    let validated = ValidatedRightsProviderRequestV1::decode_and_validate_at(
        &request_bytes,
        expected_runtime_issuer,
        now_unix_seconds,
    )
    .map_err(|_| RightsEvaluationErrorV1::Contract)?;
    if validated.selected_node_public_key() != node_public_key {
        return Err(RightsEvaluationErrorV1::WrongSelectedNode);
    }
    let chain_request =
        chain_rights_evidence_request(&request.signed_runtime_release_operation()?)?;
    let chain_data = invoke_chain(chain_request).await?;
    let evidence = parse_chain_rights_evidence_data(&chain_data)?;
    evaluate_validated_rights_with_evidence_at(
        &node_signing_key,
        &validated,
        evidence,
        now_unix_seconds,
    )
}

fn reject_injected_chain_request_fields(request: &Value) -> Result<(), RightsEvaluationErrorV1> {
    let object = request
        .as_object()
        .ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
    if object.len() != 2
        || object.get("op").and_then(Value::as_str) != Some(CHAIN_RIGHTS_EVIDENCE_OP)
        || !object
            .get("signed_runtime_release_operation")
            .and_then(Value::as_str)
            .is_some_and(|value| value.starts_with("0x"))
    {
        return Err(RightsEvaluationErrorV1::ChainEvidence);
    }
    Ok(())
}

fn optional_u64(value: Option<&Value>) -> Result<Option<u64>, RightsEvaluationErrorV1> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Err(RightsEvaluationErrorV1::ChainEvidence),
        Some(value) => value
            .as_u64()
            .ok_or(RightsEvaluationErrorV1::ChainEvidence)
            .map(Some),
    }
}

fn optional_digest32(value: Option<&Value>) -> Result<Option<Digest32>, RightsEvaluationErrorV1> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Err(RightsEvaluationErrorV1::ChainEvidence),
        Some(value) => {
            let hex = value
                .as_str()
                .ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
            let bytes = decode_0x_hex(hex).ok_or(RightsEvaluationErrorV1::ChainEvidence)?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| RightsEvaluationErrorV1::ChainEvidence)?;
            Ok(Some(Digest32::new(bytes)))
        }
    }
}

fn decode_0x_hex(value: &str) -> Option<Vec<u8>> {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(hex).ok()
}
