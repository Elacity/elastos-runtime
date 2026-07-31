use super::super::*;

#[tokio::test]
async fn test_system_can_select_default_wallet_without_exposing_connector_authority() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let account_id = "wallet:eip155:8453:0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C";
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": context.principal_id,
            "proof_binding_id": "proof:wallet:siwe:eip155:8453:0xa4c02db8653dd0ca18a8736d693b0db85c5b246c",
            "chain_namespace": "eip155:8453",
            "address": "0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C",
            "proof_type": "siwe",
            "connector_id": "wallet-metamask",
            "linked_at": 20
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/default")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"eip155:8453","intent":"transaction_intent"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["linked_count"], 1);
    assert_eq!(json["default_accounts"][0]["account_id"], account_id);
    assert_eq!(json["default_accounts"][0]["intent"], "transaction_intent");
}

#[tokio::test]
async fn test_system_approves_managed_wallet_request_and_executes_signature() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = authority.system_token.clone();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:managed",
            "status": "pending",
            "intent": "publish_envelope",
            "capsule_id": "documents",
            "resource": "elastos://content/publish",
            "reason": "Publish document revision",
            "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            "address": "0x1111111111111111111111111111111111111111",
            "proof_type": "managed_evm",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id.clone(),
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let missing_fresh_token = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Amanaged/approve")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Looks correct"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_fresh_token.status(), StatusCode::FORBIDDEN);
    let missing_body = axum::body::to_bytes(missing_fresh_token.into_body(), usize::MAX)
        .await
        .unwrap();
    let missing_text = String::from_utf8(missing_body.to_vec()).unwrap();
    assert!(missing_text.contains("fresh passkey verification is required"));

    let approval_token = intent_token_for_authority_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority,
        "wallet.approve",
        &json!({
            "request_id": "wallet-approval:managed",
            "reason": "Looks correct",
        }),
    );
    let approved = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Amanaged/approve")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"reason":"Looks correct","home_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pending_count"], 0);
    assert_eq!(json["note"], "Approved and signed by built-in wallet.");
}

#[tokio::test]
async fn test_system_does_not_approve_external_wallet_request() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = authority.system_token.clone();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:external",
            "status": "pending",
            "intent": "publish_envelope",
            "capsule_id": "documents",
            "resource": "elastos://content/publish",
            "reason": "Publish document revision",
            "account_id": "wallet:eip155:20:0xabc",
            "address": "0xabc",
            "proof_type": "siwe",
            "connector_id": "wallet-metamask",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id.clone(),
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approval_token = intent_token_for_authority_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority,
        "wallet.approve",
        &json!({
            "request_id": "wallet-approval:external",
            "reason": "Looks correct",
        }),
    );
    let approved = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Aexternal/approve")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"reason":"Looks correct","home_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("approval method"));
}
