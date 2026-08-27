use super::*;
use ed25519_dalek::{Signer as _, SigningKey};
use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
    CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1,
    CustodyEpochStatementV1, CustodyPoolIdentityV1, Digest32, EncryptedContentIdentityV1,
    EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1, HpkeCiphertextV1,
    KeyEnvelopeIdentityV1, KeyReleaseRequestV1, NodeCustodyPublicKeyV1, NodePublicKey,
    ProfileIdentityV1, ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1,
    RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1,
    RightsEvaluationEvidenceRequestV1, RightsEvaluationEvidenceV1, RightsObservationFinalityV1,
    RightsPolicyBodyV1, RightsSubjectSourceV1, RuntimeOperationIssuerKeyV1,
    RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1,
    ShareCoordinateV1, SignedCustodyEpochV1, SignedRecipientKeyAuthorizationV1,
    SignedRuntimeReleaseOperationV1, ThresholdV1, WalletAddress, WalletSignedRightsRequestV1,
    CUSTODY_HPKE_SUITE_ID_V1, HPKE_ENCAPPED_KEY_BYTES, HPKE_SEALED_SHARE_BYTES,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use k256::ecdsa::SigningKey as WalletSigningKey;
use sha3::Digest as _;

mod support;

use support::*;

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
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
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
                    "rpc_url": rpc_url,
                    "rights_methods": [{
                        "id": "has_access_by_content_id",
                        "contract": "0x0000000000000000000000000000000000000001",
                        "abi": "has_access_by_content_id_string_address_string",
                        "selector": selector
                    }]
                }]
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW;
    provider
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
                        "abi": "has_access_by_content_id_string_address_string",
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

fn protected_content_policy_and_request(
    min_confirmations: u16,
) -> (RightsPolicyBodyV1, RightsEvaluationEvidenceRequestV1) {
    let mut contract = [0u8; 20];
    contract[19] = 1;
    let policy = RightsPolicyBodyV1::new(
        "bafybeigprotectedcontent",
        RightsActionV1::View,
        "view",
        RightsSubjectSourceV1::WalletAddress,
        20,
        EvmContractAddressV1::new(contract).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
        RightsObservationFinalityV1::new(min_confirmations),
    )
    .unwrap();
    let profile_key = SigningKey::from_bytes(&[0x43; 32]);
    let binding = ProtectedContentBindingV1::new(
        EncryptedContentIdentityV1::new(Digest32::new([0x11; 32]), 2048).unwrap(),
        KeyEnvelopeIdentityV1::new(
            EncryptedContentIdentityV1::new(Digest32::new([0x11; 32]), 2048).unwrap(),
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

fn evm_bool_word(value: bool) -> Value {
    let mut bytes = [0u8; 32];
    bytes[31] = u8::from(value);
    json!(format!("0x{}", encode_hex(&bytes)))
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
    let mut bytes = [0u8; 32];
    bytes[0] = seed.max(9);
    RecipientPublicKeyBytesV1::new(bytes).unwrap()
}

fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
    recipient_public_key(seed)
        .key_identity(CUSTODY_HPKE_SUITE_ID_V1)
        .unwrap()
}

fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let nodes = vec![
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(1),
            NodeCustodyPublicKeyV1::new([0x31; 32]).unwrap(),
            ShareCoordinateV1::new(1).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(2),
            NodeCustodyPublicKeyV1::new([0x32; 32]).unwrap(),
            ShareCoordinateV1::new(2).unwrap(),
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
            node_public_key(3),
            NodeCustodyPublicKeyV1::new([0x33; 32]).unwrap(),
            ShareCoordinateV1::new(3).unwrap(),
        )
        .unwrap(),
    ];
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
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
        EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
        CustodyPoolIdentityV1::new(digest(0x25), 512).unwrap(),
        epoch.epoch_identity().unwrap(),
        CustodyCommitteeAuthorizationIdentityV1::new(digest(0x26), 512).unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        digest(0x33),
        epoch.statement().nodes().to_vec(),
    )
    .unwrap();
    let shares = [0x50, 0x51, 0x52]
        .into_iter()
        .map(|seed| {
            let mut encapped_key = [0u8; HPKE_ENCAPPED_KEY_BYTES];
            encapped_key[0] = seed;
            let mut ciphertext = [0u8; HPKE_SEALED_SHARE_BYTES];
            ciphertext.fill(seed);
            HpkeCiphertextV1::new(encapped_key, ciphertext).unwrap()
        })
        .collect();
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

fn protected_content_signed_operation(min_confirmations: u16) -> SignedRuntimeReleaseOperationV1 {
    signed_runtime_operation_for_policy(protected_content_policy_and_request(min_confirmations).0)
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
    let operation = protected_content_signed_operation(0);
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
    let operation = protected_content_signed_operation(0);
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
    let operation = protected_content_signed_operation(0);
    let bad_signature_operation =
        SignedRuntimeReleaseOperationV1::new(operation.statement().clone(), vec![0; 64]).unwrap();
    let wrong_issuer_operation = signed_runtime_operation_for_policy_and_runtime_seed(
        protected_content_policy_and_request(0).0,
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
fn protected_content_rights_evidence_returns_canonical_evidence_at_observed_block() {
    let operation = protected_content_signed_operation(3);
    let policy = operation.statement().policy_body();
    let request = operation.statement().evidence_request();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_id(),
        &wallet_subject_hex(&operation),
        policy.evm_right_argument(),
    )
    .unwrap();
    let block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("0x2a")),
        (
            "eth_getBlockByNumber",
            json!(["0x27", false]),
            json!({ "number": "0x27", "hash": block_hash }),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": block_hash, "requireCanonical": true }
            ]),
            evm_bool_word(true),
        ),
    ]);
    let mut provider = provider_with_rights_rpc_at(rpc_url, "0x12345678", || RIGHTS_EVIDENCE_NOW);

    let data = ok_data(provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }));

    assert_eq!(
        data["schema"],
        "elastos.chain.protected-content-rights-evidence/v1"
    );
    assert_eq!(data["chain_id"], 20);
    assert_eq!(data["observed_block_number"], 39);
    assert_eq!(data["head_block_number"], 42);
    assert_eq!(data["observed_block_hash"], block_hash);
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
    assert_eq!(evidence.observed_block_number(), 39);
    assert_eq!(evidence.head_block_number(), 42);
}

#[test]
fn protected_content_rights_evidence_captures_denial_without_caller_supplied_result() {
    let operation = protected_content_signed_operation(0);
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_id(),
        &wallet_subject_hex(&operation),
        policy.evm_right_argument(),
    )
    .unwrap();
    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("0x2a")),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            json!({ "number": "0x2a", "hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "requireCanonical": true }
            ]),
            evm_bool_word(false),
        ),
    ]);
    let mut provider = provider_with_rights_rpc_at(rpc_url, "0x12345678", || RIGHTS_EVIDENCE_NOW);

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
fn protected_content_rights_evidence_rejects_stale_or_mismatched_authority() {
    let operation = protected_content_signed_operation(5);
    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("0x3")),
    ]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "insufficient_finality"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_wrong_chain_block_and_hash() {
    let operation = protected_content_signed_operation(0);
    let policy = operation.statement().policy_body();
    let wrong_chain_policy = RightsPolicyBodyV1::new(
        policy.content_id(),
        RightsActionV1::View,
        policy.evm_right_argument(),
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

    let rpc_url =
        spawn_rpc_sequence_asserting_server(vec![("eth_chainId", json!([]), json!("0x15"))]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "chain_id_mismatch"
    );

    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("not-a-quantity")),
    ]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "upstream_invalid_head"
    );

    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("0x2a")),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            json!({ "number": "0x2b", "hash": "0x1234" }),
        ),
    ]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "upstream_invalid_block"
    );

    let rpc_url = spawn_rpc_sequence_asserting_server(vec![
        ("eth_chainId", json!([]), json!("0x14")),
        ("eth_blockNumber", json!([]), json!("0x2a")),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            json!({ "number": "0x2a", "hash": "0x1234" }),
        ),
    ]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "upstream_invalid_block"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_reorged_observed_hash() {
    let operation = protected_content_signed_operation(0);
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_id(),
        &wallet_subject_hex(&operation),
        policy.evm_right_argument(),
    )
    .unwrap();
    let observed_hash = "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let rpc_url = spawn_rpc_sequence_asserting_server_with_replies(vec![
        ("eth_chainId", json!([]), RpcReply::Result(json!("0x14"))),
        (
            "eth_blockNumber",
            json!([]),
            RpcReply::Result(json!("0x2a")),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x2a", false]),
            RpcReply::Result(json!({ "number": "0x2a", "hash": observed_hash })),
        ),
        (
            "eth_call",
            json!([
                { "to": "0x0000000000000000000000000000000000000001", "data": expected_data },
                { "blockHash": observed_hash, "requireCanonical": true }
            ]),
            RpcReply::Error(json!({
                "code": -32001,
                "message": "requested block hash is not canonical"
            })),
        ),
    ]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "upstream_rpc_error"
    );
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
fn protected_content_rights_evidence_rejects_selector_mismatch_without_backend() {
    let operation = protected_content_signed_operation(0);
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x87654321");
    assert_eq!(
        error_code(provider.handle(Request::ProtectedContentRightsEvidence {
            signed_runtime_release_operation: contract_hex(&operation),
        })),
        "rights_selector_mismatch"
    );
}

#[test]
fn protected_content_rights_evidence_rejects_ambiguous_configured_sources() {
    let operation = protected_content_signed_operation(0);
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
                        "abi": "has_access_by_content_id_string_address_string",
                        "selector": "0x12345678"
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
                        "abi": "has_access_by_content_id_string_address_string",
                        "selector": "0x12345678"
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
    let operation = protected_content_signed_operation(0);
    let rpc_url = spawn_rpc_sequence_asserting_server_with_replies(vec![(
        "eth_chainId",
        json!([]),
        RpcReply::Error(json!({
            "code": -32000,
            "message": "http://user:secret@127.0.0.1:8545 leaked upstream body"
        })),
    )]);
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");
    match provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "upstream_rpc_error");
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
    let operation = protected_content_signed_operation(0);
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
                    "abi": "has_access_by_content_id_string_address_string",
                    "selector": "0x12345678"
                }]
            }]
        }),
    });

    assert_eq!(error_code(response), "invalid_config");
}
