use super::super::support::*;
use super::super::*;
use elastos_protected_content_contracts::{
    AtomicReplayClaimer, CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1,
    CustodyPoolIdentityV1, Digest32, EncryptedContentIdentityV1, EvmContractAddressV1,
    EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyEnvelopeIdentityV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyIdentityV1, ReplayClaimError, ReplayClaimKeyV1,
    ReplayNonce16, RightsActionV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
    RightsSubjectSourceV1, RightsVerificationContextV1, RuntimeSessionBindingV1, ThresholdV1,
    WalletAddress, WalletSignedRightsRequestV1,
};
use elastos_wallet_contract::{
    ProtectedContentRightsSignatureResultV1, PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT,
    PROTECTED_CONTENT_RIGHTS_SIGNATURE_RESOURCE, PROTECTED_CONTENT_RIGHTS_SIGNATURE_RESULT_SCHEMA,
};
use k256::ecdsa::SigningKey;
use serde_json::Map;

struct TestReplay;

impl AtomicReplayClaimer for TestReplay {
    fn claim(
        &mut self,
        _key: ReplayClaimKeyV1,
        _expires_at: u64,
        _now: u64,
    ) -> Result<(), ReplayClaimError> {
        Ok(())
    }
}

fn digest(seed: u8) -> Digest32 {
    Digest32::new([seed; 32])
}

fn wallet_address(address: &str) -> WalletAddress {
    let hex = address.strip_prefix("0x").unwrap_or(address);
    let bytes = hex::decode(hex).expect("test address hex");
    WalletAddress::new(bytes.try_into().expect("20-byte EVM address"))
}

#[derive(Clone)]
struct RightsRequestFixture {
    wallet: WalletAddress,
    action: RightsActionV1,
    issued_at: u64,
    expires_at: u64,
    nonce: u8,
    encrypted_content_seed: u8,
    envelope_seed: u8,
    node_set_seed: u8,
    pool_seed: u8,
    epoch_seed: u8,
    committee_authorization_seed: u8,
    policy_content_seed: u8,
    profile: ProfileIdentityV1,
    session_seed: u8,
    recipient_seed: u8,
}

impl RightsRequestFixture {
    fn new(
        wallet: WalletAddress,
        action: RightsActionV1,
        issued_at: u64,
        expires_at: u64,
        nonce: u8,
    ) -> Self {
        Self {
            wallet,
            action,
            issued_at,
            expires_at,
            nonce,
            encrypted_content_seed: 0x11,
            envelope_seed: 0x22,
            node_set_seed: 0x23,
            pool_seed: 0x24,
            epoch_seed: 0x25,
            committee_authorization_seed: 0x26,
            policy_content_seed: 0x27,
            profile: ProfileIdentityV1::from_did_key(
                "did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj",
            )
            .unwrap(),
            session_seed: 0x66,
            recipient_seed: 0xa0,
        }
    }

    fn build(&self) -> RightsRequestV1 {
        let encrypted_content =
            EncryptedContentIdentityV1::new(digest(self.encrypted_content_seed), 4096).unwrap();
        let key_envelope = KeyEnvelopeIdentityV1::new(
            encrypted_content.clone(),
            digest(self.envelope_seed),
            2048,
            digest(self.node_set_seed),
            ThresholdV1::new(2, 3).unwrap(),
            CustodyPoolIdentityV1::new(digest(self.pool_seed), 512).unwrap(),
            CustodyEpochIdentityV1::new(digest(self.epoch_seed), 512).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(
                digest(self.committee_authorization_seed),
                512,
            )
            .unwrap(),
        )
        .unwrap();
        let policy = RightsPolicyBodyV1::new(
            format!(
                "elastos-content:wallet-rights-test-{:02x}",
                self.policy_content_seed
            ),
            RightsActionV1::View,
            "view",
            RightsSubjectSourceV1::WalletAddress,
            20,
            EvmContractAddressV1::new([0x44; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(1),
        )
        .unwrap();
        let binding = ProtectedContentBindingV1::new(
            encrypted_content,
            key_envelope,
            policy.policy_identity().unwrap(),
            self.profile,
            self.wallet,
            RuntimeSessionBindingV1::new(digest(self.session_seed)).unwrap(),
        )
        .unwrap();
        RightsRequestV1::new(
            binding,
            self.action,
            RecipientKeyIdentityV1::new("x25519-hkdf-sha256-hpke-v1", digest(self.recipient_seed))
                .unwrap(),
            self.issued_at,
            self.expires_at,
            ReplayNonce16::new([self.nonce; 16]),
        )
        .unwrap()
    }
}

fn alternate_profile_identity() -> ProfileIdentityV1 {
    let mut compressed_edwards_y = [0x66; 32];
    compressed_edwards_y[0] = 0x58;
    ProfileIdentityV1::from_public_key_bytes(compressed_edwards_y).unwrap()
}

fn protected_content_rights_request(
    wallet: WalletAddress,
    action: RightsActionV1,
    issued_at: u64,
    expires_at: u64,
    nonce: u8,
) -> RightsRequestV1 {
    RightsRequestFixture::new(wallet, action, issued_at, expires_at, nonce).build()
}

fn operation_for(account_id: &str, request: &RightsRequestV1) -> WalletProviderOperationV2 {
    WalletProviderOperationV2::RequestProtectedContentRightsSignature {
        account_id: account_id.to_string(),
        canonical_rights_request_hex: hex::encode(request.canonical_bytes().unwrap()),
        reason: "Review and sign the exact protected-content rights request".to_string(),
    }
}

fn create_managed_evm_account(
    provider: &mut WalletProvider,
    principal_id: &str,
) -> (String, String) {
    match invoke_wallet(
        provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed EVM account, got {other:?}"),
    }
}

fn link_external_evm_account(
    provider: &mut WalletProvider,
    principal_id: &str,
    signing_key: &SigningKey,
) -> (String, String) {
    let address = test_address(signing_key);
    let account_id = format!("wallet:eip155:20:{address}");
    match invoke_wallet(
        provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: format!("proof:eip155:20:{address}"),
            chain_namespace: "eip155:20".into(),
            address: address.clone(),
            proof_type: "siwe".into(),
            label: Some("External signer".into()),
        },
    ) {
        Response::Ok { .. } => (account_id, address),
        other => panic!("expected linked external EVM account, got {other:?}"),
    }
}

fn request_protected_content_signature(
    provider: &mut WalletProvider,
    principal_id: &str,
    actor: &str,
    account_id: &str,
    request: &RightsRequestV1,
) -> (String, String) {
    match invoke_wallet(
        provider,
        principal_id,
        actor,
        operation_for(account_id, request),
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["requires_approval"], true);
            assert_eq!(data["signature"], Value::Null);
            let approval = &data["approval_request"];
            assert_eq!(
                approval["intent"],
                PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT
            );
            assert_eq!(
                approval["resource"],
                PROTECTED_CONTENT_RIGHTS_SIGNATURE_RESOURCE
            );
            assert_eq!(
                approval["payload"]["canonical_rights_request_hex"],
                hex::encode(request.canonical_bytes().unwrap())
            );
            (
                approval["request_id"].as_str().unwrap().to_string(),
                approval["payload_hash"].as_str().unwrap().to_string(),
            )
        }
        other => panic!("expected protected-content rights approval, got {other:?}"),
    }
}

fn decode_result(result: &Value, expected: &RightsRequestV1) -> WalletSignedRightsRequestV1 {
    let result: ProtectedContentRightsSignatureResultV1 =
        serde_json::from_value(result.clone()).expect("strict protected-content signed result");
    result.validate().unwrap();
    let signed_bytes = hex::decode(result.wallet_signed_rights_request_hex).unwrap();
    let signed = WalletSignedRightsRequestV1::from_canonical_bytes(&signed_bytes).unwrap();
    assert_eq!(signed.request(), expected);
    let mut replay = TestReplay;
    signed
        .verify(
            &RightsVerificationContextV1::new(
                expected.binding().clone(),
                expected.action(),
                expected.recipient().clone(),
                expected.issued_at(),
            ),
            &mut replay,
        )
        .unwrap();
    signed
}

fn make_high_s_evm_signature(signature: &str) -> String {
    const SECP256K1_ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    let mut bytes = hex::decode(signature.strip_prefix("0x").unwrap_or(signature)).unwrap();
    assert_eq!(bytes.len(), 65);
    let mut borrow = 0u16;
    for index in (0..32).rev() {
        let minuend = u16::from(SECP256K1_ORDER[index]);
        let subtrahend = u16::from(bytes[index + 32]) + borrow;
        if minuend >= subtrahend {
            bytes[index + 32] = (minuend - subtrahend) as u8;
            borrow = 0;
        } else {
            bytes[index + 32] = (minuend + 256 - subtrahend) as u8;
            borrow = 1;
        }
    }
    assert_eq!(borrow, 0);
    bytes[64] ^= 1;
    format!("0x{}", hex::encode(bytes))
}

fn expect_error(response: Response, expected_code: &str) -> String {
    match response {
        Response::Error { code, message } => {
            assert_eq!(code, expected_code);
            message
        }
        other => panic!("expected {expected_code} error, got {other:?}"),
    }
}

#[test]
fn protected_content_rights_operation_carries_only_account_request_and_reason() {
    let request = protected_content_rights_request(
        WalletAddress::new([0x33; 20]),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x44,
    );
    let operation = operation_for(
        "wallet:eip155:20:0x3333333333333333333333333333333333333333",
        &request,
    );
    operation.validate().unwrap();
    let encoded = serde_json::to_value(&operation).unwrap();
    assert_eq!(
        encoded["kind"],
        "request_protected_content_rights_signature"
    );
    let params = encoded["params"].as_object().unwrap();
    assert_eq!(
        params.keys().cloned().collect::<Vec<_>>(),
        vec![
            "account_id".to_string(),
            "canonical_rights_request_hex".to_string(),
            "reason".to_string(),
        ]
    );
    assert!(!encoded.to_string().contains("custody_pool"));
    assert!(!encoded.to_string().contains("committee"));
    assert!(!encoded.to_string().contains("profile"));
    assert!(!encoded.to_string().contains("runtime_session"));
}

#[test]
fn protected_content_rights_managed_and_external_sign_identical_canonical_result() {
    let managed_dir = tempfile::tempdir().unwrap();
    let external_dir = tempfile::tempdir().unwrap();
    let principal_id = "person:local:protected-content";
    let mut managed_provider = init_provider(managed_dir.path());
    let mut external_provider = init_provider(external_dir.path());
    let (managed_account_id, managed_address) =
        create_managed_evm_account(&mut managed_provider, principal_id);
    let managed_key = managed_provider
        .managed_signing_key_for_account(&managed_provider.store.accounts[0])
        .unwrap();
    let external_address = test_address(&managed_key);
    assert_eq!(managed_address, external_address);
    let external_account_id = format!("wallet:eip155:20:{external_address}");
    match invoke_wallet(
        &mut external_provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: format!("proof:eip155:20:{external_address}"),
            chain_namespace: "eip155:20".into(),
            address: external_address.clone(),
            proof_type: "siwe".into(),
            label: None,
        },
    ) {
        Response::Ok { .. } => {}
        other => panic!("expected external account link, got {other:?}"),
    }
    let rights_request = protected_content_rights_request(
        wallet_address(&managed_address),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x51,
    );

    let (managed_request_id, _) = request_protected_content_signature(
        &mut managed_provider,
        principal_id,
        "library",
        &managed_account_id,
        &rights_request,
    );
    let managed_result = match invoke_wallet(
        &mut managed_provider,
        principal_id,
        "library",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: managed_request_id.clone(),
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert!(data.get("signature").is_none());
            assert_eq!(data["approval_request"]["status"], "completed");
            data["approval_request"]["signed_result"].clone()
        }
        other => panic!("expected managed protected-content signature, got {other:?}"),
    };

    let (external_request_id, external_payload_hash) = request_protected_content_signature(
        &mut external_provider,
        principal_id,
        "library",
        &external_account_id,
        &rights_request,
    );
    match invoke_wallet(
        &mut external_provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: external_request_id.clone(),
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["handoff"]["signature_type"], "personal_sign");
            assert_eq!(
                data["handoff"]["message"],
                format!(
                    "0x{}",
                    hex::encode(rights_request.canonical_bytes().unwrap())
                )
            );
        }
        other => panic!("expected external protected-content handoff, got {other:?}"),
    }
    let external_signature =
        sign_message_bytes(&managed_key, &rights_request.canonical_bytes().unwrap());
    let external_result = match invoke_wallet(
        &mut external_provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id: external_request_id,
            payload_hash: external_payload_hash,
            signature: Some(external_signature),
            signature_type: Some("personal_sign".to_string()),
            public_key: None,
            signer: external_address,
            transaction_hash: None,
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["signed_result"].clone(),
        other => panic!("expected external protected-content completion, got {other:?}"),
    };

    assert_eq!(managed_result, external_result);
    decode_result(&managed_result, &rights_request);
    decode_result(&external_result, &rights_request);
}

#[test]
fn protected_content_rights_rejects_wrong_missing_revoked_non_evm_and_time_windows() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:protected-content-negative";
    let (account_id, address) = create_managed_evm_account(&mut provider, principal_id);
    let valid = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x61,
    );

    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(
                "wallet:eip155:20:0x9999999999999999999999999999999999999999",
                &valid,
            ),
        ),
        "not_found",
    );

    let wrong_wallet = protected_content_rights_request(
        WalletAddress::new([0x99; 20]),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x62,
    );
    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(&account_id, &wrong_wallet),
        ),
        "invalid_request",
    );

    let future = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::View,
        now_ts().saturating_add(RIGHTS_CLOCK_SKEW_SECS + 30),
        now_ts().saturating_add(RIGHTS_CLOCK_SKEW_SECS + 90),
        0x63,
    );
    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(&account_id, &future),
        ),
        "invalid_request",
    );

    let expired = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::View,
        now_ts().saturating_sub(120),
        now_ts().saturating_sub(1),
        0x64,
    );
    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(&account_id, &expired),
        ),
        "invalid_request",
    );

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::RevokeAccount {
            account_id: account_id.clone(),
        },
    ) {
        Response::Ok { .. } => {}
        other => panic!("expected revoke ok, got {other:?}"),
    }
    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(&account_id, &valid),
        ),
        "not_found",
    );

    let btc = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected BTC account, got {other:?}"),
    };
    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "library",
            operation_for(&btc, &valid),
        ),
        "invalid_request",
    );
}

#[test]
fn protected_content_rights_replays_exact_result_and_rejects_substitutions() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:protected-content-replay";
    let (account_id, address) = create_managed_evm_account(&mut provider, principal_id);
    let request = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x71,
    );
    let operation = operation_for(&account_id, &request);
    let context = wallet_context(principal_id, "library");
    let wallet_request = wallet_request(&context, operation.clone());
    let (approval_id, _) = match invoke_wallet_request(&mut provider, &wallet_request) {
        Response::Ok { data: Some(data) } => (
            data["approval_request"]["request_id"]
                .as_str()
                .unwrap()
                .to_string(),
            data["approval_request"]["payload_hash"]
                .as_str()
                .unwrap()
                .to_string(),
        ),
        other => panic!("expected approval request, got {other:?}"),
    };
    let signed_result = match invoke_wallet(
        &mut provider,
        principal_id,
        "library",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: approval_id,
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["signed_result"].clone(),
        other => panic!("expected managed completion, got {other:?}"),
    };

    match invoke_wallet_request(&mut provider, &wallet_request) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["signed_result"], signed_result);
            assert_eq!(
                data["signature_receipt"]["request_id"],
                wallet_request.request_id
            );
        }
        other => panic!("expected exact signed replay, got {other:?}"),
    }

    let changed_action = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::Stream,
        request.issued_at(),
        request.expires_at(),
        0x71,
    );
    let substituted_request = WalletProviderRequestV2::new(
        &context,
        wallet_request.request_id.clone(),
        now_ts(),
        now_ts().saturating_add(120),
        operation_for(&account_id, &changed_action),
    )
    .unwrap();
    expect_error(
        invoke_wallet_request(&mut provider, &substituted_request),
        "approval_identity_conflict",
    );

    let base_fixture = RightsRequestFixture::new(
        wallet_address(&address),
        RightsActionV1::View,
        request.issued_at(),
        request.expires_at(),
        0x71,
    );
    let mut substitutions = Vec::new();
    let mut changed = base_fixture.clone();
    changed.encrypted_content_seed = 0x31;
    substitutions.push(("encrypted content", changed.build()));
    let mut changed = base_fixture.clone();
    changed.envelope_seed = 0x32;
    substitutions.push(("key-envelope", changed.build()));
    let mut changed = base_fixture.clone();
    changed.node_set_seed = 0x33;
    substitutions.push(("node set", changed.build()));
    let mut changed = base_fixture.clone();
    changed.pool_seed = 0x34;
    substitutions.push(("custody pool", changed.build()));
    let mut changed = base_fixture.clone();
    changed.epoch_seed = 0x35;
    substitutions.push(("custody epoch", changed.build()));
    let mut changed = base_fixture.clone();
    changed.committee_authorization_seed = 0x36;
    substitutions.push(("committee authorization", changed.build()));
    let mut changed = base_fixture.clone();
    changed.policy_content_seed = 0x37;
    substitutions.push(("policy identity", changed.build()));
    let mut changed = base_fixture.clone();
    changed.profile = alternate_profile_identity();
    substitutions.push(("Profile identity", changed.build()));
    let mut changed = base_fixture.clone();
    changed.wallet = WalletAddress::new([0x38; 20]);
    substitutions.push(("Wallet identity", changed.build()));
    let mut changed = base_fixture.clone();
    changed.session_seed = 0x39;
    substitutions.push(("Runtime session", changed.build()));
    let mut changed = base_fixture.clone();
    changed.recipient_seed = 0x3a;
    substitutions.push(("recipient key", changed.build()));
    for (label, changed_request) in substitutions {
        let substituted_request = WalletProviderRequestV2::new(
            &context,
            wallet_request.request_id.clone(),
            now_ts(),
            now_ts().saturating_add(120),
            operation_for(&account_id, &changed_request),
        )
        .unwrap_or_else(|err| panic!("{label} substitution must build: {err}"));
        expect_error(
            invoke_wallet_request(&mut provider, &substituted_request),
            "approval_identity_conflict",
        );
    }

    let authority_substitution =
        wallet_context_in_session(principal_id, "library", "session:substituted");
    let substituted_authority_request = WalletProviderRequestV2::new(
        &authority_substitution,
        wallet_request.request_id.clone(),
        now_ts(),
        now_ts().saturating_add(120),
        operation,
    )
    .unwrap();
    expect_error(
        invoke_wallet_request(&mut provider, &substituted_authority_request),
        "approval_identity_conflict",
    );

    let mut stored = provider.store.approval_requests[0]
        .signed_result
        .clone()
        .unwrap();
    stored["wallet_signed_rights_request_hex"] = Value::String("00".to_string());
    provider.store.approval_requests[0].signed_result = Some(stored);
    provider.save().unwrap();
    expect_error(
        invoke_wallet_request(&mut provider, &wallet_request),
        "signing_error",
    );
}

#[test]
fn protected_content_rights_external_rejects_wrong_signer_type_and_stored_shape() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:protected-content-external-negative";
    let signing_key = SigningKey::from_slice(&[0x44; 32]).unwrap();
    let wrong_key = SigningKey::from_slice(&[0x45; 32]).unwrap();
    let (account_id, address) =
        link_external_evm_account(&mut provider, principal_id, &signing_key);
    let request = protected_content_rights_request(
        wallet_address(&address),
        RightsActionV1::View,
        now_ts(),
        now_ts().saturating_add(120),
        0x81,
    );
    let (request_id, payload_hash) = request_protected_content_signature(
        &mut provider,
        principal_id,
        "library",
        &account_id,
        &request,
    );
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: request_id.clone(),
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { .. } => {}
        other => panic!("expected connector handoff, got {other:?}"),
    }

    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: request_id.clone(),
                payload_hash: payload_hash.clone(),
                signature: Some(sign_message_bytes(
                    &signing_key,
                    &request.canonical_bytes().unwrap(),
                )),
                signature_type: Some("eth_sign".to_string()),
                public_key: None,
                signer: address.clone(),
                transaction_hash: None,
            },
        ),
        "invalid_request",
    );

    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: request_id.clone(),
                payload_hash: payload_hash.clone(),
                signature: Some(make_high_s_evm_signature(&sign_message_bytes(
                    &signing_key,
                    &request.canonical_bytes().unwrap(),
                ))),
                signature_type: Some("personal_sign".to_string()),
                public_key: None,
                signer: address.clone(),
                transaction_hash: None,
            },
        ),
        "invalid_signature",
    );

    expect_error(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id,
                payload_hash,
                signature: Some(sign_message_bytes(
                    &wrong_key,
                    &request.canonical_bytes().unwrap(),
                )),
                signature_type: Some("personal_sign".to_string()),
                public_key: None,
                signer: address,
                transaction_hash: None,
            },
        ),
        "invalid_signature",
    );

    let mut extra = Map::new();
    extra.insert(
        "schema".to_string(),
        Value::String(PROTECTED_CONTENT_RIGHTS_SIGNATURE_RESULT_SCHEMA.to_string()),
    );
    extra.insert("account_id".to_string(), Value::String(account_id));
    extra.insert(
        "signer".to_string(),
        Value::String("0x3333333333333333333333333333333333333333".to_string()),
    );
    extra.insert(
        "wallet_signed_rights_request_hex".to_string(),
        Value::String("00".to_string()),
    );
    extra.insert(
        "provider_route".to_string(),
        Value::String("carrier://node".to_string()),
    );
    assert!(
        serde_json::from_value::<ProtectedContentRightsSignatureResultV1>(Value::Object(extra))
            .is_err()
    );
}
