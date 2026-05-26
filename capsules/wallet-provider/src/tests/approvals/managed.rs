use super::super::support::*;
use super::super::*;

#[test]
fn managed_account_signs_only_after_runtime_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: "eip155:20".into(),
        label: None,
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-managed"}),
        expires_at: None,
    }) {
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

    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("approved before managed signing"));
        }
        other => panic!("expected pre-approval signing rejection, got {other:?}"),
    }
    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved by runtime session".into()),
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
    }) {
        Response::Ok { data: Some(data) } => {
            let signature = data["signature"].as_str().unwrap();
            assert!(signature.starts_with("0x"));
            assert_eq!(signature.len(), 132);
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(data["signature_receipt"]["payload_hash"], payload_hash);
            assert_eq!(
                data["signed_payload"]["schema"],
                "elastos.wallet.managed_signature_payload/v1"
            );
            assert_eq!(data["signed_payload"]["request_id"], request_id);
        }
        other => panic!("expected managed signature, got {other:?}"),
    }
    match provider.handle(Request::ApprovalRequests {
        principal_id: principal_id.into(),
        include_resolved: false,
    }) {
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
    let (account_id, address) = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: "eip155:20".into(),
        label: Some("Spending".into()),
        create_new: false,
    }) {
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
    let request_id = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id.clone()),
        chain_namespace: Some("eip155:20".into()),
        intent: "browser_typed_data_sign".into(),
        capsule_id: "browser".into(),
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
        expires_at: None,
    }) {
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
    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved by runtime session".into()),
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id,
    }) {
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
    let (account_id, address) = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
        label: Some("Bitcoin".into()),
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };
    let message = match provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        network: "bitcoin".into(),
        resources: vec!["elastos://wallet/account/link".into()],
    }) {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let payload = bitcoin_bip322_payload(&address, &message);
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "bitcoin_bip322_proof".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/bitcoin/proof".into(),
        reason: "Prove Bitcoin account".into(),
        payload,
        expires_at: None,
    }) {
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

    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("approved before managed signing"));
        }
        other => panic!("expected pre-approval signing rejection, got {other:?}"),
    }
    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved Bitcoin proof".into()),
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
    }) {
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
    let (account_id, address) = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
        label: None,
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };

    match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "bitcoin_bip322_proof".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/bitcoin/proof".into(),
        reason: "Prove Bitcoin account".into(),
        payload: bitcoin_bip322_payload(&address, "Hello World"),
        expires_at: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_bitcoin_bip322_proof");
            assert!(message.contains("Runtime account proof"));
        }
        other => panic!("expected unbound BTC proof rejection, got {other:?}"),
    }

    let fake_runtime_message = format!(
        "elastos.local wants you to prove Bitcoin account ownership:\n{address}\n\nURI: http://elastos.local/apps/home/\nVersion: 1\nNetwork: bitcoin\nNonce: fake\nIssued At: 1\nExpiration Time: 2\nResources:\n- elastos://auth/bitcoin-challenge/fake"
    );
    match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some("wallet:bip122:000000000019d6689c085ae165831e93:missing".into()),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "bitcoin_bip322_proof".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/bitcoin/proof".into(),
        reason: "Prove Bitcoin account".into(),
        payload: bitcoin_bip322_payload(&address, &fake_runtime_message),
        expires_at: None,
    }) {
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
    match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "bitcoin_bip322_proof".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/bitcoin/proof".into(),
        reason: "Prove Bitcoin account".into(),
        payload: bitcoin_bip322_payload(&address, &fake_runtime_message),
        expires_at: None,
    }) {
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
    let (account_id, address) = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
        label: None,
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed BTC account, got {other:?}"),
    };
    let message = match provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        network: "bitcoin".into(),
        resources: vec!["elastos://wallet/account/link".into()],
    }) {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let request_id = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "bitcoin_bip322_proof".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/bitcoin/proof".into(),
        reason: "Prove Bitcoin account".into(),
        payload: bitcoin_bip322_payload(&address, &message),
        expires_at: None,
    }) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected BTC approval request, got {other:?}"),
    };
    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved Bitcoin proof".into()),
        }),
        Response::Ok { .. }
    ));
    provider.store.bitcoin_challenges[0].challenge.expires_at = now_ts().saturating_sub(1);
    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_bitcoin_bip322_proof");
            assert!(message.contains("expired") || message.contains("not found"));
        }
        other => panic!("expected expired challenge rejection, got {other:?}"),
    }
}

#[test]
fn approval_managed_request_cannot_sign_after_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: "eip155:20".into(),
        label: None,
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    let request_id = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-expired-managed"}),
        expires_at: None,
    }) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected approval request, got {other:?}"),
    };

    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved by runtime session".into()),
        }),
        Response::Ok { .. }
    ));
    provider.store.approval_requests[0].expires_at = now_ts().saturating_sub(1);

    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("expired"));
        }
        other => panic!("expected expired approval rejection, got {other:?}"),
    }
}
