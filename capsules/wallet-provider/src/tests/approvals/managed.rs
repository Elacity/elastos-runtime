use super::super::support::*;
use super::super::*;

fn managed_evm_account(provider: &mut WalletProvider, principal_id: &str) -> (String, String) {
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
        other => panic!("expected managed account, got {other:?}"),
    }
}

fn personal_sign_operation(
    account_id: &str,
    address: &str,
    message: &str,
) -> WalletProviderOperationV2 {
    WalletProviderOperationV2::RequestApproval {
        account_id: account_id.to_string(),
        chain_namespace: "eip155:20".into(),
        intent: "browser_personal_sign".into(),
        resource: "elastos://wallet/eip155:20/sign/browser_personal_sign".into(),
        reason: "Browser page requests personal_sign".into(),
        payload: browser_personal_sign_payload(account_id, address, message),
        expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
    }
}

fn typed_data_payload(account_id: &str, address: &str, chain_id: Value) -> Value {
    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "Message": [{ "name": "contents", "type": "string" }]
        },
        "primaryType": "Message",
        "domain": { "name": "ElastOS Browser", "chainId": chain_id },
        "message": { "contents": "Chain-bound approval" }
    });
    let canonical = serde_json::to_string(&typed_data).unwrap();
    json!({
        "schema": "elastos.browser.wallet-signature-request/v1",
        "method": "eth_signTypedData_v4",
        "params": [address, canonical.clone()],
        "typed_data": typed_data,
        "typed_data_canonical": canonical,
        "address": address,
        "account_id": account_id,
        "chain_namespace": "eip155:20",
        "page_url": "https://dapp.example/sign",
        "origin": "https://dapp.example",
        "requires_wallet_approval": true
    })
}

#[test]
fn managed_account_signs_only_after_runtime_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
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
        other => panic!("expected managed account, got {other:?}"),
    };
    let (request_id, payload_hash) = match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_personal_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/browser_personal_sign".into(),
            reason: "Browser page requests personal_sign".into(),
            payload: browser_personal_sign_payload(&account_id, &address, "Managed approval"),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let approval = &data["approval_request"];
            assert_eq!(approval["proof_type"], MANAGED_EVM_PROOF_TYPE);
            (
                approval["request_id"].as_str().unwrap().to_string(),
                approval["payload_hash"].as_str().unwrap().to_string(),
            )
        }
        other => panic!("expected approval request, got {other:?}"),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet-metamask",
        WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: request_id.clone(),
            reason: "Wrong approval class".to_string(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("connector handoff authority"));
        }
        other => panic!("expected managed/connector boundary rejection, got {other:?}"),
    }
    match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: request_id.clone(),
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let signature = data["signature"].as_str().unwrap();
            assert!(signature.starts_with("0x"));
            assert_eq!(signature.len(), 132);
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(data["signature_receipt"]["payload_hash"], payload_hash);
            assert_eq!(
                data["signed_payload"]["schema"],
                "elastos.browser.personal-sign-result/v1"
            );
            assert_eq!(data["signed_payload"]["request_id"], request_id);
        }
        other => panic!("expected managed signature, got {other:?}"),
    }
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListApprovals {
            include_resolved: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert!(data["approval_requests"].as_array().unwrap().is_empty());
        }
        other => panic!("expected empty pending list, got {other:?}"),
    }
}

#[test]
fn managed_account_signs_browser_typed_data_after_runtime_approval() {
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
    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "Message": [
                { "name": "contents", "type": "string" }
            ]
        },
        "primaryType": "Message",
        "domain": { "name": "ElastOS Browser", "chainId": 20 },
        "message": { "contents": "Connect wallet" }
    });
    let typed_data_canonical = serde_json::to_string(&typed_data).unwrap();
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_typed_data_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/browser_typed_data_sign".into(),
            reason: "Browser page requests eth_signTypedData_v4".into(),
            payload: json!({
                "schema": "elastos.browser.wallet-signature-request/v1",
                "method": "eth_signTypedData_v4",
                "params": [address.clone(), typed_data_canonical.clone()],
                "typed_data": typed_data,
                "typed_data_canonical": typed_data_canonical,
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
        Response::Ok { data: Some(data) } => {
            assert_eq!(
                data["approval_request"]["intent"],
                "browser_typed_data_sign"
            );
            data["approval_request"]["request_id"]
                .as_str()
                .unwrap()
                .to_string()
        }
        other => panic!("expected typed-data approval request, got {other:?}"),
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
            assert_eq!(
                data["approval_request"]["signed_result"]["schema"],
                "elastos.browser.typed-data-sign-result/v1"
            );
            let hash = eip712_payload_hash(&data["approval_request"]["payload"]).unwrap();
            let recovered = recover_evm_address_from_hash(&hash, signature).unwrap();
            assert_eq!(
                normalize_evm_address(&recovered),
                normalize_evm_address(&address)
            );
        }
        other => panic!("expected managed typed-data signature, got {other:?}"),
    }
}

#[test]
fn managed_btc_account_signs_bip322_after_runtime_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: Some("Bitcoin".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };
    let message = match invoke_wallet(
        &mut provider,
        "person:local:test",
        "wallet",
        WalletProviderOperationV2::BitcoinChallenge {
            domain: "elastos.local".into(),
            uri: "http://elastos.local/apps/home/".into(),
            address: address.clone(),
            network: PublicNetwork::bitcoin(),
            resources: vec!["elastos://wallet/account/link".into()],
        },
    ) {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let payload = bitcoin_bip322_payload(&address, &message);
    let (request_id, payload_hash) = match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            intent: "bitcoin_bip322_proof".into(),
            resource: "elastos://wallet/bitcoin/proof".into(),
            reason: "Prove Bitcoin account".into(),
            payload,
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let approval = &data["approval_request"];
            assert_eq!(approval["proof_type"], MANAGED_BTC_P2WPKH_PROOF_TYPE);
            assert_eq!(approval["intent"], "bitcoin_bip322_proof");
            (
                approval["request_id"].as_str().unwrap().to_string(),
                approval["payload_hash"].as_str().unwrap().to_string(),
            )
        }
        other => panic!("expected BTC approval request, got {other:?}"),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: request_id.clone(),
            reason: "approved".to_string(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let signature = data["signature"].as_str().unwrap();
            assert!(!signature.is_empty());
            assert!(data.get("signed_transaction").is_none());
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(data["signature_receipt"]["payload_hash"], payload_hash);
            assert_eq!(
                data["signed_payload"]["schema"],
                "elastos.wallet.bip322_signature_payload/v1"
            );
            assert_eq!(data["signed_payload"]["signature_type"], "bip322_simple");
            assert_eq!(data["signed_payload"]["request_id"], request_id);
            verify_bip322_simple("bitcoin", &address, &message, signature)
                .expect("managed BIP-322 signature should verify");
        }
        other => panic!("expected managed BTC signature, got {other:?}"),
    }
}

#[test]
fn managed_btc_account_rejects_unbound_bip322_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            intent: "bitcoin_bip322_proof".into(),
            resource: "elastos://wallet/bitcoin/proof".into(),
            reason: "Prove Bitcoin account".into(),
            payload: bitcoin_bip322_payload(&address, "Hello World"),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_bitcoin_bip322_proof");
            assert!(message.contains("Runtime account proof"));
        }
        other => panic!("expected unbound BTC proof rejection, got {other:?}"),
    }

    let fake_runtime_message = format!(
        "elastos.local wants you to prove Bitcoin account ownership:\n{address}\n\nURI: http://elastos.local/apps/home/\nVersion: 1\nNetwork: bitcoin\nNonce: fake\nIssued At: 1\nExpiration Time: 2\nResources:\n- elastos://auth/bitcoin-challenge/fake"
    );
    match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::RequestApproval {
            account_id: "wallet:bip122:000000000019d6689c085ae165831e93:missing".into(),
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            intent: "bitcoin_bip322_proof".into(),
            resource: "elastos://wallet/bitcoin/proof".into(),
            reason: "Prove Bitcoin account".into(),
            payload: bitcoin_bip322_payload(&address, &fake_runtime_message),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "not_found");
            assert!(message.contains("active linked account"));
        }
        other => panic!("expected missing-account rejection, got {other:?}"),
    }
    let account_id = match provider.store.accounts.first() {
        Some(account) => account.account_id.clone(),
        None => panic!("expected managed BTC account"),
    };
    match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            intent: "bitcoin_bip322_proof".into(),
            resource: "elastos://wallet/bitcoin/proof".into(),
            reason: "Prove Bitcoin account".into(),
            payload: bitcoin_bip322_payload(&address, &fake_runtime_message),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_bitcoin_bip322_proof");
            assert!(message.contains("challenge not found"));
        }
        other => panic!("expected fake challenge rejection, got {other:?}"),
    }
}

#[test]
fn managed_btc_account_rejects_expired_challenge_at_signing_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };
    let message = match invoke_wallet(
        &mut provider,
        "person:local:test",
        "wallet",
        WalletProviderOperationV2::BitcoinChallenge {
            domain: "elastos.local".into(),
            uri: "http://elastos.local/apps/home/".into(),
            address: address.clone(),
            network: PublicNetwork::bitcoin(),
            resources: vec!["elastos://wallet/account/link".into()],
        },
    ) {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            intent: "bitcoin_bip322_proof".into(),
            resource: "elastos://wallet/bitcoin/proof".into(),
            reason: "Prove Bitcoin account".into(),
            payload: bitcoin_bip322_payload(&address, &message),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected BTC approval request, got {other:?}"),
    };
    provider.store.bitcoin_challenges[0].challenge.expires_at = now_ts().saturating_sub(1);
    match invoke_wallet(
        &mut provider,
        principal_id,
        "system",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "approved".to_string(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_bitcoin_bip322_proof");
            assert!(message.contains("expired") || message.contains("not found"));
        }
        other => panic!("expected expired challenge rejection, got {other:?}"),
    }
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Pending
    );
}

#[test]
fn approval_managed_request_cannot_sign_after_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match invoke_wallet(
        &mut provider,
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
        other => panic!("expected managed account, got {other:?}"),
    };
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_personal_sign".into(),
            resource: "elastos://wallet/eip155:20/sign/browser_personal_sign".into(),
            reason: "Browser page requests personal_sign".into(),
            payload: browser_personal_sign_payload(&account_id, &address, "Expired approval"),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected approval request, got {other:?}"),
    };

    provider.store.approval_requests[0].expires_at = now_ts().saturating_sub(1);

    match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "approved".to_string(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("expired"));
        }
        other => panic!("expected expired approval rejection, got {other:?}"),
    }
}

#[test]
fn managed_accounts_reject_unimplemented_intents_before_recording_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:managed-intents";
    let (account_id, _) = managed_evm_account(&mut provider, principal_id);

    for intent in [
        "auth_challenge",
        "capability_grant",
        "credential",
        "publish_envelope",
        "browser_connect",
        "revocation",
    ] {
        match invoke_wallet(
            &mut provider,
            principal_id,
            "system",
            WalletProviderOperationV2::RequestApproval {
                account_id: account_id.clone(),
                chain_namespace: "eip155:20".into(),
                intent: intent.into(),
                resource: format!("elastos://wallet/eip155:20/sign/{intent}"),
                reason: "Unsupported managed signature".into(),
                payload: json!({ "intent": intent }),
                expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
            },
        ) {
            Response::Error { code, .. } => {
                assert_eq!(code, "unsupported_managed_signing_intent")
            }
            other => panic!("expected {intent} to fail closed, got {other:?}"),
        }
    }
    assert!(provider.store.approval_requests.is_empty());
}

#[test]
fn persisted_unsupported_managed_intent_cannot_reach_signing() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:persisted-intent";
    let (account_id, address) = managed_evm_account(&mut provider, principal_id);
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        personal_sign_operation(&account_id, &address, "Persisted intent"),
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected approval request, got {other:?}"),
    };
    provider.store.approval_requests[0].intent = "publish_envelope".to_string();

    match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "Approved".into(),
        },
    ) {
        Response::Error { code, .. } => {
            assert_eq!(code, "unsupported_managed_signing_intent")
        }
        other => panic!("expected persisted intent rejection, got {other:?}"),
    }
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Pending
    );
}

#[test]
fn active_approval_limit_is_principal_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let alice = "person:local:approval-limit-alice";
    let bob = "person:local:approval-limit-bob";
    let (alice_account_id, alice_address) = managed_evm_account(&mut provider, alice);
    let (bob_account_id, bob_address) = managed_evm_account(&mut provider, bob);
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            alice,
            "browser",
            personal_sign_operation(&alice_account_id, &alice_address, "Request 0"),
        ),
        Response::Ok { .. }
    ));
    let template = provider.store.approval_requests[0].clone();
    for index in 1..MAX_ACTIVE_APPROVAL_REQUESTS_PER_PRINCIPAL {
        let mut request = template.clone();
        request.request_id = format!("synthetic-active-request-{index}");
        request.created_at = request.created_at.saturating_add(index as u64);
        provider.store.approval_requests.push(request);
    }

    match invoke_wallet(
        &mut provider,
        alice,
        "browser",
        personal_sign_operation(&alice_account_id, &alice_address, "Over limit"),
    ) {
        Response::Error { code, .. } => assert_eq!(code, "approval_limit_reached"),
        other => panic!("expected active approval limit, got {other:?}"),
    }
    assert_eq!(
        provider
            .store
            .approval_requests
            .iter()
            .filter(|request| request.principal_id == alice)
            .count(),
        MAX_ACTIVE_APPROVAL_REQUESTS_PER_PRINCIPAL
    );
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            bob,
            "browser",
            personal_sign_operation(&bob_account_id, &bob_address, "Bob remains independent"),
        ),
        Response::Ok { .. }
    ));
}

#[test]
fn pruning_caps_resolved_history_without_evicting_live_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let alice = "person:local:pruning-alice";
    let bob = "person:local:pruning-bob";
    let (account_id, address) = managed_evm_account(&mut provider, alice);
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            alice,
            "browser",
            personal_sign_operation(&account_id, &address, "Template"),
        ),
        Response::Ok { .. }
    ));
    let template = provider.store.approval_requests[0].clone();
    let now = now_ts();
    let mut store = WalletStore::default();
    for (principal_id, count) in [
        (alice, MAX_RESOLVED_APPROVAL_HISTORY_PER_PRINCIPAL + 7),
        (bob, MAX_RESOLVED_APPROVAL_HISTORY_PER_PRINCIPAL),
    ] {
        for index in 0..count {
            let mut request = template.clone();
            request.principal_id = principal_id.to_string();
            request.request_id = format!("{principal_id}-resolved-{index}");
            request.status = ApprovalStatus::Rejected;
            request.created_at = index as u64;
            request.expires_at = now.saturating_sub(1);
            request.resolved_at = Some(now);
            store.approval_requests.push(request);
        }
    }
    for (principal_id, request_id, status) in [
        (alice, "alice-pending", ApprovalStatus::Pending),
        (bob, "bob-approved", ApprovalStatus::Approved),
    ] {
        let mut request = template.clone();
        request.principal_id = principal_id.to_string();
        request.request_id = request_id.to_string();
        request.status = status;
        request.expires_at = now.saturating_add(60);
        store.approval_requests.push(request);
    }
    let mut transaction = template;
    transaction.request_id = "alice-signed-unrecorded-transaction".to_string();
    transaction.status = ApprovalStatus::Completed;
    transaction.intent = "transaction_intent".to_string();
    transaction.expires_at = now.saturating_sub(1);
    transaction.signed_result = Some(json!({ "signed_transaction": "0x02f8" }));
    store.approval_requests.push(transaction);

    let pruned = prune_store(store, now);
    for principal_id in [alice, bob] {
        assert_eq!(
            pruned
                .approval_requests
                .iter()
                .filter(|request| {
                    request.principal_id == principal_id
                        && request.request_id.contains("-resolved-")
                })
                .count(),
            MAX_RESOLVED_APPROVAL_HISTORY_PER_PRINCIPAL
        );
    }
    for request_id in [
        "alice-pending",
        "bob-approved",
        "alice-signed-unrecorded-transaction",
    ] {
        assert!(pruned
            .approval_requests
            .iter()
            .any(|request| request.request_id == request_id));
    }
}

#[test]
fn eip712_domain_chain_is_rechecked_before_managed_signing() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:typed-chain";
    let (account_id, address) = managed_evm_account(&mut provider, principal_id);
    let operation = |payload| WalletProviderOperationV2::RequestApproval {
        account_id: account_id.clone(),
        chain_namespace: "eip155:20".into(),
        intent: "browser_typed_data_sign".into(),
        resource: "elastos://wallet/eip155:20/sign/browser_typed_data_sign".into(),
        reason: "Browser page requests typed data".into(),
        payload,
        expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        operation(typed_data_payload(&account_id, &address, json!(8453))),
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_browser_typed_data_sign");
            assert!(message.contains("domain.chainId"));
        }
        other => panic!("expected typed-data chain rejection, got {other:?}"),
    }
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        operation(typed_data_payload(&account_id, &address, json!("0x14"))),
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected chain-bound typed approval, got {other:?}"),
    };
    let substituted_payload = typed_data_payload(&account_id, &address, json!(8453));
    provider.store.approval_requests[0].payload_hash = value_hash(&substituted_payload);
    provider.store.approval_requests[0].payload = substituted_payload;

    match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "Approved".into(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_browser_typed_data_sign");
            assert!(message.contains("domain.chainId"));
        }
        other => panic!("expected signing-time chain rejection, got {other:?}"),
    }
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Pending
    );
}
