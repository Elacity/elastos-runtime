use super::*;
use ed25519_dalek::{Signer as _, SigningKey};
use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    CanonicalContract, ContentAccessIdV1, CustodyApprovedSuitesV1,
    CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeManifestV1, CustodyEnvelopeV1,
    CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
    CustodyPoolIdentityV1, Digest32, EncryptedContentIdentityV1, EvmContractAddressV1,
    EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyEnvelopeIdentityV1, KeyReleaseRequestV1,
    NodeCustodyPublicKeyV1, NodePublicKey, PqHybridSealedShareV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1,
    RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1, RightsEvaluationEvidenceRequestV1,
    RightsEvaluationEvidenceV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
    RightsSubjectSourceV1, RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1,
    RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1, ShareCoordinateV1,
    SignedCustodyEpochV1, SignedRecipientKeyAuthorizationV1, SignedRuntimeReleaseOperationV1,
    ThresholdV1, WalletAddress, WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES, X_WING_DRAFT06_CIPHERTEXT_BYTES,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use k256::ecdsa::SigningKey as WalletSigningKey;
use sha3::Digest as _;
use x_wing::kem::{Decapsulator as _, KeyExport as _};
use x_wing::TryKeyInit as _;

mod support;

use support::*;

const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

fn ok_data(response: Response) -> Value {
    match response {
        Response::Ok { data: Some(data) } => data,
        other => panic!("expected ok data, got {other:?}"),
    }
}

fn error_code(response: Response) -> String {
    match response {
        Response::Error { code, .. } => code,
        other => panic!("expected error, got {other:?}"),
    }
}

fn provider_with_rpc(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "esc-local",
                "display_name": "ESC Local",
                "kind": "evm_json_rpc",
                "chain_id": 20,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW;
    provider
}

#[test]
fn chain_provider_rejects_hidden_prepare_transaction_fields() {
    let request = json!({
        "op": "prepare_transaction",
        "network": "esc",
        "from": "0x0000000000000000000000000000000000000001",
        "to": "0x0000000000000000000000000000000000000002",
        "value": "0",
        "gas_price": "1"
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("chain transaction requests must reject hidden raw transaction fields")
        .to_string();
    assert!(err.contains("gas_price"), "unexpected error: {err}");
}

#[test]
fn chain_provider_rejects_hidden_node_lifecycle_fields() {
    let request = json!({
        "op": "node_lifecycle",
        "network": "btc-local",
        "action": "status",
        "rpc_url": "http://127.0.0.1:8332"
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("node lifecycle requests must reject hidden raw RPC authority")
        .to_string();
    assert!(err.contains("rpc_url"), "unexpected error: {err}");
}

fn provider_with_rights_rpc(rpc_url: String, selector: &str) -> ChainProvider {
    provider_with_rights_rpc_and_policies(rpc_url, selector, json!([]))
}

fn protected_content_policy_sources(
    action: &str,
    evidence_rpc_urls: Vec<String>,
) -> serde_json::Value {
    json!([{
        "action": action,
        "evidence_rpc_urls": evidence_rpc_urls,
    }])
}

fn provider_with_rights_rpc_and_policies(
    rpc_url: String,
    selector: &str,
    protected_content_policies: Value,
) -> ChainProvider {
    provider_with_rights_rpc_policies_and_purchase(
        rpc_url,
        selector,
        protected_content_policies,
        Value::Null,
    )
}

fn provider_with_rights_rpc_policies_and_purchase(
    rpc_url: String,
    selector: &str,
    protected_content_policies: Value,
    protected_content_market: Value,
) -> ChainProvider {
    let networks = vec![json!({
        "id": "esc-local",
        "display_name": "ESC Local",
        "kind": "evm_json_rpc",
        "chain_id": 20,
        "native_symbol": "ELA",
        "provider": "test",
        "mainnet": false,
        "explorer_url": null,
        "rpc_url": rpc_url,
        "rights_methods": [{
            "id": "has_access_by_content_id",
            "contract": "0x0000000000000000000000000000000000000001",
            "abi": "has_access_by_content_id_address_bytes16",
            "selector": selector,
            "protected_content_policies": protected_content_policies
        }],
        "protected_content_market": protected_content_market
    })];
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                "networks": networks
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW;
    provider
}

fn protected_content_market_source(evidence_rpc_urls: Vec<String>) -> Value {
    json!({
        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
        "evidence_rpc_urls": evidence_rpc_urls
    })
}

fn mutated_asset_created_log_data(mut log: Value, mutate: impl FnOnce(&mut Vec<u8>)) -> Value {
    let data = log
        .get("data")
        .and_then(Value::as_str)
        .expect("asset log data");
    let mut bytes = decode_hex(data, None, "asset log data").expect("decode asset log data");
    mutate(&mut bytes);
    log["data"] = json!(format!("0x{}", encode_hex(&bytes)));
    log
}

fn provider_with_creator_mint_rpc(rpc_url: String) -> ChainProvider {
    provider_with_creator_mint_rpc_and_market_sources(
        rpc_url,
        vec![
            "https://rpc-a.example".to_string(),
            "https://rpc-b.example".to_string(),
        ],
    )
}

fn provider_with_creator_mint_rpc_and_market_sources(
    rpc_url: String,
    evidence_rpc_urls: Vec<String>,
) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                "networks": [{
                    "id": "base-local",
                    "display_name": "Base Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 8453,
                    "native_symbol": "ETH",
                    "provider": "test",
                    "mainnet": true,
                    "explorer_url": null,
                    "rpc_url": rpc_url,
                    "rights_methods": [],
                    "protected_content_creator_mint": {
                        "ledger": "0x0000000000000000000000000000000000000022",
                        "pay_token": "0x0000000000000000000000000000000000000033",
                        "asset_created_emitter": "0x00000000000000000000000000000000000000dd",
                        "abi": "elacity_mint_v1"
                    },
                    "protected_content_market": {
                        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
                        "evidence_rpc_urls": evidence_rpc_urls
                    }
                }]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

fn resolved_policy_body_from_data(data: &Value) -> RightsPolicyBodyV1 {
    assert_eq!(data["schema"], PROTECTED_CONTENT_POLICY_SCHEMA);
    let hex = data["policy_body"]
        .as_str()
        .expect("policy response must include canonical bytes");
    let bytes = decode_hex(hex, None, "policy_body").unwrap();
    RightsPolicyBodyV1::from_canonical_bytes(&bytes).unwrap()
}

fn provider_with_rights_rpc_without_runtime_issuer(
    rpc_url: String,
    selector: &str,
) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": rpc_url,
                    "rights_methods": [{
                        "id": "has_access_by_content_id",
                        "contract": "0x0000000000000000000000000000000000000001",
                        "abi": "has_access_by_content_id_address_bytes16",
                        "selector": selector
                    }]
                }]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

fn provider_with_rights_rpc_at(
    rpc_url: String,
    selector: &str,
    now_unix_seconds: fn() -> u64,
) -> ChainProvider {
    let mut provider = provider_with_rights_rpc(rpc_url, selector);
    provider.now_unix_seconds = now_unix_seconds;
    provider
}

fn protected_content_policy_and_request() -> (RightsPolicyBodyV1, RightsEvaluationEvidenceRequestV1)
{
    let mut contract = [0u8; 20];
    contract[19] = 1;
    let encrypted_content =
        EncryptedContentIdentityV1::new(Digest32::new([0x11; 32]), 2048).unwrap();
    let content_access_id = ContentAccessIdV1::new([0x41; 16]).unwrap();
    let policy = RightsPolicyBodyV1::new(
        encrypted_content.clone(),
        content_access_id,
        RightsActionV1::View,
        RightsSubjectSourceV1::WalletAddress,
        20,
        EvmContractAddressV1::new(contract).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
        RightsObservationFinalityV1::finalized(),
    )
    .unwrap();
    let profile_key = SigningKey::from_bytes(&[0x43; 32]);
    let binding = ProtectedContentBindingV1::new(
        encrypted_content.clone(),
        KeyEnvelopeIdentityV1::new(
            encrypted_content,
            Digest32::new([0x22; 32]),
            512,
            Digest32::new([0x33; 32]),
            ThresholdV1::new(2, 3).unwrap(),
            CustodyPoolIdentityV1::new(Digest32::new([0x44; 32]), 128).unwrap(),
            CustodyEpochIdentityV1::new(Digest32::new([0x55; 32]), 128).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(Digest32::new([0x66; 32]), 128).unwrap(),
        )
        .unwrap(),
        policy.policy_identity().unwrap(),
        ProfileIdentityV1::from_public_key_bytes(profile_key.verifying_key().to_bytes()).unwrap(),
        wallet_address(7),
        RuntimeSessionBindingV1::new(Digest32::new([0x77; 32])).unwrap(),
    )
    .unwrap();
    let request =
        RightsEvaluationEvidenceRequestV1::new(binding, policy.policy_identity().unwrap()).unwrap();
    (policy, request)
}

fn contract_hex(contract: &impl CanonicalContract) -> String {
    format!("0x{}", encode_hex(&contract.canonical_bytes().unwrap()))
}

fn content_access_id(seed: u8) -> ContentAccessIdV1 {
    ContentAccessIdV1::new([seed; 16]).unwrap()
}

fn encrypted_content(seed: u8) -> EncryptedContentIdentityV1 {
    EncryptedContentIdentityV1::new(Digest32::new([seed; 32]), 2048).unwrap()
}

fn evm_bool_word(value: bool) -> Value {
    let mut bytes = [0u8; 32];
    bytes[31] = u8::from(value);
    json!(format!("0x{}", encode_hex(&bytes)))
}

fn unbound_content_id_error(access_id: &ContentAccessIdV1) -> Value {
    json!({
        "code": 3,
        "message": "execution reverted",
        "data": format!(
            "0x{}{}{}",
            "cad88223",
            encode_hex(access_id.as_bytes()),
            "00000000000000000000000000000000"
        ),
    })
}

fn protected_content_asset_created_log(
    emitter: &str,
    creator: &str,
    ledger: &str,
    operative: &str,
    token_id: &str,
    token_uri: &str,
    op_type_code: u16,
) -> Value {
    validate_hex_quantity(token_id, "token_id").unwrap();
    let raw = token_id.strip_prefix("0x").unwrap();
    let padded = if raw.len().is_multiple_of(2) {
        raw.to_string()
    } else {
        format!("0{raw}")
    };
    let mut data = padded
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = (chunk[0] as char).to_digit(16).unwrap() as u8;
            let low = (chunk[1] as char).to_digit(16).unwrap() as u8;
            (high << 4) | low
        })
        .collect::<Vec<_>>();
    let mut token_id_word = vec![0u8; 32 - data.len()];
    token_id_word.append(&mut data);
    let mut encoded = token_id_word;
    encoded.extend_from_slice(&abi_word_usize(96));
    encoded.extend_from_slice(&abi_word_u128(op_type_code as u128));
    encoded.extend_from_slice(&abi_encode_string(token_uri.as_bytes()));
    json!({
        "address": emitter,
        "topics": [
            ProtectedContentCreatorMintAbi::ElacityMintV1.asset_created_topic0(),
            format!("0x{}", encode_hex(&abi_word_address(creator).unwrap())),
            format!("0x{}", encode_hex(&abi_word_address(ledger).unwrap())),
            format!("0x{}", encode_hex(&abi_word_address(operative).unwrap())),
        ],
        "data": format!("0x{}", encode_hex(&encoded)),
    })
}

fn protected_content_mint_receipt_json(
    transaction_hash: &str,
    from: &str,
    to: &str,
    block_number: &str,
    block_hash: &str,
    status: &str,
    logs: Vec<Value>,
) -> Value {
    json!({
        "transactionHash": transaction_hash,
        "status": status,
        "from": from,
        "to": to,
        "blockNumber": block_number,
        "blockHash": block_hash,
        "logs": logs,
    })
}

fn canonical_block_json(number: &str, hash: &str, transactions: Vec<&str>) -> Value {
    json!({
        "number": number,
        "hash": hash,
        "transactions": transactions,
    })
}

fn finalized_block_json(number: &str, hash: &str) -> Value {
    finalized_block_json_at(number, hash, RIGHTS_EVIDENCE_NOW - 5)
}

fn finalized_block_json_at(number: &str, hash: &str, timestamp: u64) -> Value {
    json!({
        "number": number,
        "hash": hash,
        "timestamp": format!("0x{:x}", timestamp),
    })
}

#[test]
fn encode_has_access_by_content_id_call_uses_address_and_bytes16_abi() {
    let access_id = [0x41; 16];
    let encoded = encode_has_access_by_content_id_call(
        "0x12345678",
        &access_id,
        "0x00000000000000000000000000000000000000ab",
    )
    .unwrap();
    assert_eq!(
        encoded,
        concat!(
            "0x12345678",
            "00000000000000000000000000000000000000000000000000000000000000ab",
            "4141414141414141414141414141414100000000000000000000000000000000"
        )
    );
}

#[test]
fn encode_authority_gateway_buy_access_call_uses_exact_listing_terms_and_optional_pay_token() {
    let without_pay_token = encode_authority_gateway_buy_access_call(
        PROTECTED_CONTENT_BUY_ACCESS_NATIVE_SELECTOR,
        "0x0000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000002",
        "0x03",
        "0x04",
        "0x05",
        None,
    )
    .unwrap();
    assert_eq!(
        without_pay_token,
        concat!(
            "0xf7580ad9",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000002",
            "0000000000000000000000000000000000000000000000000000000000000003",
            "0000000000000000000000000000000000000000000000000000000000000004",
            "0000000000000000000000000000000000000000000000000000000000000005"
        )
    );

    let with_pay_token = encode_authority_gateway_buy_access_call(
        PROTECTED_CONTENT_BUY_ACCESS_ERC20_SELECTOR,
        "0x0000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000002",
        "0x03",
        "0x04",
        "0x05",
        Some("0x0000000000000000000000000000000000000006"),
    )
    .unwrap();
    assert_eq!(
        with_pay_token,
        concat!(
            "0x0ede2294",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000002",
            "0000000000000000000000000000000000000000000000000000000000000003",
            "0000000000000000000000000000000000000000000000000000000000000004",
            "0000000000000000000000000000000000000000000000000000000000000005",
            "0000000000000000000000000000000000000000000000000000000000000006"
        )
    );

    let approval =
        encode_erc20_approve_call("0x00000000000000000000000000000000000000bb", "0x14").unwrap();
    assert_eq!(
        approval,
        concat!(
            "0x095ea7b3",
            "00000000000000000000000000000000000000000000000000000000000000bb",
            "0000000000000000000000000000000000000000000000000000000000000014"
        )
    );
}

#[test]
fn protected_content_read_selectors_match_reviewed_abi_signatures() {
    let operative_selector = &sha3::Keccak256::digest(b"operative(address,uint256)")[..4];
    assert_eq!(
        format!("0x{}", encode_hex(operative_selector)),
        PROTECTED_CONTENT_OPERATIVE_SELECTOR
    );

    let mint_selector = &sha3::Keccak256::digest(b"mint(string,uint16,bytes,bytes)")[..4];
    assert_eq!(
        format!("0x{}", encode_hex(mint_selector)),
        ProtectedContentCreatorMintAbi::ElacityMintV1.selector()
    );
}

#[test]
fn encode_authority_gateway_listing_call_uses_access_token_id_one() {
    let encoded = encode_authority_gateway_listing_call(
        "0x0000000000000000000000000000000000000044",
        "0x0000000000000000000000000000000000000055",
    )
    .unwrap();
    assert_eq!(
        encoded,
        concat!(
            "0x6bd3a64b",
            "0000000000000000000000000000000000000000000000000000000000000044",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000055"
        )
    );
}

#[test]
fn decode_evm_bool_requires_exact_32_byte_canonical_word() {
    assert!(decode_evm_bool(&json!("0x1")).is_err());
    assert!(decode_evm_bool(&json!(format!(
        "0x{}",
        encode_hex(&{
            let mut bytes = [0u8; 32];
            bytes[0] = 1;
            bytes
        })
    )))
    .is_err());
    assert!(decode_evm_bool(&json!(format!(
        "0x{}",
        encode_hex(&{
            let mut bytes = [0u8; 32];
            bytes[31] = 2;
            bytes
        })
    )))
    .is_err());
    assert!(!decode_evm_bool(&evm_bool_word(false)).unwrap());
    assert!(decode_evm_bool(&evm_bool_word(true)).unwrap());
}

const RIGHTS_EVIDENCE_NOW: u64 = 2_000_000_010;

fn rights_evidence_now() -> u64 {
    RIGHTS_EVIDENCE_NOW
}

fn before_runtime_operation_window() -> u64 {
    1_999_999_500
}

fn after_runtime_operation_window() -> u64 {
    2_000_000_041
}

fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
    .unwrap()
}

fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
    RuntimeOperationIssuerKeyV1::new(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
    .unwrap()
}

fn runtime_issuer_hex(seed: u8) -> String {
    format!("0x{}", encode_hex(runtime_issuer(seed).as_bytes()))
}

fn wallet_address(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = sha3::Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
    RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(seed.max(9))).unwrap()
}

fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
    recipient_public_key(seed)
        .key_identity(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
        .unwrap()
}

fn xwing_public_key_bytes(
    seed: u8,
) -> [u8; elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
    let secret = x_wing::DecapsulationKey::from([seed; x_wing::DECAPSULATION_KEY_SIZE]);
    secret.encapsulation_key().to_bytes().into()
}

fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
    NodeCustodyPublicKeyV1::new(xwing_public_key_bytes(seed)).unwrap()
}

fn sealed_share(seed: u8) -> PqHybridSealedShareV1 {
    let public = x_wing::EncapsulationKey::new_from_slice(&xwing_public_key_bytes(seed)).unwrap();
    let (ciphertext, _) =
        public.encapsulate_deterministic(&[seed; x_wing::ENCAPSULATION_RANDOMNESS_SIZE].into());
    let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
    let mut envelope = Vec::with_capacity(PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES);
    envelope.extend_from_slice(&ciphertext);
    envelope.extend_from_slice(&[seed; PQ_HYBRID_AEAD_NONCE_BYTES]);
    envelope.extend_from_slice(&[seed ^ 0x5a; PQ_HYBRID_WRAPPED_SHARE_BYTES]);
    PqHybridSealedShareV1::new(envelope).unwrap()
}

fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let nodes = vec![
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(1),
            node_custody_public_key(0x31),
            ShareCoordinateV1::new(1).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(2),
            node_custody_public_key(0x32),
            ShareCoordinateV1::new(2).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(3),
            node_custody_public_key(0x33),
            ShareCoordinateV1::new(3).unwrap(),
        )
        .unwrap(),
    ];
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        nodes,
    )
    .unwrap();
    SignedCustodyEpochV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn custody_envelope_for_policy(policy: &RightsPolicyBodyV1) -> CustodyEnvelopeV1 {
    let epoch = signed_custody_epoch();
    let manifest = CustodyEnvelopeManifestV1::new(
        policy.encrypted_content().clone(),
        CustodyPoolIdentityV1::new(digest(0x25), 512).unwrap(),
        epoch.epoch_identity().unwrap(),
        CustodyCommitteeAuthorizationIdentityV1::new(digest(0x26), 512).unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        digest(0x33),
        epoch.statement().nodes().to_vec(),
    )
    .unwrap();
    let shares = [0x50, 0x51, 0x52].into_iter().map(sealed_share).collect();
    let envelope = CustodyEnvelopeV1::new(manifest, shares).unwrap();
    let binding = binding_for_policy_and_envelope(policy, &envelope);
    assert_eq!(binding.rights_policy(), &policy.policy_identity().unwrap());
    envelope
}

fn binding_for_policy_and_envelope(
    policy: &RightsPolicyBodyV1,
    envelope: &CustodyEnvelopeV1,
) -> ProtectedContentBindingV1 {
    let profile_key = SigningKey::from_bytes(&[0x43; 32]);
    ProtectedContentBindingV1::new(
        envelope.manifest().encrypted_content().clone(),
        envelope.key_envelope_identity().unwrap(),
        policy.policy_identity().unwrap(),
        ProfileIdentityV1::from_public_key_bytes(profile_key.verifying_key().to_bytes()).unwrap(),
        wallet_address(7),
        RuntimeSessionBindingV1::new(digest(0x77)).unwrap(),
    )
    .unwrap()
}

fn signed_runtime_operation_for_policy(
    policy: RightsPolicyBodyV1,
) -> SignedRuntimeReleaseOperationV1 {
    signed_runtime_operation_for_policy_and_runtime_seed(policy, 0x42)
}

fn signed_runtime_operation_for_policy_and_runtime_seed(
    policy: RightsPolicyBodyV1,
    runtime_seed: u8,
) -> SignedRuntimeReleaseOperationV1 {
    let runtime_key = SigningKey::from_bytes(&[runtime_seed; 32]);
    let envelope = custody_envelope_for_policy(&policy);
    let binding = binding_for_policy_and_envelope(&policy, &envelope);
    let rights_request = elastos_protected_content_contracts::RightsRequestV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_identity(0x30),
        2_000_000_000,
        2_000_000_180,
        ReplayNonce16::new([0x55; 16]),
    )
    .unwrap();
    let wallet_key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
    let (wallet_signature, recovery_id) = wallet_key
        .sign_prehash_recoverable(&ethereum_signed_message_hash(
            &rights_request.canonical_bytes().unwrap(),
        ))
        .unwrap();
    let mut wallet_signature_bytes = wallet_signature.to_bytes().to_vec();
    wallet_signature_bytes.push(recovery_id.to_byte());
    let signed_rights =
        WalletSignedRightsRequestV1::new(rights_request, wallet_signature_bytes).unwrap();
    let release_request = KeyReleaseRequestV1::new(
        binding.clone(),
        signed_rights.request().request_hash().unwrap(),
        RightsActionV1::View,
        signed_rights.request().recipient().clone(),
        2_000_000_001,
        2_000_000_050,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let recipient_public_key = recipient_public_key(0x30);
    let profile_key = SigningKey::from_bytes(&[0x43; 32]);
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_public_key,
        signed_rights.request().recipient().clone(),
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        2_000_000_000,
        2_000_000_090,
    )
    .unwrap();
    let authorization = SignedRecipientKeyAuthorizationV1::new(
        authorization_statement.clone(),
        profile_key
            .sign(&authorization_statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let evidence_request =
        RightsEvaluationEvidenceRequestV1::new(binding, policy.policy_identity().unwrap()).unwrap();
    let statement = RuntimeReleaseOperationStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        signed_rights,
        release_request,
        recipient_public_key,
        authorization,
        policy,
        evidence_request,
        signed_custody_epoch(),
        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
        2_000_000_002,
        2_000_000_040,
    )
    .unwrap();
    SignedRuntimeReleaseOperationV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn protected_content_signed_operation() -> SignedRuntimeReleaseOperationV1 {
    signed_runtime_operation_for_policy(protected_content_policy_and_request().0)
}

fn wallet_subject_hex(operation: &SignedRuntimeReleaseOperationV1) -> String {
    format!(
        "0x{}",
        encode_hex(
            operation
                .statement()
                .evidence_request()
                .binding()
                .wallet()
                .as_bytes()
        )
    )
}

fn provider_with_bitcoin_rpc(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    init_bitcoin_rpc_provider(&mut provider, rpc_url);
    provider
}

fn provider_with_bitcoin_rpc_in(data_dir: &Path, rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::with_data_dir(data_dir.to_path_buf());
    init_bitcoin_rpc_provider(&mut provider, rpc_url);
    provider
}

fn init_bitcoin_rpc_provider(provider: &mut ChainProvider, rpc_url: String) {
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_core_rpc",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "Bitcoin Core",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
}

fn write_node_supervisor_helper(data_dir: &Path) -> String {
    let helper = data_dir.join("test-node-supervisor");
    fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    helper.to_string_lossy().into_owned()
}

fn add_test_node_supervisor(provider: &mut ChainProvider, network_id: &str, program: &str) {
    let init = provider.handle(Request::Init {
        config: json!({
            "node_supervisor": {
                "networks": {
                    network_id: {
                        "start": { "program": program, "args": [] },
                        "stop": { "program": program, "args": [] },
                        "restart": { "program": program, "args": [] },
                        "timeout_ms": 1000
                    }
                }
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
}

fn provider_with_bitcoin_rest(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_rest",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

fn provider_with_mainchain_rest(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "ela-local",
                "display_name": "ELA Local",
                "kind": "mainchain_rest",
                "chain_id": null,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

#[test]
fn lists_production_default_networks_without_rpc_urls() {
    let mut provider = ChainProvider::new();
    let data = ok_data(provider.handle(Request::Networks));
    let networks = data["networks"].as_array().unwrap();
    let ids = networks
        .iter()
        .map(|network| network["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["ela-mainnet", "esc-mainnet", "base-mainnet", "btc-mainnet"]
    );
    assert!(networks.iter().all(|network| network["mainnet"] == true));
    assert!(networks
        .iter()
        .all(|network| !network["id"].as_str().unwrap().contains("testnet")));
    assert!(networks
        .iter()
        .all(|network| !network["id"].as_str().unwrap().contains("eid")));
    assert!(networks
        .iter()
        .any(|network| network["id"] == "base-mainnet"
            && network["kind"] == "evm_json_rpc"
            && network["chain_id"] == 8453
            && network["configured"] == true));
    assert!(networks.iter().any(|network| network["id"] == "btc-mainnet"
        && network["kind"] == "bitcoin_rest"
        && network["configured"] == true));
    assert!(networks
        .iter()
        .all(|network| network.get("rpc_url").is_none()));
}

#[test]
fn rejects_invalid_balance_address_before_upstream() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let response = provider.handle(Request::Balance {
        network: "esc-local".to_string(),
        address: "0x1234".to_string(),
        block: None,
    });
    assert_eq!(error_code(response), "invalid_address");
}

#[test]
fn rejects_mainchain_for_evm_operations() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Balance {
        network: "ela-mainnet".to_string(),
        address: "0x0000000000000000000000000000000000000000".to_string(),
        block: None,
    });
    assert_eq!(error_code(response), "unsupported_network_kind");
}

#[test]
fn proxies_mainchain_status_with_typed_rest_method() {
    let rpc_url = spawn_http_sequence_server(vec![(
        "/blocks?page=1&pageSize=1",
        json!({
            "data": [{
                "height": 2203455,
                "hash": "c5646678a05b7abcdc7449edafd331b5994a998b50f784e5b4ee05071749930a",
                "timestamp": 1777700819,
                "txCount": 3
            }],
            "total": 2203456
        })
        .to_string(),
        "application/json",
    )]);
    let mut provider = provider_with_mainchain_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "ela-local".to_string(),
    }));
    assert_eq!(data["block_height"], 2203455);
    assert_eq!(
        data["best_block_hash"],
        "c5646678a05b7abcdc7449edafd331b5994a998b50f784e5b4ee05071749930a"
    );
    assert_eq!(data["tx_count"], 3);
}

#[test]
fn bitcoin_status_fails_closed_when_node_is_not_configured() {
    let mut provider = provider_with_bitcoin_rpc(String::new());
    let response = provider.handle(Request::Status {
        network: "btc-local".to_string(),
    });
    assert_eq!(error_code(response), "node_not_configured");
}

#[test]
fn proxies_bitcoin_status_with_typed_method() {
    let rpc_url = spawn_rpc_server(
        "getblockchaininfo",
        json!({
            "chain": "main",
            "blocks": 840000,
            "headers": 840001,
            "bestblockhash": "0000000000000000000000000000000000000000000000000000000000000000",
            "initialblockdownload": false,
            "verificationprogress": 0.999,
        }),
    );
    let mut provider = provider_with_bitcoin_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["chain"], "main");
    assert_eq!(data["block_height"], 840000);
    assert_eq!(data["headers"], 840001);
}

#[test]
fn proxies_bitcoin_rest_status_with_typed_methods() {
    let rpc_url = spawn_http_sequence_server(vec![
        ("/blocks/tip/height", "840000".to_string(), "text/plain"),
        (
            "/blocks/tip/hash",
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            "text/plain",
        ),
    ]);
    let mut provider = provider_with_bitcoin_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["chain"], "main");
    assert_eq!(data["block_height"], 840000);
    assert_eq!(
        data["best_block_hash"],
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn proxies_bitcoin_rest_balance_with_typed_method() {
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    let rpc_url = spawn_http_sequence_server(vec![(
        "/address/bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
        json!({
            "chain_stats": {
                "funded_txo_sum": 120_000,
                "spent_txo_sum": 20_000
            },
            "mempool_stats": {
                "funded_txo_sum": 7_000,
                "spent_txo_sum": 2_000
            }
        })
        .to_string(),
        "application/json",
    )]);
    let mut provider = provider_with_bitcoin_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Balance {
        network: "btc-local".to_string(),
        address: address.to_string(),
        block: None,
    }));
    assert_eq!(data["network"], "btc-local");
    assert_eq!(data["address"], address);
    assert_eq!(data["confirmed_sats"], 100_000);
    assert_eq!(data["mempool_sats"], 5_000);
    assert_eq!(data["balance_sats"], 105_000);
    assert_eq!(data["native_symbol"], "BTC");
}

#[test]
fn proxies_block_number_with_typed_evm_method() {
    let rpc_url = spawn_rpc_server("eth_blockNumber", json!("0x2a"));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::BlockNumber {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["block_number_hex"], "0x2a");
    assert_eq!(data["block_number"], 42);
}

#[test]
fn proxies_evm_sync_health_with_typed_method() {
    let rpc_url = spawn_rpc_server("eth_syncing", json!(false));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["synced"], true);
    assert_eq!(data["syncing"], false);
    assert_eq!(data["network"]["id"], "esc-local");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn parses_evm_sync_progress_without_raw_rpc_passthrough() {
    let rpc_url = spawn_rpc_server(
        "eth_syncing",
        json!({
            "startingBlock": "0x1",
            "currentBlock": "0x2a",
            "highestBlock": "0x64"
        }),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["synced"], false);
    assert_eq!(data["sync"]["starting_block"], 1);
    assert_eq!(data["sync"]["current_block"], 42);
    assert_eq!(data["sync"]["highest_block"], 100);
}

#[test]
fn creates_typed_sync_health_proof_without_exposing_rpc() {
    let rpc_url = spawn_rpc_server("eth_syncing", json!(false));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Proof {
        network: "esc-local".to_string(),
        proof_kind: ChainProofKind::SyncHealth,
        subject: "person:local:alice".to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.proof/v1");
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["proof_kind"], "sync_health");
    assert_eq!(data["subject"], "person:local:alice");
    assert!(data["evidence_hash"].as_str().unwrap().starts_with("0x"));
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn verifies_erc1271_signature_through_typed_eth_call() {
    let message_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let signature = "0x010203";
    let expected_data = encode_erc1271_is_valid_signature_call(
        &decode_hex(message_hash, Some(32), "message_hash").unwrap(),
        &decode_hex(signature, None, "signature").unwrap(),
    );
    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x1626ba7e00000000000000000000000000000000000000000000000000000000"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Erc1271IsValidSignature {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        message_hash: message_hash.to_string(),
        signature: signature.to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.erc1271_proof/v1");
    assert_eq!(data["chain_id"], 20);
    assert_eq!(
        data["contract"],
        "0x0000000000000000000000000000000000000001"
    );
    assert_eq!(data["message_hash"], message_hash);
    assert_eq!(data["valid"], true);
    assert_eq!(data["magic_value"], "0x1626ba7e");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn proxies_typed_evm_contract_call_without_raw_rpc_url() {
    let data_hex = "0x70a082310000000000000000000000001111111111111111111111111111111111111111";
    let rpc_url = spawn_eth_call_server(
        data_hex.to_string(),
        json!("0x0000000000000000000000000000000000000000000000000000000000000042"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::ContractCall {
        network: "esc-local".to_string(),
        to: "0x0000000000000000000000000000000000000001".to_string(),
        data: data_hex.to_string(),
        block: None,
    }));

    assert_eq!(data["schema"], "elastos.chain.contract_call/v1");
    assert_eq!(data["network"], "esc-local");
    assert_eq!(
        data["result"],
        "0x0000000000000000000000000000000000000000000000000000000000000042"
    );
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn proxies_typed_evm_gas_estimate_without_wallet_approval() {
    let rpc_url = spawn_rpc_server("eth_estimateGas", json!("0x5208"));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::EstimateGas {
        network: "esc-local".to_string(),
        from: "0x0000000000000000000000000000000000000001".to_string(),
        to: "0x0000000000000000000000000000000000000002".to_string(),
        value: Some("0x1".to_string()),
        data: Some("0x1234".to_string()),
    }));

    assert_eq!(data["schema"], "elastos.chain.gas_estimate/v1");
    assert_eq!(data["gas_limit"], "0x5208");
    assert!(data.get("requires_wallet_approval").is_none());
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn erc1271_rejects_invalid_inputs_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    assert_eq!(
        error_code(provider.handle(Request::Erc1271IsValidSignature {
            network: "esc-local".to_string(),
            contract: "0x0000000000000000000000000000000000000001".to_string(),
            message_hash: "0x1234".to_string(),
            signature: "0x0102".to_string(),
        })),
        "invalid_message_hash"
    );
    assert_eq!(
        error_code(provider.handle(Request::Erc1271IsValidSignature {
            network: "esc-local".to_string(),
            contract: "not-an-address".to_string(),
            message_hash:
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            signature: "0x0102".to_string(),
        })),
        "invalid_contract"
    );
}

#[test]
fn prepares_typed_evm_transaction_intent_without_node_write() {
    let rpc_url = spawn_rpc_sequence_server(vec![
        ("eth_getTransactionCount", json!("0x7")),
        ("eth_gasPrice", json!("0x3b9aca00")),
        ("eth_estimateGas", json!("0x5208")),
    ]);
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::PrepareTransaction {
        network: "esc-local".to_string(),
        from: "0x0000000000000000000000000000000000000001".to_string(),
        to: "0x0000000000000000000000000000000000000002".to_string(),
        value: "0x0".to_string(),
        data: Some("0x1234".to_string()),
    }));

    assert_eq!(
        data["schema"],
        "elastos.chain.unsigned_transaction_intent/v1"
    );
    assert_eq!(data["transaction_type"], "eip155_legacy");
    assert_eq!(data["nonce"], "0x7");
    assert_eq!(data["gas_price"], "0x3b9aca00");
    assert_eq!(data["gas_limit"], "0x5208");
    assert_eq!(data["requires_wallet_approval"], true);
    assert_eq!(data["wallet_intent"], "transaction_intent");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn prepare_transaction_rejects_oversized_data_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let oversized = format!("0x{}", "00".repeat(256 * 1024 + 1));
    let response = provider.handle(Request::PrepareTransaction {
        network: "esc-local".to_string(),
        from: "0x0000000000000000000000000000000000000001".to_string(),
        to: "0x0000000000000000000000000000000000000002".to_string(),
        value: "0x0".to_string(),
        data: Some(oversized),
    });

    assert_eq!(error_code(response), "invalid_data");
}

#[test]
fn exposes_typed_evm_dapp_read_helpers_without_raw_rpc_urls() {
    let rpc_url = spawn_rpc_sequence_server(vec![
        ("eth_getTransactionCount", json!("0x7")),
        ("eth_gasPrice", json!("0x3b9aca00")),
        (
            "eth_feeHistory",
            json!({
                "oldestBlock": "0x1",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca01"],
                "gasUsedRatio": [0.5],
                "reward": [["0x1"]]
            }),
        ),
        ("eth_getCode", json!("0x60016001")),
        (
            "eth_getLogs",
            json!([{
                "address": "0x0000000000000000000000000000000000000002",
                "blockNumber": "0x2a",
                "data": "0x",
                "topics": []
            }]),
        ),
    ]);
    let mut provider = provider_with_rpc(rpc_url);
    let address = "0x0000000000000000000000000000000000000001";

    let nonce = ok_data(provider.handle(Request::TransactionCount {
        network: "esc-local".to_string(),
        address: address.to_string(),
        block: Some("pending".to_string()),
    }));
    assert_eq!(nonce["schema"], "elastos.chain.transaction_count/v1");
    assert_eq!(nonce["nonce"], "0x7");
    assert!(nonce.get("rpc_url").is_none());

    let gas_price = ok_data(provider.handle(Request::GasPrice {
        network: "esc-local".to_string(),
    }));
    assert_eq!(gas_price["schema"], "elastos.chain.gas_price/v1");
    assert_eq!(gas_price["gas_price"], "0x3b9aca00");

    let history = ok_data(provider.handle(Request::FeeHistory {
        network: "esc-local".to_string(),
        block_count: "0x1".to_string(),
        newest_block: "latest".to_string(),
        reward_percentiles: vec![1.0],
    }));
    assert_eq!(history["schema"], "elastos.chain.fee_history/v1");
    assert_eq!(history["history"]["oldestBlock"], "0x1");

    let code = ok_data(provider.handle(Request::Code {
        network: "esc-local".to_string(),
        address: address.to_string(),
        block: Some("latest".to_string()),
    }));
    assert_eq!(code["schema"], "elastos.chain.code/v1");
    assert_eq!(code["code"], "0x60016001");

    let logs = ok_data(provider.handle(Request::Logs {
        network: "esc-local".to_string(),
        filter: json!({
            "fromBlock": "0x1",
            "toBlock": "latest",
            "address": "0x0000000000000000000000000000000000000002",
            "topics": []
        }),
    }));
    assert_eq!(logs["schema"], "elastos.chain.logs/v1");
    assert_eq!(logs["logs"][0]["blockNumber"], "0x2a");
    assert_json_strings_do_not_contain(&logs, "127.0.0.1");
}

#[test]
fn broadcasts_typed_evm_signed_transaction() {
    let rpc_url = spawn_rpc_server(
        "eth_sendRawTransaction",
        json!("0x000000000000000000000000000000000000000000000000000000000000002a"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::BroadcastTransaction {
        network: "esc-local".to_string(),
        signed_transaction: "0x1234".to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.broadcast_receipt/v1");
    assert_eq!(
        data["transaction_hash"],
        "0x000000000000000000000000000000000000000000000000000000000000002a"
    );
}

#[test]
fn node_lifecycle_reports_status_and_fails_closed_for_control() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:8332".to_string());
    let data = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(data["schema"], "elastos.chain.node_lifecycle/v1");
    assert_eq!(data["managed"], true);
    assert_eq!(data["control_available"], false);
    assert_eq!(
        data["control_reason"],
        "node lifecycle control requires an operator-approved supervisor"
    );
    assert_eq!(data["state"], "external_loopback");
    assert!(data["network"].get("rpc_url").is_none());
    assert_json_strings_do_not_contain(&data, "127.0.0.1");
    assert_json_strings_do_not_contain(&data, "8332");

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
}

#[test]
fn node_lifecycle_runs_operator_supervisor_for_loopback_nodes() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18446".to_string());
    let supervisor_program = write_node_supervisor_helper(data_dir.path());
    add_test_node_supervisor(&mut provider, "btc-local", &supervisor_program);

    let status = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(status["managed"], true);
    assert_eq!(status["control_available"], true);
    assert_eq!(status["state"], "managed_local");
    assert_json_strings_do_not_contain(&status, &supervisor_program);
    assert_json_strings_do_not_contain(&status, "18446");

    let start = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    }));
    assert_eq!(start["action"], "start");
    assert_eq!(start["control_available"], true);
    assert_eq!(start["state"], "managed_local");
    assert_json_strings_do_not_contain(&start, &supervisor_program);
    assert_json_strings_do_not_contain(&start, "18446");
}

#[test]
fn node_lifecycle_rejects_supervisor_control_for_remote_backends() {
    let data_dir = TestDataDir::new();
    let mut provider = provider_with_rpc("https://example.invalid/rpc".to_string());
    let supervisor_program = write_node_supervisor_helper(data_dir.path());
    add_test_node_supervisor(&mut provider, "esc-local", &supervisor_program);

    let response = provider.handle(Request::NodeLifecycle {
        network: "esc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });

    assert_eq!(error_code(response), "managed_node_unavailable");
}

#[test]
fn node_lifecycle_state_survives_provider_reload_without_raw_rpc() {
    let data_dir = TestDataDir::new();
    let rpc_url = "http://127.0.0.1:18443".to_string();
    let mut provider = provider_with_bitcoin_rpc_in(data_dir.path(), rpc_url.clone());

    let first = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    let first_seen_at = first["first_seen_at"].as_u64().unwrap();
    assert_eq!(first["state"], "external_loopback");
    assert_json_strings_do_not_contain(&first, &rpc_url);
    assert_json_strings_do_not_contain(&first, "18443");

    let state_path = node_lifecycle_state_path(data_dir.path());
    let state = read_node_lifecycle_state_file(&state_path).unwrap();
    let persisted = state.networks.get("btc-local").unwrap();
    assert_eq!(persisted.state, NodeLifecycleStateKind::ExternalLoopback);
    assert!(persisted.managed);
    let state_json = serde_json::to_value(&state).unwrap();
    assert!(state_json
        .pointer("/networks/btc-local/control_available")
        .is_none());
    assert!(state_json
        .pointer("/networks/btc-local/control_reason")
        .is_none());
    assert_json_strings_do_not_contain(&state_json, &rpc_url);
    assert_json_strings_do_not_contain(&state_json, "18443");

    let mut reloaded = provider_with_bitcoin_rpc_in(data_dir.path(), rpc_url);
    let second = ok_data(reloaded.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(second["state"], "external_loopback");
    assert_eq!(second["first_seen_at"].as_u64().unwrap(), first_seen_at);
    assert_json_strings_do_not_contain(&second, "127.0.0.1");
    assert_json_strings_do_not_contain(&second, "18443");
}

#[test]
fn unsupported_node_lifecycle_actions_do_not_persist_state() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18444".to_string());

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let state_path = node_lifecycle_state_path(data_dir.path());
    assert!(!state_path.exists());

    ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    let before = fs::read_to_string(&state_path).unwrap();

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Restart,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let after = fs::read_to_string(&state_path).unwrap();
    assert_eq!(after, before);

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Stop,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let after = fs::read_to_string(&state_path).unwrap();
    assert_eq!(after, before);
}

#[test]
fn corrupt_node_lifecycle_state_fails_closed_with_typed_error() {
    let data_dir = TestDataDir::new();
    let state_path = node_lifecycle_state_path(data_dir.path());
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    fs::write(&state_path, "{not json").unwrap();

    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18445".to_string());
    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    });
    assert_eq!(error_code(response), "node_lifecycle_state_unavailable");
}

#[test]
fn bitcoin_sync_health_reports_initial_block_download() {
    let rpc_url = spawn_rpc_server(
        "getblockchaininfo",
        json!({
            "chain": "main",
            "blocks": 840000,
            "headers": 840100,
            "initialblockdownload": true,
            "verificationprogress": 0.98
        }),
    );
    let mut provider = provider_with_bitcoin_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["synced"], false);
    assert_eq!(data["syncing"], true);
    assert_eq!(data["block_height"], 840000);
    assert_eq!(data["headers"], 840100);
}

#[test]
fn protected_content_rights_evidence_requires_configured_runtime_issuer_before_backend() {
    let operation = protected_content_signed_operation();
    let mut provider = provider_with_rights_rpc_without_runtime_issuer(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
    );

    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "runtime_issuer_not_configured"
    );
}

#[test]
fn protected_content_rights_evidence_reinit_replaces_runtime_issuer_trust() {
    let operation = protected_content_signed_operation();
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x12345678");

    let malformed = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": "0xnothex"
            }
        }),
    });
    assert_eq!(error_code(malformed), "invalid_config");
    assert_eq!(
        provider.protected_content_runtime_issuer,
        Some(runtime_issuer(0x42))
    );

    let cleared = provider.handle(Request::Init {
        config: json!({
            "extra": {}
        }),
    });
    assert!(matches!(cleared, Response::Ok { .. }));
    assert_eq!(provider.protected_content_runtime_issuer, None);
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "runtime_issuer_not_configured"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_bad_signature_issuer_and_window_before_backend() {
    let operation = protected_content_signed_operation();
    let bad_signature_operation =
        SignedRuntimeReleaseOperationV1::new(operation.statement().clone(), vec![0; 64]).unwrap();
    let wrong_issuer_operation = signed_runtime_operation_for_policy_and_runtime_seed(
        protected_content_policy_and_request().0,
        0x43,
    );

    for (label, signed_runtime_release_operation, now_unix_seconds) in [
        (
            "bad signature",
            contract_hex(&bad_signature_operation),
            rights_evidence_now as fn() -> u64,
        ),
        (
            "wrong issuer",
            contract_hex(&wrong_issuer_operation),
            rights_evidence_now,
        ),
        (
            "future operation",
            contract_hex(&operation),
            before_runtime_operation_window,
        ),
        (
            "expired operation",
            contract_hex(&operation),
            after_runtime_operation_window,
        ),
    ] {
        let mut provider = provider_with_rights_rpc_at(
            "http://127.0.0.1:9".to_string(),
            "0x12345678",
            now_unix_seconds,
        );
        assert_eq!(
            error_code(provider.handle(Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation,
            })),
            "invalid_runtime_operation",
            "{label}"
        );
    }
}

#[test]
fn protected_content_rights_evidence_returns_canonical_evidence_at_finalized_block() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let request = operation.statement().evidence_request();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x27", block_hash),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": block_hash, "requireCanonical": true }
            ]),
            evm_bool_word(true),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        ),
    );
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW;

    let data = ok_data(provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }));

    assert_eq!(
        data["schema"],
        "elastos.chain.protected-content-rights-evidence/v1"
    );
    assert_eq!(data["chain_id"], 20);
    assert_eq!(data["finalized_block_number"], 39);
    assert_eq!(data["finalized_block_hash"], block_hash);
    assert!(data.get("network").is_none());
    assert!(data.get("rpc_url").is_none());
    let evidence_hex = data["rights_evaluation_evidence"].as_str().unwrap();
    let evidence_bytes = decode_hex(evidence_hex, None, "rights_evaluation_evidence").unwrap();
    let evidence = RightsEvaluationEvidenceV1::from_canonical_bytes(&evidence_bytes).unwrap();
    evidence.validate_against_request(request, policy).unwrap();
    assert_eq!(
        evidence.runtime_operation_hash(),
        operation.statement().canonical_hash().unwrap()
    );
    assert_eq!(
        evidence.release_request_hash(),
        operation
            .statement()
            .release_request()
            .request_hash()
            .unwrap()
    );
    assert_eq!(evidence.acquired_at(), RIGHTS_EVIDENCE_NOW);
    assert_eq!(evidence.expires_at(), RIGHTS_EVIDENCE_NOW + 30);
    assert!(evidence.has_access());
    assert_eq!(evidence.finalized_block_number(), 39);
}

#[test]
fn protected_content_rights_evidence_captures_denial_without_caller_supplied_result() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json(
                "0x2a",
                "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "requireCanonical": true }
            ]),
            evm_bool_word(false),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        ),
    );
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW;

    let data = ok_data(provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }));
    let evidence_bytes = decode_hex(
        data["rights_evaluation_evidence"].as_str().unwrap(),
        None,
        "rights_evaluation_evidence",
    )
    .unwrap();
    let evidence = RightsEvaluationEvidenceV1::from_canonical_bytes(&evidence_bytes).unwrap();
    assert!(!evidence.has_access());
}

#[test]
fn protected_content_rights_evidence_rejects_fewer_than_two_matching_sources() {
    let operation = protected_content_signed_operation();
    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json(
                "0x2a",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x0000000000000000000000000000000000000001",
                    "data": encode_has_access_by_content_id_call(
                        "0x12345678",
                        operation.statement().policy_body().content_access_id().as_bytes(),
                        &wallet_subject_hex(&operation),
                    )
                    .unwrap()
                },
                {
                    "blockHash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "requireCanonical": true
                }
            ]),
            evm_bool_word(true),
        ),
    ]);
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources("view", vec![rpc_url, "http://127.0.0.1:9".to_string()]),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_rights_observations"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_chain_and_finalized_block_mismatches() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let wrong_chain_policy = RightsPolicyBodyV1::new(
        policy.encrypted_content().clone(),
        policy.content_access_id(),
        RightsActionV1::View,
        RightsSubjectSourceV1::WalletAddress,
        21,
        policy.contract_address(),
        policy.function_selector(),
        policy.method_abi(),
        policy.observation_finality(),
    )
    .unwrap();
    let wrong_chain_operation = signed_runtime_operation_for_policy(wrong_chain_policy);
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&wrong_chain_operation),
        })),
        "rights_query_not_configured"
    );

    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let wrong_chain_sequence = vec![
        ("eth_chainId", json!([]), json!("0x15")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json(
                "0x2a",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                {
                    "blockHash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "requireCanonical": true
                }
            ]),
            evm_bool_word(true),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(wrong_chain_sequence.clone()),
                spawn_rpc_sequence_asserting_server(wrong_chain_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "conflicting_rights_observations"
    );

    let invalid_chain_sequence = vec![("eth_chainId", json!([]), json!("not-a-quantity"))];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(invalid_chain_sequence.clone()),
                spawn_rpc_sequence_asserting_server(invalid_chain_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_rights_observations"
    );

    let bool_mismatch_true = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json(
                "0x2a",
                "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                {
                    "blockHash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "requireCanonical": true
                }
            ]),
            evm_bool_word(true),
        ),
    ];
    let bool_mismatch_false = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json(
                "0x2a",
                "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                {
                    "blockHash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "requireCanonical": true
                }
            ]),
            evm_bool_word(false),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(bool_mismatch_true),
                spawn_rpc_sequence_asserting_server(bool_mismatch_false),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "conflicting_rights_observations"
    );

    let invalid_block_sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2a", "0x1234"),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(invalid_block_sequence.clone()),
                spawn_rpc_sequence_asserting_server(invalid_block_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_rights_observations"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_when_eip1898_call_does_not_succeed_twice() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let finalized_hash = "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let failing_rpc_url = spawn_rpc_sequence_asserting_server_with_replies(vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(json!({
                "code": -32001,
                "message": "requested block hash is not canonical"
            })),
        ),
    ]);
    let success_sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2a", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            evm_bool_word(true),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                failing_rpc_url,
                spawn_rpc_sequence_asserting_server(success_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_rights_observations"
    );
}

#[test]
fn protected_content_rights_evidence_classifies_exact_matching_unbound_content_id() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let finalized_hash = "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(unbound_content_id_error(&policy.content_access_id())),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(sequence.clone()),
                spawn_rpc_sequence_asserting_server_with_replies(sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "unknown_protected_content_object"
    );
}

#[test]
fn protected_content_rights_evidence_requires_two_matching_unbound_sources() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let finalized_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let unbound_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(unbound_content_id_error(&policy.content_access_id())),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(unbound_sequence),
                "http://127.0.0.1:9".to_string(),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_rights_observations"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_unbound_conflicts_and_wrong_reverts() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let finalized_hash = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let unbound_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(unbound_content_id_error(&policy.content_access_id())),
        ),
    ];
    let allow_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Result(evm_bool_word(true)),
        ),
    ];
    let mut unbound_vs_allow = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(unbound_sequence.clone()),
                spawn_rpc_sequence_asserting_server_with_replies(allow_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(
            unbound_vs_allow.handle(Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation: contract_hex(&operation),
            })
        ),
        "conflicting_rights_observations"
    );

    let deny_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Result(evm_bool_word(false)),
        ),
    ];
    let mut unbound_vs_deny = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(unbound_sequence.clone()),
                spawn_rpc_sequence_asserting_server_with_replies(deny_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(
            unbound_vs_deny.handle(Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation: contract_hex(&operation),
            })
        ),
        "conflicting_rights_observations"
    );

    let wrong_selector = json!({
        "code": 3,
        "message": "execution reverted",
        "data": format!(
            "0x{}{}{}",
            "deadbeef",
            encode_hex(policy.content_access_id().as_bytes()),
            "00000000000000000000000000000000"
        ),
    });
    let wrong_selector_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(wrong_selector),
        ),
    ];
    let mut wrong_selector_provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(wrong_selector_sequence.clone()),
                spawn_rpc_sequence_asserting_server_with_replies(wrong_selector_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(
            wrong_selector_provider.handle(Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation: contract_hex(&operation),
            })
        ),
        "insufficient_rights_observations"
    );

    let wrong_access_id = content_access_id(0x99);
    let wrong_kid_sequence = vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            RpcReply::Result(finalized_block_json("0x2a", finalized_hash)),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": finalized_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(unbound_content_id_error(&wrong_access_id)),
        ),
    ];
    let mut wrong_kid_provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server_with_replies(wrong_kid_sequence.clone()),
                spawn_rpc_sequence_asserting_server_with_replies(wrong_kid_sequence),
            ],
        ),
    );
    assert_eq!(
        error_code(
            wrong_kid_provider.handle(Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation: contract_hex(&operation),
            })
        ),
        "insufficient_rights_observations"
    );
}

#[test]
fn resolve_protected_content_policy_returns_canonical_policy_and_evidence_accepts_it() {
    let encrypted_content = encrypted_content(0x31);
    let access_id = content_access_id(0x51);
    let policies = protected_content_policy_sources(
        "view",
        vec![
            "https://rpc-a.example".to_string(),
            "https://rpc-b.example".to_string(),
        ],
    );
    let mut resolver = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        policies.clone(),
    );
    let data = ok_data(resolver.handle(Request::ResolveProtectedContentPolicy {
        encrypted_content: contract_hex(&encrypted_content),
        content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
        action: ProtectedContentPolicyAction::View,
    }));
    let rendered = serde_json::to_string(&data).unwrap();
    assert!(!rendered.contains("http://127.0.0.1:9"));
    assert!(!rendered.contains("\"contract\""));
    assert!(!rendered.contains("\"selector\""));
    let policy = resolved_policy_body_from_data(&data);
    assert_eq!(policy.encrypted_content(), &encrypted_content);
    assert_eq!(policy.content_access_id(), access_id);
    assert_eq!(policy.required_action(), RightsActionV1::View);
    assert_eq!(
        policy.observation_finality(),
        RightsObservationFinalityV1::finalized()
    );

    let operation = signed_runtime_operation_for_policy(policy);
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        access_id.as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x1e", block_hash),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": block_hash, "requireCanonical": true }
            ]),
            evm_bool_word(true),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        ),
    );
    let evidence = ok_data(provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }));
    assert_eq!(
        evidence["schema"],
        "elastos.chain.protected-content-rights-evidence/v1"
    );
}

#[test]
fn resolve_protected_content_policy_rejects_missing_or_ambiguous_sources() {
    let encrypted_content = encrypted_content(0x31);
    let access_id = content_access_id(0x51);
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ResolveProtectedContentPolicy {
            encrypted_content: contract_hex(&encrypted_content),
            content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
            action: ProtectedContentPolicyAction::View,
        })),
        "rights_policy_not_configured"
    );

    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                "networks": [
                    {
                        "id": "esc-a",
                        "display_name": "ESC A",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "test-a",
                        "mainnet": false,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [{
                            "id": "has_access_by_content_id",
                            "contract": "0x0000000000000000000000000000000000000001",
                            "abi": "has_access_by_content_id_address_bytes16",
                            "selector": "0x12345678",
                            "protected_content_policies": [{
                                "action": "view",
                                "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                            }]
                        }]
                    },
                    {
                        "id": "esc-b",
                        "display_name": "ESC B",
                        "kind": "evm_json_rpc",
                        "chain_id": 21,
                        "native_symbol": "ELA",
                        "provider": "test-b",
                        "mainnet": false,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [{
                            "id": "has_access_by_content_id",
                            "contract": "0x0000000000000000000000000000000000000001",
                            "abi": "has_access_by_content_id_address_bytes16",
                            "selector": "0x12345678",
                            "protected_content_policies": [{
                                "action": "view",
                                "evidence_rpc_urls": ["https://rpc-c.example", "https://rpc-d.example"]
                            }]
                        }]
                    }
                ]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    assert_eq!(
        error_code(provider.handle(Request::ResolveProtectedContentPolicy {
            encrypted_content: contract_hex(&encrypted_content),
            content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
            action: ProtectedContentPolicyAction::View,
        })),
        "ambiguous_rights_policy_source"
    );
}

#[test]
fn protected_content_creator_royalty_share_value_is_pinned() {
    // 0x3b6 = 950. The constant is named tenths-of-percent (which would read
    // as 95%); the deployed Base 8453 AuthorityGateway's actual royalty-share
    // unit is still unproven and tracked by the deployed-facts verification
    // task. This pin exists so any change to the mint's money constant is a
    // reviewed decision instead of silent drift — the exact-call test below
    // re-derives its expectation from the same constant and cannot catch it.
    assert_eq!(PROTECTED_CONTENT_CREATOR_ROYALTY_TENTHS_PERCENT, "0x3b6");
    assert_eq!(
        u64::from_str_radix(
            PROTECTED_CONTENT_CREATOR_ROYALTY_TENTHS_PERCENT.trim_start_matches("0x"),
            16,
        )
        .unwrap(),
        950
    );
}

#[test]
fn resolve_protected_content_creator_mint_returns_exact_call_and_content_access_id() {
    let mut provider = provider_with_creator_mint_rpc("http://127.0.0.1:9".to_string());
    let access_id = [0x41; 16];
    let creator = "0x0000000000000000000000000000000000000011";
    let data = ok_data(
        provider.handle(Request::ResolveProtectedContentCreatorMint {
            creator: creator.to_string(),
            token_uri: "ipfs://protected-content/metadata.json".to_string(),
            content_access_id: format!("0x{}", encode_hex(&access_id)),
            copies: "0x7".to_string(),
            price: "0x5".to_string(),
        }),
    );
    assert_eq!(data["schema"], PROTECTED_CONTENT_CREATOR_MINT_SCHEMA);
    assert_eq!(data["network"], "base-local");
    assert_eq!(data["chain_namespace"], "eip155:8453");
    assert_eq!(
        data["function"],
        ProtectedContentCreatorMintAbi::ElacityMintV1.function()
    );
    assert_eq!(data["ledger"], "0x0000000000000000000000000000000000000022");
    assert_eq!(
        data["pay_token"],
        "0x0000000000000000000000000000000000000033"
    );
    assert_eq!(data["to"], "0x0000000000000000000000000000000000000022");
    assert_eq!(data["value"], "0x0");
    assert_eq!(
        data["content_access_id"],
        format!("0x{}", encode_hex(&access_id))
    );
    assert_eq!(
        data["data"],
        encode_protected_content_creator_mint_call(
            ProtectedContentCreatorMintAbi::ElacityMintV1.selector(),
            "ipfs://protected-content/metadata.json",
            PROTECTED_CONTENT_CREATOR_BUY_ONCE_OP_TYPE,
            &encode_protected_content_mint_op_raw_paid(
                &access_id,
                "ipfs://protected-content/metadata.json",
                &[creator.to_string(), creator.to_string()],
                &[
                    PROTECTED_CONTENT_CREATOR_ACCESS_TOKEN_ROLE,
                    PROTECTED_CONTENT_CREATOR_ROYALTY_SHARE_ROLE,
                ],
                &[
                    "0x7".to_string(),
                    PROTECTED_CONTENT_CREATOR_ROYALTY_TENTHS_PERCENT.to_string(),
                ],
                None,
            )
            .unwrap(),
            &encode_protected_content_sell_raw_data(
                "0x7",
                "0x5",
                "0x0000000000000000000000000000000000000033",
            )
            .unwrap(),
        )
        .unwrap()
    );
    assert!(data.get("reseller_cut").is_none());
}

#[test]
fn describe_protected_content_creator_mint_source_returns_exact_configured_facts() {
    let mut provider = provider_with_creator_mint_rpc("http://127.0.0.1:9".to_string());
    let data = ok_data(provider.handle(Request::DescribeProtectedContentCreatorMintSource));
    assert_eq!(data["schema"], PROTECTED_CONTENT_CREATOR_MINT_SOURCE_SCHEMA);
    assert_eq!(data["network"], "base-local");
    assert_eq!(data["chain_namespace"], "eip155:8453");
    assert_eq!(data["ledger"], "0x0000000000000000000000000000000000000022");
    assert_eq!(
        data["pay_token"],
        "0x0000000000000000000000000000000000000033"
    );
    assert_eq!(data["abi"], "elacity_mint_v1");
    assert_eq!(
        data["function"],
        ProtectedContentCreatorMintAbi::ElacityMintV1.function()
    );
}

#[test]
fn resolve_protected_content_creator_mint_rejects_missing_or_ambiguous_creator_network() {
    let mut unconfigured = ChainProvider::new();
    let init = unconfigured.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [{
                    "id": "base-local",
                    "display_name": "Base Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 8453,
                    "native_symbol": "ETH",
                    "provider": "test",
                    "mainnet": true,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [],
                    "protected_content_market": {
                        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
                        "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                    }
                }]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    assert_eq!(
        error_code(unconfigured.handle(Request::DescribeProtectedContentCreatorMintSource)),
        "protected_content_creator_mint_not_configured"
    );
    assert_eq!(
        error_code(
            unconfigured.handle(Request::ResolveProtectedContentCreatorMint {
                creator: "0x0000000000000000000000000000000000000011".to_string(),
                token_uri: "ipfs://protected-content/metadata.json".to_string(),
                content_access_id: format!("0x{}", encode_hex(&[0x41; 16])),
                copies: "0x1".to_string(),
                price: "0x5".to_string(),
            })
        ),
        "protected_content_creator_mint_not_configured"
    );

    let mut ambiguous = ChainProvider::new();
    let init = ambiguous.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [
                    {
                        "id": "base-a",
                        "display_name": "Base A",
                        "kind": "evm_json_rpc",
                        "chain_id": 8453,
                        "native_symbol": "ETH",
                        "provider": "test",
                        "mainnet": true,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [],
                        "protected_content_creator_mint": {
                            "ledger": "0x0000000000000000000000000000000000000022",
                            "pay_token": "0x0000000000000000000000000000000000000033",
                            "asset_created_emitter": "0x00000000000000000000000000000000000000dd",
                            "abi": "elacity_mint_v1"
                        }
                    },
                    {
                        "id": "base-b",
                        "display_name": "Base B",
                        "kind": "evm_json_rpc",
                        "chain_id": 8454,
                        "native_symbol": "ETH",
                        "provider": "test",
                        "mainnet": true,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [],
                        "protected_content_creator_mint": {
                            "ledger": "0x0000000000000000000000000000000000000044",
                            "pay_token": "0x0000000000000000000000000000000000000055",
                            "asset_created_emitter": "0x00000000000000000000000000000000000000ee",
                            "abi": "elacity_mint_v1"
                        }
                    }
                ]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    assert_eq!(
        error_code(ambiguous.handle(Request::DescribeProtectedContentCreatorMintSource)),
        "ambiguous_protected_content_creator_mint_source"
    );
    assert_eq!(
        error_code(
            ambiguous.handle(Request::ResolveProtectedContentCreatorMint {
                creator: "0x0000000000000000000000000000000000000011".to_string(),
                token_uri: "ipfs://protected-content/metadata.json".to_string(),
                content_access_id: format!("0x{}", encode_hex(&[0x41; 16])),
                copies: "0x1".to_string(),
                price: "0x5".to_string(),
            })
        ),
        "ambiguous_protected_content_creator_mint_source"
    );
}

#[test]
fn init_rejects_invalid_protected_content_creator_mint_addresses() {
    for (field, value) in [
        ("ledger", "0x1234"),
        ("pay_token", "not-an-address"),
        ("asset_created_emitter", "0x1234"),
    ] {
        let mut creator_mint = json!({
            "ledger": "0x0000000000000000000000000000000000000022",
            "pay_token": "0x0000000000000000000000000000000000000033",
            "asset_created_emitter": "0x00000000000000000000000000000000000000dd",
            "abi": "elacity_mint_v1"
        });
        creator_mint[field] = json!(value);
        let mut provider = ChainProvider::new();
        let response = provider.handle(Request::Init {
            config: json!({
                "extra": {
                    "networks": [{
                        "id": "base-local",
                        "display_name": "Base Local",
                        "kind": "evm_json_rpc",
                        "chain_id": 8453,
                        "native_symbol": "ETH",
                        "provider": "test",
                        "mainnet": true,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [],
                        "protected_content_creator_mint": creator_mint
                    }]
                }
            }),
        });
        assert_eq!(error_code(response), "invalid_config");
    }
}

#[test]
fn resolve_protected_content_mint_receipt_requires_two_finalized_agreeing_receipts() {
    let hash = "0x1111111111111111111111111111111111111111111111111111111111111111";
    let creator = "0x0000000000000000000000000000000000000011";
    let ledger = "0x0000000000000000000000000000000000000022";
    let operative = "0x0000000000000000000000000000000000000044";
    let emitter = "0x00000000000000000000000000000000000000dd";
    let token_uri = "ipfs://protected-content/metadata.json";
    let receipt_block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let finalized_hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let receipt = protected_content_mint_receipt_json(
        hash,
        creator,
        ledger,
        "0x2a",
        receipt_block_hash,
        "0x1",
        vec![protected_content_asset_created_log(
            emitter, creator, ledger, operative, "0x03", token_uri, 0,
        )],
    );
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        ("eth_getTransactionReceipt", json!([hash]), receipt.clone()),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
    ];
    let mut provider = provider_with_creator_mint_rpc_and_market_sources(
        "http://127.0.0.1:9".to_string(),
        vec![
            spawn_rpc_sequence_asserting_server(sequence.clone()),
            spawn_rpc_sequence_asserting_server(sequence),
        ],
    );
    let data = ok_data(
        provider.handle(Request::ResolveProtectedContentMintReceipt {
            network: "base-local".to_string(),
            hash: hash.to_string(),
            creator: creator.to_string(),
            ledger: ledger.to_string(),
            token_uri: token_uri.to_string(),
            op_type_code: 0,
        }),
    );
    assert_eq!(data["schema"], PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA);
    assert_eq!(data["network"], "base-local");
    assert_eq!(data["chain_id"], 8453);
    assert_eq!(data["token_id"], "0x3");
    assert_eq!(data["operative"], operative);
}

#[test]
fn resolve_protected_content_mint_receipt_rejects_invalid_or_ambiguous_receipts() {
    let hash = "0x1111111111111111111111111111111111111111111111111111111111111111";
    let creator = "0x0000000000000000000000000000000000000011";
    let wrong_creator = "0x0000000000000000000000000000000000000099";
    let ledger = "0x0000000000000000000000000000000000000022";
    let operative = "0x0000000000000000000000000000000000000044";
    let wrong_ledger = "0x00000000000000000000000000000000000000aa";
    let emitter = "0x00000000000000000000000000000000000000dd";
    let token_uri = "ipfs://protected-content/metadata.json";
    let receipt_block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let finalized_hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn mutate_asset_created_log_data(mut log: Value, mutate: impl FnOnce(&mut Vec<u8>)) -> Value {
        let data = log
            .get("data")
            .and_then(Value::as_str)
            .expect("asset log data");
        let mut bytes = decode_hex(data, None, "asset log data").expect("decode asset log data");
        mutate(&mut bytes);
        log["data"] = json!(format!("0x{}", encode_hex(&bytes)));
        log
    }

    fn mutate_asset_created_log_topic(
        mut log: Value,
        index: usize,
        mutate: impl FnOnce(&mut Vec<u8>),
    ) -> Value {
        let topic = log["topics"][index].as_str().expect("asset log topic");
        let mut bytes = decode_hex(topic, Some(32), "asset log topic").expect("decode topic");
        mutate(&mut bytes);
        log["topics"][index] = json!(format!("0x{}", encode_hex(&bytes)));
        log
    }

    for (label, receipt, expected_code) in [
        (
            "wrong emitter",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    "0x00000000000000000000000000000000000000ee",
                    creator,
                    ledger,
                    operative,
                    "0x03",
                    token_uri,
                    0,
                )],
            ),
            "protected_content_mint_receipt_not_bound",
        ),
        (
            "wrong creator",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    emitter,
                    wrong_creator,
                    ledger,
                    operative,
                    "0x03",
                    token_uri,
                    0,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "wrong ledger",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    emitter,
                    creator,
                    wrong_ledger,
                    operative,
                    "0x03",
                    token_uri,
                    0,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "ambiguous",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![
                    protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    ),
                    protected_content_asset_created_log(
                        emitter,
                        creator,
                        ledger,
                        "0x0000000000000000000000000000000000000055",
                        "0x04",
                        token_uri,
                        0,
                    ),
                ],
            ),
            "ambiguous_protected_content_mint_receipt",
        ),
        (
            "wrong transaction hash",
            protected_content_mint_receipt_json(
                "0x9999999999999999999999999999999999999999999999999999999999999999",
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    emitter, creator, ledger, operative, "0x03", token_uri, 0,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "malformed status",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x2",
                vec![protected_content_asset_created_log(
                    emitter, creator, ledger, operative, "0x03", token_uri, 0,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "non-canonical creator topic",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![mutate_asset_created_log_topic(
                    protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    ),
                    1,
                    |topic| topic[0] = 1,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "non-canonical token uri offset",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![mutate_asset_created_log_data(
                    protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    ),
                    |data| data[63] = 0x80,
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "non-canonical trailing data",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![mutate_asset_created_log_data(
                    protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    ),
                    |data| data.extend_from_slice(&[0u8; 32]),
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "non-zero token uri padding",
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![mutate_asset_created_log_data(
                    protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    ),
                    |data| {
                        let last = data.last_mut().expect("token uri padding byte");
                        *last = 1;
                    },
                )],
            ),
            "invalid_protected_content_mint_receipt",
        ),
    ] {
        let sequence = vec![
            ("eth_chainId", json!([]), json!("0x2105")),
            ("eth_getTransactionReceipt", json!([hash]), receipt.clone()),
            (
                "eth_getBlockByNumber",
                json!(["0x2a", false]),
                canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
            ),
            (
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                finalized_block_json("0x2b", finalized_hash),
            ),
        ];
        let mut provider = provider_with_creator_mint_rpc_and_market_sources(
            "http://127.0.0.1:9".to_string(),
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        );
        assert_eq!(
            error_code(
                provider.handle(Request::ResolveProtectedContentMintReceipt {
                    network: "base-local".to_string(),
                    hash: hash.to_string(),
                    creator: creator.to_string(),
                    ledger: ledger.to_string(),
                    token_uri: token_uri.to_string(),
                    op_type_code: 0,
                })
            ),
            expected_code,
            "{label}"
        );
    }

    let conflicting_a = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        (
            "eth_getTransactionReceipt",
            json!([hash]),
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    emitter, creator, ledger, operative, "0x03", token_uri, 0,
                )],
            ),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
    ];
    let conflicting_b = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        (
            "eth_getTransactionReceipt",
            json!([hash]),
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![protected_content_asset_created_log(
                    emitter,
                    creator,
                    ledger,
                    "0x0000000000000000000000000000000000000055",
                    "0x04",
                    token_uri,
                    0,
                )],
            ),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
    ];
    let mut conflicting_provider = provider_with_creator_mint_rpc_and_market_sources(
        "http://127.0.0.1:9".to_string(),
        vec![
            spawn_rpc_sequence_asserting_server(conflicting_a),
            spawn_rpc_sequence_asserting_server(conflicting_b),
        ],
    );
    assert_eq!(
        error_code(
            conflicting_provider.handle(Request::ResolveProtectedContentMintReceipt {
                network: "base-local".to_string(),
                hash: hash.to_string(),
                creator: creator.to_string(),
                ledger: ledger.to_string(),
                token_uri: token_uri.to_string(),
                op_type_code: 0,
            })
        ),
        "conflicting_protected_content_mint_receipt_observations"
    );

    for (label, canonical_block, expected_code) in [
        (
            "noncanonical receipt block",
            canonical_block_json(
                "0x2a",
                "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                vec![hash],
            ),
            "invalid_protected_content_mint_receipt",
        ),
        (
            "missing transaction in canonical block",
            canonical_block_json(
                "0x2a",
                receipt_block_hash,
                vec!["0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            ),
            "invalid_protected_content_mint_receipt",
        ),
    ] {
        let sequence = vec![
            ("eth_chainId", json!([]), json!("0x2105")),
            (
                "eth_getTransactionReceipt",
                json!([hash]),
                protected_content_mint_receipt_json(
                    hash,
                    creator,
                    ledger,
                    "0x2a",
                    receipt_block_hash,
                    "0x1",
                    vec![protected_content_asset_created_log(
                        emitter, creator, ledger, operative, "0x03", token_uri, 0,
                    )],
                ),
            ),
            (
                "eth_getBlockByNumber",
                json!(["0x2a", false]),
                canonical_block,
            ),
            (
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                finalized_block_json("0x2b", finalized_hash),
            ),
        ];
        let mut provider = provider_with_creator_mint_rpc_and_market_sources(
            "http://127.0.0.1:9".to_string(),
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        );
        assert_eq!(
            error_code(
                provider.handle(Request::ResolveProtectedContentMintReceipt {
                    network: "base-local".to_string(),
                    hash: hash.to_string(),
                    creator: creator.to_string(),
                    ledger: ledger.to_string(),
                    token_uri: token_uri.to_string(),
                    op_type_code: 0,
                })
            ),
            expected_code,
            "{label}"
        );
    }

    let failed_sequence = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        (
            "eth_getTransactionReceipt",
            json!([hash]),
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x0",
                vec![protected_content_asset_created_log(
                    emitter, creator, ledger, operative, "0x03", token_uri, 0,
                )],
            ),
        ),
    ];
    let mut failed_provider = provider_with_creator_mint_rpc_and_market_sources(
        "http://127.0.0.1:9".to_string(),
        vec![
            spawn_rpc_sequence_asserting_server(failed_sequence.clone()),
            spawn_rpc_sequence_asserting_server(failed_sequence),
        ],
    );
    assert_eq!(
        error_code(
            failed_provider.handle(Request::ResolveProtectedContentMintReceipt {
                network: "base-local".to_string(),
                hash: hash.to_string(),
                creator: creator.to_string(),
                ledger: ledger.to_string(),
                token_uri: token_uri.to_string(),
                op_type_code: 0,
            })
        ),
        "protected_content_mint_receipt_failed"
    );

    let mut extra_topics_log = protected_content_asset_created_log(
        emitter, creator, ledger, operative, "0x03", token_uri, 0,
    );
    extra_topics_log
        .get_mut("topics")
        .and_then(Value::as_array_mut)
        .expect("topics array")
        .push(json!(
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        ));
    let extra_topics_sequence = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        (
            "eth_getTransactionReceipt",
            json!([hash]),
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![extra_topics_log],
            ),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
    ];
    let mut extra_topics_provider = provider_with_creator_mint_rpc_and_market_sources(
        "http://127.0.0.1:9".to_string(),
        vec![
            spawn_rpc_sequence_asserting_server(extra_topics_sequence.clone()),
            spawn_rpc_sequence_asserting_server(extra_topics_sequence),
        ],
    );
    assert_eq!(
        error_code(
            extra_topics_provider.handle(Request::ResolveProtectedContentMintReceipt {
                network: "base-local".to_string(),
                hash: hash.to_string(),
                creator: creator.to_string(),
                ledger: ledger.to_string(),
                token_uri: token_uri.to_string(),
                op_type_code: 0,
            })
        ),
        "invalid_protected_content_mint_receipt"
    );
}

#[test]
fn resolve_protected_content_verified_listing_returns_common_finalized_tuple() {
    let finalized_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let operative = "0x0000000000000000000000000000000000000044";
    let payment_processor = "0x00000000000000000000000000000000000000bb";
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!("0x{:0>64}", operative.trim_start_matches("0x"))),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        operative,
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000007",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000033"
            )),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": operative,
                    "data": encode_operatives_payment_processor_call().unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!(
                "0x{:0>64}",
                payment_processor.trim_start_matches("0x")
            )),
        ),
    ];
    let mut provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(sequence.clone()),
            spawn_rpc_sequence_asserting_server(sequence),
        ]),
    );
    let listing = ok_data(
        provider.handle(Request::ResolveProtectedContentVerifiedListing {
            network: "esc-local".to_string(),
            seller: "0x0000000000000000000000000000000000000011".to_string(),
            ledger: "0x0000000000000000000000000000000000000022".to_string(),
            token_id: "0x03".to_string(),
        }),
    );
    assert_eq!(listing["schema"], PROTECTED_CONTENT_VERIFIED_LISTING_SCHEMA);
    assert_eq!(listing["chain_id"], 20);
    assert_eq!(
        listing["seller"],
        "0x0000000000000000000000000000000000000011"
    );
    assert_eq!(
        listing["ledger"],
        "0x0000000000000000000000000000000000000022"
    );
    assert_eq!(listing["token_id"], "0x3");
    assert_eq!(listing["operative"], operative);
    assert_eq!(listing["quantity"], "0x7");
    assert_eq!(listing["price"], "0x5");
    assert_eq!(
        listing["pay_token"],
        "0x0000000000000000000000000000000000000033"
    );
    assert_eq!(listing["payment_processor"], payment_processor);
}

#[test]
fn resolve_protected_content_verified_listing_rejects_conflicting_sources() {
    let finalized_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let sequence_a = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!("0x0000000000000000000000000000000000000000000000000000000000000044"),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        "0x0000000000000000000000000000000000000044",
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000007",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000033"
            )),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x0000000000000000000000000000000000000044",
                    "data": encode_operatives_payment_processor_call().unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!("0x00000000000000000000000000000000000000000000000000000000000000bb"),
        ),
    ];
    let sequence_b = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!("0x0000000000000000000000000000000000000000000000000000000000000044"),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        "0x0000000000000000000000000000000000000044",
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000007",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000099"
            )),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x0000000000000000000000000000000000000044",
                    "data": encode_operatives_payment_processor_call().unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!("0x00000000000000000000000000000000000000000000000000000000000000bb"),
        ),
    ];
    let mut provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(sequence_a),
            spawn_rpc_sequence_asserting_server(sequence_b),
        ]),
    );
    assert_eq!(
        error_code(
            provider.handle(Request::ResolveProtectedContentVerifiedListing {
                network: "esc-local".to_string(),
                seller: "0x0000000000000000000000000000000000000011".to_string(),
                ledger: "0x0000000000000000000000000000000000000022".to_string(),
                token_id: "0x03".to_string(),
            })
        ),
        "conflicting_protected_content_verified_listing_observations"
    );
}

#[test]
fn resolve_protected_content_verified_listing_rejects_noncanonical_address_word() {
    let finalized_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let mut noncanonical_operative_word =
        abi_word_address("0x0000000000000000000000000000000000000044").unwrap();
    noncanonical_operative_word[0] = 1;
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!("0x{}", encode_hex(&noncanonical_operative_word))),
        ),
    ];
    let mut provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(sequence.clone()),
            spawn_rpc_sequence_asserting_server(sequence),
        ]),
    );
    assert_eq!(
        error_code(
            provider.handle(Request::ResolveProtectedContentVerifiedListing {
                network: "esc-local".to_string(),
                seller: "0x0000000000000000000000000000000000000011".to_string(),
                ledger: "0x0000000000000000000000000000000000000022".to_string(),
                token_id: "0x03".to_string(),
            })
        ),
        "upstream_invalid_protected_content_verified_listing"
    );
}

#[test]
fn resolve_protected_content_purchase_returns_exact_network_target_value_and_data() {
    let finalized_hash = "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let operative = "0x0000000000000000000000000000000000000044";
    let payment_processor = "0x00000000000000000000000000000000000000bb";
    let erc20_sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!("0x{:0>64}", operative.trim_start_matches("0x"))),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        operative,
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "000000000000000000000000000000000000000000000000000000000000270f",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000033"
            )),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": operative,
                    "data": encode_operatives_payment_processor_call().unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!(
                "0x{:0>64}",
                payment_processor.trim_start_matches("0x")
            )),
        ),
    ];
    let mut provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(erc20_sequence.clone()),
            spawn_rpc_sequence_asserting_server(erc20_sequence),
        ]),
    );
    let data = ok_data(provider.handle(Request::ResolveProtectedContentPurchase {
        seller: "0x0000000000000000000000000000000000000011".to_string(),
        chain_namespace: "eip155:20".to_string(),
        network: "esc-local".to_string(),
        ledger: "0x0000000000000000000000000000000000000022".to_string(),
        token_id: "0x03".to_string(),
    }));
    let rendered = serde_json::to_string(&data).unwrap();
    assert!(!rendered.contains("http://127.0.0.1:9"));
    assert_eq!(data["schema"], PROTECTED_CONTENT_PURCHASE_SCHEMA);
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["purchase_quantity"], "0x1");
    assert_eq!(data["verified_listing"]["token_id"], "0x3");
    assert_eq!(data["verified_listing"]["available_quantity"], "0x270f");
    assert_eq!(data["verified_listing"]["price"], "0x5");
    assert_eq!(
        data["verified_listing"]["pay_token"],
        "0x0000000000000000000000000000000000000033"
    );
    assert_eq!(
        data["verified_listing"]["payment_processor"],
        payment_processor
    );
    let steps = data["steps"].as_array().expect("ordered transaction steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["stage"], "approval");
    assert_eq!(steps[0]["to"], "0x0000000000000000000000000000000000000033");
    assert_eq!(steps[0]["value"], "0x0");
    assert_eq!(
        steps[0]["data"],
        encode_erc20_approve_call(payment_processor, "0x5",).unwrap()
    );
    assert_eq!(steps[1]["stage"], "buy");
    assert_eq!(steps[1]["to"], "0x00000000000000000000000000000000000000aa");
    assert_eq!(steps[1]["value"], "0x0");
    assert_eq!(
        steps[1]["data"],
        encode_authority_gateway_buy_access_call(
            PROTECTED_CONTENT_BUY_ACCESS_ERC20_SELECTOR,
            "0x0000000000000000000000000000000000000011",
            "0x0000000000000000000000000000000000000022",
            "0x3",
            "0x1",
            "0x05",
            Some("0x0000000000000000000000000000000000000033"),
        )
        .unwrap()
    );

    let native_sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!("0x{:0>64}", operative.trim_start_matches("0x"))),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        operative,
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000009999",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000000"
            )),
        ),
    ];
    let mut native_provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(native_sequence.clone()),
            spawn_rpc_sequence_asserting_server(native_sequence),
        ]),
    );
    let native = ok_data(
        native_provider.handle(Request::ResolveProtectedContentPurchase {
            seller: "0x0000000000000000000000000000000000000011".to_string(),
            chain_namespace: "eip155:20".to_string(),
            network: "esc-local".to_string(),
            ledger: "0x0000000000000000000000000000000000000022".to_string(),
            token_id: "0x03".to_string(),
        }),
    );
    assert_eq!(native["purchase_quantity"], "0x1");
    assert_eq!(native["verified_listing"]["available_quantity"], "0x9999");
    let native_steps = native["steps"].as_array().expect("native steps");
    assert_eq!(native_steps.len(), 1);
    assert_eq!(native_steps[0]["stage"], "buy");
    assert_eq!(
        native_steps[0]["to"],
        "0x00000000000000000000000000000000000000aa"
    );
    assert_eq!(native_steps[0]["value"], "0x5");
    assert_eq!(
        native_steps[0]["data"],
        encode_authority_gateway_buy_access_call(
            PROTECTED_CONTENT_BUY_ACCESS_NATIVE_SELECTOR,
            "0x0000000000000000000000000000000000000011",
            "0x0000000000000000000000000000000000000022",
            "0x3",
            "0x1",
            "0x05",
            None,
        )
        .unwrap()
    );

    let zero_stock_sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_operative_call(
                        "0x0000000000000000000000000000000000000022",
                        "0x03"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(format!("0x{:0>64}", operative.trim_start_matches("0x"))),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x00000000000000000000000000000000000000aa",
                    "data": encode_authority_gateway_listing_call(
                        operative,
                        "0x0000000000000000000000000000000000000011"
                    ).unwrap()
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000000"
            )),
        ),
    ];
    let mut zero_stock_provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(vec![
            spawn_rpc_sequence_asserting_server(zero_stock_sequence.clone()),
            spawn_rpc_sequence_asserting_server(zero_stock_sequence),
        ]),
    );
    assert_eq!(
        error_code(
            zero_stock_provider.handle(Request::ResolveProtectedContentPurchase {
                seller: "0x0000000000000000000000000000000000000011".to_string(),
                chain_namespace: "eip155:20".to_string(),
                network: "esc-local".to_string(),
                ledger: "0x0000000000000000000000000000000000000022".to_string(),
                token_id: "0x03".to_string(),
            })
        ),
        "protected_content_verified_listing_unavailable"
    );

    let denied = provider.handle(Request::ResolveProtectedContentPurchase {
        seller: "0x0000000000000000000000000000000000000011".to_string(),
        chain_namespace: "eip155:8453".to_string(),
        network: "esc-local".to_string(),
        ledger: "0x0000000000000000000000000000000000000022".to_string(),
        token_id: "0x03".to_string(),
    });
    assert_eq!(
        error_code(denied),
        "invalid_protected_content_purchase_request"
    );
}

#[test]
fn resolve_protected_content_purchase_access_uses_view_policy_source_and_hides_topology() {
    let access_id = content_access_id(0x51);
    let wallet = "0x0000000000000000000000000000000000000007";
    let expected_data =
        encode_has_access_by_content_id_call("0x12345678", access_id.as_bytes(), wallet).unwrap();
    let finalized_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let finalized_timestamp = format!("0x{:x}", RIGHTS_EVIDENCE_NOW - 5);
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x14")),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            json!({
                "number": "0x2c",
                "hash": finalized_hash,
                "timestamp": finalized_timestamp,
            }),
        ),
        (
            "eth_call",
            json!([
                {
                    "to": "0x0000000000000000000000000000000000000001",
                    "data": expected_data
                },
                {
                    "blockHash": finalized_hash,
                    "requireCanonical": true
                }
            ]),
            evm_bool_word(true),
        ),
    ];
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "view",
            vec![
                spawn_rpc_sequence_asserting_server(sequence.clone()),
                spawn_rpc_sequence_asserting_server(sequence),
            ],
        ),
    );
    let data = ok_data(
        provider.handle(Request::ResolveProtectedContentPurchaseAccess {
            request_id: "purchase-access:exact".to_string(),
            network: "esc-local".to_string(),
            wallet: wallet.to_string(),
            content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
        }),
    );
    let rendered = serde_json::to_string(&data).unwrap();
    assert_eq!(data["schema"], PROTECTED_CONTENT_PURCHASE_ACCESS_SCHEMA);
    assert_eq!(data["request_id"], "purchase-access:exact");
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["chain_id"], 20);
    assert_eq!(data["wallet"], wallet);
    assert_eq!(
        data["content_access_id"],
        format!("0x{}", encode_hex(access_id.as_bytes()))
    );
    assert_eq!(data["has_access"], true);
    assert_eq!(data["finalized_block_number"], 44);
    assert_eq!(data["finalized_block_hash"], finalized_hash);
    assert_eq!(data["finalized_block_timestamp"], RIGHTS_EVIDENCE_NOW - 5);
    assert_eq!(data["observed_at"], RIGHTS_EVIDENCE_NOW);
    assert!(!rendered.contains("http://127.0.0.1:9"));
    assert!(!rendered.contains("\"contract\""));
    assert!(!rendered.contains("\"selector\""));
}

#[test]
fn resolve_protected_content_purchase_access_rejects_non_view_policy_source() {
    let access_id = content_access_id(0x51);
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources(
            "download",
            vec![
                "https://rpc-a.example".to_string(),
                "https://rpc-b.example".to_string(),
            ],
        ),
    );
    assert_eq!(
        error_code(
            provider.handle(Request::ResolveProtectedContentPurchaseAccess {
                request_id: "purchase-access:missing-view".to_string(),
                network: "esc-local".to_string(),
                wallet: "0x0000000000000000000000000000000000000007".to_string(),
                content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
            })
        ),
        "protected_content_purchase_access_not_configured"
    );
}

#[test]
fn resolve_protected_content_purchase_access_rejects_stale_or_future_finalized_observation() {
    let access_id = content_access_id(0x51);
    let wallet = "0x0000000000000000000000000000000000000007";
    for finalized_timestamp in [
        RIGHTS_EVIDENCE_NOW - super::PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FINALIZED_AGE_SECS - 1,
        RIGHTS_EVIDENCE_NOW + super::PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FUTURE_SKEW_SECS + 1,
    ] {
        let expected_data =
            encode_has_access_by_content_id_call("0x12345678", access_id.as_bytes(), wallet)
                .unwrap();
        let finalized_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let sequence = vec![
            ("eth_chainId", json!([]), json!("0x14")),
            (
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                json!({
                    "number": "0x2c",
                    "hash": finalized_hash,
                    "timestamp": format!("0x{:x}", finalized_timestamp),
                }),
            ),
            (
                "eth_call",
                json!([
                    {
                        "to": "0x0000000000000000000000000000000000000001",
                        "data": expected_data
                    },
                    {
                        "blockHash": finalized_hash,
                        "requireCanonical": true
                    }
                ]),
                evm_bool_word(true),
            ),
        ];
        let mut provider = provider_with_rights_rpc_and_policies(
            "http://127.0.0.1:9".to_string(),
            "0x12345678",
            protected_content_policy_sources(
                "view",
                vec![
                    spawn_rpc_sequence_asserting_server(sequence.clone()),
                    spawn_rpc_sequence_asserting_server(sequence),
                ],
            ),
        );
        assert_eq!(
            error_code(
                provider.handle(Request::ResolveProtectedContentPurchaseAccess {
                    request_id: "purchase-access:stale".to_string(),
                    network: "esc-local".to_string(),
                    wallet: wallet.to_string(),
                    content_access_id: format!("0x{}", encode_hex(access_id.as_bytes())),
                })
            ),
            "stale_protected_content_purchase_access_observation"
        );
    }
}

#[test]
fn init_rejects_unknown_protected_content_market_selector_field() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [],
                    "protected_content_market": {
                        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
                        "selector": "0x0ede2294"
                    }
                }]
            }
        }),
    });
    assert_eq!(error_code(response), "invalid_config");
}

#[test]
fn init_rejects_unknown_protected_content_market_payment_processor_field() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [],
                    "protected_content_market": {
                        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
                        "payment_processor": "0x00000000000000000000000000000000000000bb",
                        "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                    }
                }]
            }
        }),
    });
    assert_eq!(error_code(response), "invalid_config");
}

#[test]
fn init_rejects_stale_protected_content_purchase_config_field() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [],
                    "protected_content_purchase": {
                        "authority_gateway_contract": "0x00000000000000000000000000000000000000aa",
                        "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                    }
                }]
            }
        }),
    });
    assert_eq!(error_code(response), "invalid_config");
}

#[test]
fn init_rejects_invalid_or_duplicate_protected_content_policy_sources() {
    for invalid_policies in [
        json!([{
            "action": "annotate",
            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
        }]),
        json!([{
            "action": "view",
            "evidence_rpc_urls": ["https://rpc-a.example"]
        }]),
        json!([{
            "action": "view",
            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-a.example"]
        }]),
        json!([{
            "action": "view",
            "evidence_rpc_urls": ["https://rpc-a.example", " https://rpc-a.example "]
        }]),
        json!([{
            "action": "view",
            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-a.example/"]
        }]),
        json!([{
            "action": "view",
            "min_confirmations": 12,
            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
        }]),
    ] {
        let mut provider = ChainProvider::new();
        let response = provider.handle(Request::Init {
            config: json!({
                "extra": {
                    "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                    "networks": [{
                        "id": "esc-local",
                        "display_name": "ESC Local",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "test",
                        "mainnet": false,
                        "explorer_url": null,
                        "rpc_url": "http://127.0.0.1:9",
                        "rights_methods": [{
                            "id": "has_access_by_content_id",
                            "contract": "0x0000000000000000000000000000000000000001",
                            "abi": "has_access_by_content_id_address_bytes16",
                            "selector": "0x12345678",
                            "protected_content_policies": invalid_policies
                        }]
                    }]
                }
            }),
        });
        assert_eq!(error_code(response), "invalid_config");
    }

    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [
                        {
                            "id": "has_access_by_content_id",
                            "contract": "0x0000000000000000000000000000000000000001",
                            "abi": "has_access_by_content_id_address_bytes16",
                            "selector": "0x12345678",
                            "protected_content_policies": [{
                                "action": "view",
                                "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                            }]
                        },
                        {
                            "id": "has_access_by_content_id",
                            "contract": "0x0000000000000000000000000000000000000002",
                            "abi": "has_access_by_content_id_address_bytes16",
                            "selector": "0x87654321",
                            "protected_content_policies": [{
                                "action": "view",
                                "evidence_rpc_urls": ["https://rpc-c.example", "https://rpc-d.example"]
                            }]
                        }
                    ]
                }]
            }
        }),
    });
    assert_eq!(error_code(response), "invalid_config");

    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42),
                "networks": [{
                    "id": "esc-local",
                    "display_name": "ESC Local",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [{
                        "id": "has_access_by_content_id",
                        "contract": "0x0000000000000000000000000000000000000001",
                        "abi": "has_access_by_content_id_string_address_string",
                        "selector": "0x12345678",
                        "protected_content_policies": [{
                            "action": "view",
                            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                        }]
                    }]
                }]
            }
        }),
    });
    assert_eq!(error_code(response), "invalid_config");
}

#[test]
fn protected_content_rights_evidence_rejects_malformed_or_oversized_contract_bytes() {
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: "0xABCDEF".to_string(),
        })),
        "invalid_runtime_operation"
    );

    let oversized = format!(
        "0x{}",
        "00".repeat(MAX_PROTECTED_CONTENT_RUNTIME_OPERATION_BYTES + 1)
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: oversized,
        })),
        "invalid_runtime_operation"
    );

    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: "0x00".to_string(),
        })),
        "invalid_runtime_operation"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_unconfigured_selector_without_backend() {
    let operation = protected_content_signed_operation();
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x87654321",
        protected_content_policy_sources(
            "view",
            vec![
                "https://rpc-a.example".to_string(),
                "https://rpc-b.example".to_string(),
            ],
        ),
    );
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "rights_query_not_configured"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_ambiguous_configured_sources() {
    let operation = protected_content_signed_operation();
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "extra": {
                "protected_content_runtime_issuer": runtime_issuer_hex(0x42)
            },
            "networks": [
                {
                    "id": "esc-a",
                    "display_name": "ESC A",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [{
                        "id": "has_access_by_content_id",
                        "contract": "0x0000000000000000000000000000000000000001",
                        "abi": "has_access_by_content_id_address_bytes16",
                        "selector": "0x12345678",
                        "protected_content_policies": [{
                            "action": "view",
                            "evidence_rpc_urls": ["https://rpc-a.example", "https://rpc-b.example"]
                        }]
                    }]
                },
                {
                    "id": "esc-b",
                    "display_name": "ESC B",
                    "kind": "evm_json_rpc",
                    "chain_id": 20,
                    "native_symbol": "ELA",
                    "provider": "test",
                    "mainnet": false,
                    "explorer_url": null,
                    "rpc_url": "http://127.0.0.1:9",
                    "rights_methods": [{
                        "id": "has_access_by_content_id",
                        "contract": "0x0000000000000000000000000000000000000001",
                        "abi": "has_access_by_content_id_address_bytes16",
                        "selector": "0x12345678",
                        "protected_content_policies": [{
                            "action": "view",
                            "evidence_rpc_urls": ["https://rpc-c.example", "https://rpc-d.example"]
                        }]
                    }]
                }
            ]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider.now_unix_seconds = rights_evidence_now;
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "ambiguous_rights_evidence_source"
    );
}

#[test]
fn protected_content_rights_evidence_sanitizes_upstream_rpc_errors() {
    let operation = protected_content_signed_operation();
    let rpc_url = spawn_rpc_sequence_asserting_server_with_replies(vec![(
        "eth_chainId",
        json!([]),
        RpcReply::Error(json!({
            "code": -32000,
            "message": "http://user:secret@127.0.0.1:8545 leaked upstream body"
        })),
    )]);
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources("view", vec![rpc_url, "http://127.0.0.1:9".to_string()]),
    );
    match provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "insufficient_rights_observations");
            for forbidden in ["secret", "user", "127.0.0.1", "8545", "body"] {
                assert!(
                    !message.contains(forbidden),
                    "provider error leaked {forbidden}: {message}"
                );
            }
        }
        other => panic!("expected sanitized error, got {other:?}"),
    }
}

#[test]
fn resolve_protected_content_mint_receipt_rejects_huge_token_uri_length_without_panicking() {
    let hash = "0x1111111111111111111111111111111111111111111111111111111111111111";
    let creator = "0x0000000000000000000000000000000000000011";
    let ledger = "0x0000000000000000000000000000000000000022";
    let operative = "0x0000000000000000000000000000000000000044";
    let emitter = "0x00000000000000000000000000000000000000dd";
    let token_uri = "ipfs://protected-content/metadata.json";
    let receipt_block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let finalized_hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let huge_length_log = mutated_asset_created_log_data(
        protected_content_asset_created_log(
            emitter, creator, ledger, operative, "0x03", token_uri, 0,
        ),
        |data| {
            data[96..128].fill(0xff);
        },
    );
    let sequence = vec![
        ("eth_chainId", json!([]), json!("0x2105")),
        (
            "eth_getTransactionReceipt",
            json!([hash]),
            protected_content_mint_receipt_json(
                hash,
                creator,
                ledger,
                "0x2a",
                receipt_block_hash,
                "0x1",
                vec![huge_length_log],
            ),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            canonical_block_json("0x2a", receipt_block_hash, vec![hash]),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2b", finalized_hash),
        ),
    ];
    let mut provider = provider_with_creator_mint_rpc_and_market_sources(
        "http://127.0.0.1:9".to_string(),
        vec![
            spawn_rpc_sequence_asserting_server(sequence.clone()),
            spawn_rpc_sequence_asserting_server(sequence),
        ],
    );
    assert_eq!(
        error_code(
            provider.handle(Request::ResolveProtectedContentMintReceipt {
                network: "base-local".to_string(),
                hash: hash.to_string(),
                creator: creator.to_string(),
                ledger: ledger.to_string(),
                token_uri: token_uri.to_string(),
                op_type_code: 0,
            })
        ),
        "invalid_protected_content_mint_receipt"
    );
}

#[test]
fn init_rejects_rpc_url_userinfo_for_all_rpc_kinds() {
    let mut provider = ChainProvider::new();
    let evm = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "esc-local",
                "display_name": "ESC Local",
                "kind": "evm_json_rpc",
                "chain_id": 20,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": "http://user:secret@127.0.0.1:8545"
            }]
        }),
    });
    assert_eq!(error_code(evm), "invalid_config");

    let mut provider = ChainProvider::new();
    let bitcoin = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_core_rpc",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "Bitcoin Core",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": "http://user:secret@127.0.0.1:18443"
            }]
        }),
    });
    assert_eq!(error_code(bitcoin), "invalid_config");
}

#[test]
fn protected_content_rights_evidence_rejects_legacy_or_injected_fields() {
    let operation = protected_content_signed_operation();
    for value in [
        json!({
            "op": "protected_content_rights_evidence",
            "signed_runtime_release_operation": contract_hex(&operation),
            "observed_evidence": { "has_access": true }
        }),
        json!({
            "op": "protected_content_rights_evidence",
            "signed_runtime_release_operation": contract_hex(&operation),
            "has_access": true
        }),
        json!({
            "op": "protected_content_rights_evidence",
            "network": "esc-local",
            "signed_runtime_release_operation": contract_hex(&operation)
        }),
        json!({
            "op": "protected_content_has_access_by_content_id",
            "signed_runtime_release_operation": contract_hex(&operation)
        }),
        json!({
            "op": "has_access_by_content_id",
            "network": "esc-local",
            "contract": "0x0000000000000000000000000000000000000001",
            "content_id": "bafybeigprotectedcontent",
            "subject": "0x0000000000000000000000000000000000000002",
            "right": "view"
        }),
    ] {
        assert!(
            serde_json::from_value::<Request>(value).is_err(),
            "protected-content chain request accepted caller-supplied evidence or legacy op"
        );
    }
}

#[test]
fn init_rejects_rights_methods_on_non_evm_networks() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_rest",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": "https://mempool.space/api",
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": "0x0000000000000000000000000000000000000001",
                    "abi": "has_access_by_content_id_address_bytes16",
                    "selector": "0x12345678"
                }]
            }]
        }),
    });

    assert_eq!(error_code(response), "invalid_config");
}
