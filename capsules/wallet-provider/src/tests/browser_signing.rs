use super::support::*;
use super::*;

#[test]
fn wallet_provider_rejects_hidden_signature_request_fields() {
    let request = json!({
        "op": "request_signature",
        "principal_id": "person:local:abc123",
        "chain_namespace": "eip155:20",
        "intent": "transaction_intent",
        "capsule_id": "documents",
        "resource": "elastos://wallet/eip155:20/sign/transaction_intent",
        "reason": "Sign typed transaction",
        "payload": {
            "schema": "elastos.chain.unsigned_transaction_intent/v1"
        },
        "private_key": "secret"
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("wallet signing requests must reject hidden authority fields")
        .to_string();
    assert!(err.contains("unknown variant"), "unexpected error: {err}");
}

#[test]
fn browser_personal_sign_is_a_typed_approval_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[9u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:1:{}", normalize_evm_address(&address));
    let message = "Sign into Glide";

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: format!("proof:eip155:1:{}", normalize_evm_address(&address)),
                chain_namespace: "eip155:1".into(),
                address: address.clone(),
                proof_type: "siwe".into(),
                label: None,
            },
        ),
        Response::Ok { .. }
    ));

    let (request_id, payload_hash) = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:1".into(),
            intent: "browser_personal_sign".into(),
            resource: "elastos://wallet/eip155:1/sign/browser_personal_sign".into(),
            reason: "Browser page requests personal_sign".into(),
            payload: json!({
                "schema": "elastos.browser.wallet-signature-request/v1",
                "method": "personal_sign",
                "params": [message, address.clone()],
                "message": message,
                "address": address.clone(),
                "account_id": account_id.clone(),
                "chain_namespace": "eip155:1",
                "page_url": "https://glidefinance.io/",
                "origin": "https://glidefinance.io",
                "requires_wallet_approval": true
            }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["intent"], "browser_personal_sign");
            assert_eq!(
                data["approval_request"]["payload"]["schema"],
                "elastos.browser.wallet-signature-request/v1"
            );
            assert_eq!(data["approval_request"]["status"], "pending");
            (
                data["approval_request"]["request_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                data["approval_request"]["payload_hash"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        }
        other => panic!("expected browser approval request, got {other:?}"),
    };

    let handoff_message = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: request_id.clone(),
            reason: "Looks correct".into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "approved");
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected browser handoff, got {other:?}"),
    };
    assert_eq!(handoff_message, message);

    let signature = sign_message(&signing_key, message);
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id,
            payload_hash,
            signature: Some(signature.clone()),
            signature_type: Some("personal_sign".into()),
            public_key: None,
            signer: address.clone(),
            transaction_hash: None,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["approval_request"]["signed_result"]["schema"],
                "elastos.browser.personal-sign-result/v1"
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["signature"],
                signature
            );
        }
        other => panic!("expected browser signature completion, got {other:?}"),
    };
}

/// Regression for the ESP connector delegation-signing feature: a `ddrm_delegation_sign` approval
/// (minted by `viewer_grant_sign::create_delegation_sign_request` in elastos-server) uses a
/// DIFFERENT payload schema than `browser_personal_sign`
/// (`elastos.viewer.delegation-sign-request/v1`, no `address`/`account_id`/`chain_namespace`/
/// `page_url`/`origin` fields) but must still be EIP-191 `personal_sign`-verified against its exact
/// `payload.message` and must end up with a readable `signed_result.signature`, mirroring
/// `browser_personal_sign_is_a_typed_approval_request` above.
#[test]
fn ddrm_delegation_sign_round_trip_completes_with_a_readable_signature() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[11u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:20:{}", normalize_evm_address(&address));
    let delegation_canonical = "delegation-canonical-text-for-content-kid";

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: format!("proof:eip155:20:{}", normalize_evm_address(&address)),
                chain_namespace: "eip155:20".into(),
                address: address.clone(),
                proof_type: "siwe".into(),
                label: None,
            },
        ),
        Response::Ok { .. }
    ));

    let (request_id, payload_hash) = match invoke_wallet(
        &mut provider,
        principal_id,
        "home",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "ddrm_delegation_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/ddrm_delegation_sign".into(),
            reason: "Confirm the dDRM delegation signature for content 0xkid".into(),
            payload: json!({
                "schema": "elastos.viewer.delegation-sign-request/v1",
                "method": "personal_sign",
                "message": delegation_canonical,
                "owner_address": address.clone(),
                "kid": "0xkid",
                "uri": "elastos://owned/asset-1",
                "connector_id": "wallet-metamask",
                "principal_id": principal_id,
                "session_id": "session-1",
                "requires_wallet_approval": true
            }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["intent"], "ddrm_delegation_sign");
            assert_eq!(data["approval_request"]["status"], "pending");
            (
                data["approval_request"]["request_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                data["approval_request"]["payload_hash"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        }
        other => panic!("expected ddrm delegation-sign approval request, got {other:?}"),
    };

    let handoff_message = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: request_id.clone(),
            reason: "Looks correct".into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "approved");
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected ddrm delegation-sign handoff, got {other:?}"),
    };
    // The connector must be handed the EXACT delegation_canonical string to sign — not the generic
    // `ElastOS Wallet Approval...` fallback message other unrecognized intents get.
    assert_eq!(handoff_message, delegation_canonical);

    let signature = sign_message(&signing_key, delegation_canonical);
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id,
            payload_hash,
            signature: Some(signature.clone()),
            signature_type: Some("personal_sign".into()),
            public_key: None,
            signer: address.clone(),
            transaction_hash: None,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["approval_request"]["signed_result"]["signature"],
                signature
            );
        }
        other => panic!("expected ddrm delegation-sign signature completion, got {other:?}"),
    };
}

#[test]
fn managed_browser_personal_sign_decodes_hex_messages_like_injected_wallets() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Spending".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed account, got {other:?}"),
    };
    let message = "Approve signature on https://ela.city with nonce 0";
    let hex_message = format!("0x{}", hex::encode(message.as_bytes()));
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_personal_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/browser_personal_sign".into(),
            reason: "Browser page requests personal_sign".into(),
            payload: json!({
                "schema": "elastos.browser.wallet-signature-request/v1",
                "method": "personal_sign",
                "params": [hex_message, address.clone()],
                "message": hex_message,
                "address": address.clone(),
                "account_id": account_id,
                "chain_namespace": "eip155:20",
                "page_url": "https://ela.city/home",
                "origin": "https://ela.city",
                "requires_wallet_approval": true
            }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected browser approval request, got {other:?}"),
    };
    match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let signature = data["signature"].as_str().unwrap();
            let decoded_hash = ethereum_signed_message_hash(message.as_bytes());
            let recovered = recover_evm_address_from_hash(&decoded_hash, signature).unwrap();
            assert_eq!(
                normalize_evm_address(&recovered),
                normalize_evm_address(&address)
            );
        }
        other => panic!("expected managed Browser personal_sign signature, got {other:?}"),
    }
}

#[test]
fn external_browser_personal_sign_completion_accepts_hex_decoded_signature() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[8u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:20:{}", normalize_evm_address(&address));
    let message = "Approve signature on https://ela.city with nonce 0";
    let hex_message = format!("0x{}", hex::encode(message.as_bytes()));

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: format!("proof:eip155:20:{}", normalize_evm_address(&address)),
                chain_namespace: "eip155:20".into(),
                address: address.clone(),
                proof_type: "siwe".into(),
                label: None,
            },
        ),
        Response::Ok { .. }
    ));

    let (request_id, payload_hash) = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_personal_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/browser_personal_sign".into(),
            reason: "Browser page requests personal_sign".into(),
            payload: json!({
                "schema": "elastos.browser.wallet-signature-request/v1",
                "method": "personal_sign",
                "params": [hex_message, address.clone()],
                "message": hex_message,
                "address": address.clone(),
                "account_id": account_id,
                "chain_namespace": "eip155:20",
                "page_url": "https://ela.city/home",
                "origin": "https://ela.city",
                "requires_wallet_approval": true
            }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
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
        other => panic!("expected browser approval request, got {other:?}"),
    };
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::ApproveConnectorHandoff {
                request_id: request_id.clone(),
                reason: "Approved by runtime session".into(),
            },
        ),
        Response::Ok { .. }
    ));
    let signature = sign_message_bytes(&signing_key, message.as_bytes());
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id,
            payload_hash,
            signature: Some(signature.clone()),
            signature_type: Some("personal_sign".into()),
            public_key: None,
            signer: address,
            transaction_hash: None,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["approval_request"]["signed_result"]["signature"],
                signature
            );
        }
        other => panic!("expected external Browser personal_sign completion, got {other:?}"),
    };
}

#[test]
fn wallet_provider_rejects_hidden_connector_completion_fields() {
    let request = json!({
        "op": "complete_approval",
        "principal_id": "person:local:abc123",
        "request_id": "wallet-approval:abc123",
        "connector_id": "wallet-metamask",
        "payload_hash": "0xabab",
        "signature": "0xsigned",
        "signer": "0x0000000000000000000000000000000000000001",
        "wallet_object": {}
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("retired connector completion must not reach the production decoder")
        .to_string();
    assert!(err.contains("unknown variant"), "unexpected error: {err}");
}

#[test]
fn init_and_status_do_not_expose_internal_storage_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = WalletProvider::new();
    let init = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "init",
            "config": { "base_path": dir.path().display().to_string() }
        }),
    );
    match init {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["protocol_version"], WALLET_PROTOCOL_VERSION);
            assert_eq!(data["storage_configured"], true);
            assert_eq!(data["managed_wallets_configured"], true);
            assert!(data.get("storage").is_none());
            assert!(data.get("managed_wallet_storage").is_none());
        }
        other => panic!("expected init ok, got {other:?}"),
    }

    match decode_and_handle_outer(&mut provider, json!({ "op": "status" })) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["protocol_version"], WALLET_PROTOCOL_VERSION);
            assert_eq!(data["supported_operations"], json!(["status"]));
            assert_eq!(data["storage_configured"], true);
            assert_eq!(data["managed_wallets_configured"], true);
            assert!(data.get("storage").is_none());
            assert!(data.get("managed_wallet_storage").is_none());
        }
        other => panic!("expected status ok, got {other:?}"),
    }
}
