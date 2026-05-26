use super::super::support::*;
use super::super::*;

#[test]
fn signature_request_records_pending_approval_without_signing() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = "wallet:eip155:20:0xabc";

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: "proof:eip155:20:0xabc".into(),
            chain_namespace: "eip155:20".into(),
            address: "0xabc".into(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));

    let response = provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id.into()),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-test", "revision": 1}),
        expires_at: None,
    });

    let request_id = match response {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["requires_approval"], true);
            assert!(data["signature"].is_null());
            let approval = &data["approval_request"];
            assert_eq!(approval["status"], "pending");
            assert_eq!(approval["intent"], "publish_envelope");
            assert_eq!(approval["account_id"], account_id);
            assert!(approval["payload_hash"].as_str().unwrap().starts_with("0x"));
            approval["request_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected approval request, got {other:?}"),
    };

    let mut provider = init_provider(dir.path());
    match provider.handle(Request::ApprovalRequests {
        principal_id: principal_id.into(),
        include_resolved: false,
    }) {
        Response::Ok { data: Some(data) } => {
            let requests = data["approval_requests"].as_array().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["request_id"], request_id);
        }
        other => panic!("expected approval list, got {other:?}"),
    }
}

#[test]
fn approval_request_can_be_rejected_and_hidden_from_pending_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = "wallet:eip155:20:0xabc";

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: "proof:eip155:20:0xabc".into(),
            chain_namespace: "eip155:20".into(),
            address: "0xabc".into(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));
    let request_id = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id.into()),
        chain_namespace: Some("eip155:20".into()),
        intent: "credential".into(),
        capsule_id: "system".into(),
        resource: "elastos://wallet/eip155:20/sign/credential".into(),
        reason: "Issue credential".into(),
        payload: json!({"credential": "test"}),
        expires_at: None,
    }) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected approval request, got {other:?}"),
    };

    match provider.handle(Request::RejectApproval {
        principal_id: principal_id.into(),
        request_id,
        reason: Some("Not now".into()),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "rejected");
            assert_eq!(data["approval_request"]["rejection_reason"], "Not now");
        }
        other => panic!("expected rejection, got {other:?}"),
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
fn approval_request_can_be_approved_and_completed_without_exposing_raw_signature() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[8u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:20:{address}");

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: format!("proof:eip155:20:{address}"),
            chain_namespace: "eip155:20".into(),
            address: address.clone(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-test"}),
        expires_at: None,
    }) {
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

    let handoff_message = match provider.handle(Request::ApproveApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        reason: Some("Looks correct".into()),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "approved");
            assert_eq!(data["approval_request"]["approval_reason"], "Looks correct");
            assert_eq!(data["handoff"]["status"], "awaiting_wallet_signature");
            assert!(data["signature"].is_null());
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected approval handoff, got {other:?}"),
    };

    let signature = sign_message(&signing_key, &handoff_message);
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet-metamask".into(),
        payload_hash,
        signature: Some(signature),
        signature_type: None,
        public_key: None,
        signer: address.clone(),
        transaction_hash: None,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["signature_receipt"]["schema"],
                "elastos.wallet.signature_receipt/v1"
            );
            assert_eq!(data["signature_receipt"]["signer"], address);
            assert!(data["signature_receipt"]["signature_hash"]
                .as_str()
                .unwrap()
                .starts_with("0x"));
            assert!(data.get("signature").is_none());
        }
        other => panic!("expected completion receipt, got {other:?}"),
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
fn approval_external_bitcoin_request_completes_with_bip322_connector_signature() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = bip322_test_signing_key();
    let address = bip322_test_address(&signing_key);
    let account_id = format!("wallet:{BITCOIN_MAINNET_CHAIN_NAMESPACE}:{address}");

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

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: format!("proof:wallet:{BITCOIN_MAINNET_CHAIN_NAMESPACE}:{address}"),
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            address: address.clone(),
            proof_type: "bip322_simple".into(),
            connector_id: Some("wallet".into()),
            label: Some("Bitcoin".into()),
        }),
        Response::Ok { .. }
    ));
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
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
        Response::Ok { data: Some(data) } => {
            let approval = &data["approval_request"];
            assert_eq!(approval["connector_id"], "wallet");
            assert_eq!(approval["proof_type"], "bip322_simple");
            (
                approval["request_id"].as_str().unwrap().to_string(),
                approval["payload_hash"].as_str().unwrap().to_string(),
            )
        }
        other => panic!("expected BTC approval request, got {other:?}"),
    };

    let handoff_message = match provider.handle(Request::ApproveApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        reason: Some("Approved Bitcoin proof".into()),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["handoff"]["status"], "awaiting_wallet_signature");
            assert_eq!(data["handoff"]["signer"], address);
            assert_eq!(data["handoff"]["payload_hash"], payload_hash);
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected Bitcoin handoff, got {other:?}"),
    };
    assert_eq!(handoff_message, message);

    let signature = sign_bip322_simple_p2wpkh(&signing_key, &address, &handoff_message);
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet".into(),
        payload_hash,
        signature: Some(signature),
        signature_type: None,
        public_key: None,
        signer: address.clone(),
        transaction_hash: None,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(data["signature_receipt"]["signer"], address);
            assert!(data["signature_receipt"]["signature_hash"]
                .as_str()
                .unwrap()
                .starts_with("0x"));
            assert!(data.get("signature").is_none());
        }
        other => panic!("expected Bitcoin completion receipt, got {other:?}"),
    }
}

#[test]
fn approval_completion_rejects_signature_from_wrong_evm_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[9u8; 32]).into()).unwrap();
    let wrong_key = SigningKey::from_bytes((&[10u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:20:{address}");

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: format!("proof:eip155:20:{address}"),
            chain_namespace: "eip155:20".into(),
            address: address.clone(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-test"}),
        expires_at: None,
    }) {
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
    let handoff_message = match provider.handle(Request::ApproveApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        reason: None,
    }) {
        Response::Ok { data: Some(data) } => {
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected approval handoff, got {other:?}"),
    };
    let wrong_signature = sign_message(&wrong_key, &handoff_message);

    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet-metamask".into(),
        payload_hash,
        signature: Some(wrong_signature),
        signature_type: None,
        public_key: None,
        signer: address,
        transaction_hash: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_signature");
            assert!(message.contains("signer mismatch"));
        }
        other => panic!("expected signer mismatch rejection, got {other:?}"),
    }
}

#[test]
fn approval_external_request_cannot_complete_after_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let signing_key = SigningKey::from_bytes((&[11u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);
    let account_id = format!("wallet:eip155:20:{address}");

    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: format!("proof:eip155:20:{address}"),
            chain_namespace: "eip155:20".into(),
            address: address.clone(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-expired-external"}),
        expires_at: None,
    }) {
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
    let handoff_message = match provider.handle(Request::ApproveApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        reason: None,
    }) {
        Response::Ok { data: Some(data) } => {
            data["handoff"]["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected approval handoff, got {other:?}"),
    };
    provider.store.approval_requests[0].expires_at = now_ts().saturating_sub(1);
    let signature = sign_message(&signing_key, &handoff_message);

    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet-metamask".into(),
        payload_hash,
        signature: Some(signature),
        signature_type: None,
        public_key: None,
        signer: address,
        transaction_hash: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("expired"));
        }
        other => panic!("expected expired approval rejection, got {other:?}"),
    }
}
