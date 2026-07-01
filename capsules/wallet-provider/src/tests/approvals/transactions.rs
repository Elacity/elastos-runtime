use super::super::support::*;
use super::super::*;

#[test]
fn managed_account_signs_eip155_transaction_intent_after_runtime_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let (account_id, address) = match provider.handle(Request::CreateManagedAccount {
        principal_id: principal_id.into(),
        chain_namespace: "eip155:20".into(),
        label: None,
        create_new: false,
    }) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed account, got {other:?}"),
    };
    let mut payload = transaction_intent_payload(&address);
    payload["page_url"] = json!("https://ela.city/cinema/view/test");
    payload["origin"] = json!("https://ela.city");
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id),
        chain_namespace: Some("eip155:20".into()),
        intent: "transaction_intent".into(),
        capsule_id: "system".into(),
        resource: "elastos://chain/esc-mainnet/broadcast_transaction".into(),
        reason: "Send EVM transaction".into(),
        payload,
        expires_at: None,
    }) {
        Response::Ok { data: Some(data) } => {
            let approval = &data["approval_request"];
            assert_eq!(approval["intent"], "transaction_intent");
            assert_eq!(approval["status"], "pending");
            (
                approval["request_id"].as_str().unwrap().to_string(),
                approval["payload_hash"].as_str().unwrap().to_string(),
            )
        }
        other => panic!("expected transaction approval request, got {other:?}"),
    };
    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: Some("Approved transaction".into()),
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::SignApproved {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
    }) {
        Response::Ok { data: Some(data) } => {
            let signed_transaction = data["signed_transaction"].as_str().unwrap();
            assert!(signed_transaction.starts_with("0x"));
            assert!(signed_transaction.len() > 64);
            assert!(data.get("signature").is_none());
            assert_eq!(
                data["signed_payload"]["schema"],
                "elastos.wallet.signed_transaction/v1"
            );
            assert_eq!(data["signed_payload"]["transaction_type"], "eip155_legacy");
            assert_eq!(data["signed_payload"]["payload_hash"], payload_hash);
            assert!(data["signed_payload"]["transaction_hash"]
                .as_str()
                .unwrap()
                .starts_with("0x"));
        }
        other => panic!("expected signed transaction, got {other:?}"),
    }
    let broadcast_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    match provider.handle(Request::RecordTransactionHash {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        transaction_hash: broadcast_hash.into(),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["approval_request"]["signed_result"]["transaction_hash"],
                broadcast_hash
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["page_url"],
                "https://ela.city/cinema/view/test"
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["origin"],
                "https://ela.city"
            );
        }
        other => panic!("expected recorded transaction hash, got {other:?}"),
    }
    match provider.handle(Request::ApprovalRequests {
        principal_id: principal_id.into(),
        include_resolved: true,
    }) {
        Response::Ok { data: Some(data) } => {
            let requests = data["approval_requests"].as_array().unwrap();
            assert!(requests.iter().any(|request| {
                request["request_id"] == request_id
                    && request["status"] == "completed"
                    && request["signed_result"]["transaction_hash"] == broadcast_hash
            }));
        }
        other => panic!("expected approval history with transaction hash, got {other:?}"),
    }
}

#[test]
fn transaction_intent_validates_payload_and_allows_external_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let managed = match provider.handle(Request::CreateManagedAccount {
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

    match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(managed),
        chain_namespace: Some("eip155:20".into()),
        intent: "transaction_intent".into(),
        capsule_id: "system".into(),
        resource: "elastos://chain/esc-mainnet/broadcast_transaction".into(),
        reason: "Send EVM transaction".into(),
        payload: json!({
            "schema": "elastos.chain.unsigned_transaction_intent/v1",
            "transaction_type": "eip155_legacy",
            "chain_id": 20
        }),
        expires_at: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_transaction_intent");
            assert!(message.contains("wallet_intent") || message.contains("missing"));
        }
        other => panic!("expected invalid transaction payload, got {other:?}"),
    }

    let external_address = "0x3333333333333333333333333333333333333333";
    let external_account_id = format!("wallet:eip155:20:{external_address}");
    assert!(matches!(
        provider.handle(Request::LinkAccount {
            principal_id: principal_id.into(),
            proof_binding_id: format!("proof:eip155:20:{external_address}"),
            chain_namespace: "eip155:20".into(),
            address: external_address.into(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        Response::Ok { .. }
    ));
    let mut external_payload = transaction_intent_payload(external_address);
    external_payload["page_url"] = json!("https://ela.city/cinema/view/test");
    external_payload["origin"] = json!("https://ela.city");
    let (request_id, payload_hash) = match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(external_account_id.clone()),
        chain_namespace: Some("eip155:20".into()),
        intent: "transaction_intent".into(),
        capsule_id: "system".into(),
        resource: "elastos://chain/esc-mainnet/broadcast_transaction".into(),
        reason: "Send EVM transaction".into(),
        payload: external_payload,
        expires_at: None,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["intent"], "transaction_intent");
            assert_eq!(data["approval_request"]["connector_id"], "wallet-metamask");
            assert_eq!(
                data["approval_request"]["payload"]["schema"],
                "elastos.chain.unsigned_transaction_intent/v1"
            );
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
        other => panic!("expected external transaction approval request, got {other:?}"),
    };
    let transaction = match provider.handle(Request::ApproveApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        reason: Some("Approved transaction".into()),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["handoff"]["intent"], "transaction_intent");
            data["handoff"]["transaction"].clone()
        }
        other => panic!("expected external transaction handoff, got {other:?}"),
    };
    assert_eq!(transaction["from"], external_address);
    assert_eq!(transaction["chainId"], "0x14");
    let transaction_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        connector_id: "wallet-metamask".into(),
        payload_hash: payload_hash.clone(),
        signature: Some("0xsigned-transaction-should-not-be-here".into()),
        signature_type: None,
        public_key: None,
        signer: external_address.into(),
        transaction_hash: Some(transaction_hash.into()),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("must not include signature"));
        }
        other => panic!("expected transaction signature rejection, got {other:?}"),
    }
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet-metamask".into(),
        payload_hash,
        signature: None,
        signature_type: None,
        public_key: None,
        signer: external_address.into(),
        transaction_hash: Some(transaction_hash.into()),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["status"], "completed");
            assert_eq!(
                data["approval_request"]["signed_result"]["schema"],
                "elastos.wallet.external-transaction-result/v1"
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["transaction_hash"],
                transaction_hash
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["page_url"],
                "https://ela.city/cinema/view/test"
            );
            assert_eq!(
                data["approval_request"]["signed_result"]["origin"],
                "https://ela.city"
            );
        }
        other => panic!("expected external transaction completion, got {other:?}"),
    };
}
