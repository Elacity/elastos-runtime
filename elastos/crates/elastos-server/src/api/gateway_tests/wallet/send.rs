use super::super::*;

#[tokio::test]
async fn test_wallet_send_signs_and_broadcasts_managed_evm_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_chain_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:20","label":"ELA Wallet","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account = json["accounts"]
        .as_array()
        .and_then(|accounts| {
            accounts.iter().find(|account| {
                account
                    .get("chain_namespace")
                    .and_then(|value| value.as_str())
                    == Some("eip155:20")
            })
        })
        .expect("created EVM account");
    assert_eq!(account["signing_available"], true);
    assert_eq!(account["signing_status"], "managed_key_available");
    let account_id = account["account_id"].as_str().unwrap();
    let send_intent = json!({
        "account_id": account_id,
        "chain_namespace": "eip155:20",
        "to": "0x2222222222222222222222222222222222222222",
        "amount": "0.000000000000000001",
    });
    let send_token = intent_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.send",
        &send_intent,
    );

    let sent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/send")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","to":"0x2222222222222222222222222222222222222222","amount":"0.000000000000000001","home_token":"{}"}}"#,
                    send_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sent_status = sent.status();
    let body = axum::body::to_bytes(sent.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        sent_status,
        StatusCode::OK,
        "wallet send failed: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["schema"], "elastos.wallet.send-transaction-result/v1");
    assert_eq!(
        json["transaction_hash"],
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        json["signed_result"]["schema"],
        "elastos.wallet.managed-transaction-result/v1"
    );
    assert_eq!(
        json["receipt"]["schema"],
        "elastos.chain.broadcast_receipt/v1"
    );

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let activity = summary_json["wallet_approvals"]["approval_requests"]
        .as_array()
        .unwrap();
    assert!(activity.iter().any(|request| {
        request["status"] == "completed"
            && request["capsule_id"] == WALLET_CAPSULE_ID
            && request["intent"] == "transaction_intent"
            && request["transaction_hash"]
                == "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            && request["completed_at"].as_u64().is_some()
    }));
    assert_eq!(summary_json["wallet_approvals"]["pending_count"], 0);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    assert!(auth_state.audit.iter().any(|event| {
        event.event_type == "wallet.transaction.requested" && event.result == "requested"
    }));
    assert!(auth_state.audit.iter().any(|event| {
        event.event_type == "wallet.approval.completed" && event.result == "completed"
    }));
    assert!(auth_state.audit.iter().any(|event| {
        event.event_type == "wallet.transaction.completed" && event.result == "completed"
    }));
}
