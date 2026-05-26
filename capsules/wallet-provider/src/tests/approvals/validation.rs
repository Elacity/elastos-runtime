use super::super::support::*;
use super::super::*;

#[test]
fn approval_completion_fails_closed_before_approval_or_with_wrong_hash() {
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

    let wrong_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id: request_id.clone(),
        connector_id: "wallet-metamask".into(),
        payload_hash: wrong_hash.into(),
        signature: Some("0xsigned-wallet-result".into()),
        signature_type: None,
        public_key: None,
        signer: "0xabc".into(),
        transaction_hash: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("approved before completion"));
        }
        other => panic!("expected fail-closed completion, got {other:?}"),
    }

    assert!(matches!(
        provider.handle(Request::ApproveApproval {
            principal_id: principal_id.into(),
            request_id: request_id.clone(),
            reason: None,
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::CompleteApproval {
        principal_id: principal_id.into(),
        request_id,
        connector_id: "wallet-metamask".into(),
        payload_hash: wrong_hash.into(),
        signature: Some("0xsigned-wallet-result".into()),
        signature_type: None,
        public_key: None,
        signer: "0xabc".into(),
        transaction_hash: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("payload hash mismatch"));
        }
        other => panic!("expected hash rejection, got {other:?}"),
    }
}

#[test]
fn signature_request_rejects_unknown_intent_and_unlinked_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());

    for (intent, account_id) in [
        ("raw_sign", "wallet:eip155:20:0xabc"),
        ("publish_envelope", "wallet:eip155:20:missing"),
    ] {
        match provider.handle(Request::Signature {
            principal_id: "person:local:alice".into(),
            account_id: Some(account_id.into()),
            chain_namespace: Some("eip155:20".into()),
            intent: intent.into(),
            capsule_id: "documents".into(),
            resource: "elastos://content/publish".into(),
            reason: "Publish document revision".into(),
            payload: json!({"cid": "bafy-test"}),
            expires_at: None,
        }) {
            Response::Error { code, .. } => {
                assert!(code == "invalid_request" || code == "not_found")
            }
            other => panic!("expected request rejection, got {other:?}"),
        }
    }
}

#[test]
fn signature_request_rejects_explicit_account_on_incompatible_chain() {
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

    match provider.handle(Request::Signature {
        principal_id: principal_id.into(),
        account_id: Some(account_id.into()),
        chain_namespace: Some(BITCOIN_MAINNET_CHAIN_NAMESPACE.into()),
        intent: "publish_envelope".into(),
        capsule_id: "documents".into(),
        resource: "elastos://content/publish".into(),
        reason: "Publish document revision".into(),
        payload: json!({"cid": "bafy-test"}),
        expires_at: None,
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("chain_namespace"));
        }
        other => panic!("expected request rejection, got {other:?}"),
    }
}

#[test]
fn rejects_path_like_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());

    match provider.handle(Request::LinkAccount {
        principal_id: "../alice".into(),
        proof_binding_id: "proof:eip155:20:0xabc".into(),
        chain_namespace: "eip155:20".into(),
        address: "0xabc".into(),
        proof_type: "siwe".into(),
        connector_id: Some("wallet-metamask".into()),
        label: None,
    }) {
        Response::Error { code, .. } => assert_eq!(code, "invalid_request"),
        other => panic!("expected invalid request, got {other:?}"),
    }
}

#[test]
fn account_operations_fail_before_init() {
    let mut provider = WalletProvider::new();

    for response in [
        provider.accounts("person:local:alice", false),
        provider.handle(Request::LinkAccount {
            principal_id: "person:local:alice".into(),
            proof_binding_id: "proof:eip155:20:0xabc".into(),
            chain_namespace: "eip155:20".into(),
            address: "0xabc".into(),
            proof_type: "siwe".into(),
            connector_id: Some("wallet-metamask".into()),
            label: None,
        }),
        provider.handle(Request::RevokeAccount {
            principal_id: "person:local:alice".into(),
            account_id: "wallet:eip155:20:0xabc".into(),
        }),
    ] {
        match response {
            Response::Error { code, .. } => assert_eq!(code, "not_initialized"),
            other => panic!("expected not initialized, got {other:?}"),
        }
    }

    assert!(provider.store.accounts.is_empty());
}
