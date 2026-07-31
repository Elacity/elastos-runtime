use super::super::*;

#[tokio::test]
async fn test_wallet_send_signs_and_broadcasts_managed_evm_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state.clone());

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    let account_id = account["account_id"].as_str().unwrap().to_string();
    let send_intent = json!({
        "account_id": account_id,
        "chain_namespace": "eip155:20",
        "to": "0x2222222222222222222222222222222222222222",
        "amount": "0.000000000000000001",
    });
    let send_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.send",
        &send_intent,
    );
    let send_body = format!(
        r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","to":"0x2222222222222222222222222222222222222222","amount":"0.000000000000000001","step_up_token":"{}"}}"#,
        send_token
    );
    reset_mock_chain_broadcast_count("0x1234");

    let sent = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/send")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(send_body.clone()))
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
        "elastos.wallet.signed-transaction-result/v1"
    );
    assert_eq!(
        json["receipt"]["schema"],
        "elastos.chain.broadcast_receipt/v1"
    );
    assert_eq!(mock_chain_broadcast_count("0x1234"), 1);
    let effect_store = transaction_effect_store_for_test(
        &state,
        wallet_read_authority.verified_context().principal_id(),
    );
    let effects = effect_store["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 1);
    assert!(effects[0].get("signed_transaction").is_none());
    assert_eq!(effects[0]["wallet_binding"]["kind"], "managed_signed");

    let encoded_account_id = account_id.replace(':', "%3A");
    let delete_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.account.delete",
        &json!({ "account_id": account_id }),
    );
    let deleted = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("DELETE")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded_account_id}"
                ))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"step_up_token":"{delete_token}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);

    let replay = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/send")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(send_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let replay_status = replay.status();
    let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        replay_status,
        StatusCode::OK,
        "wallet send exact replay failed: {}",
        String::from_utf8_lossy(&replay_body)
    );
    let replay_json: serde_json::Value = serde_json::from_slice(&replay_body).unwrap();
    assert_eq!(replay_json["request_id"], json["request_id"]);
    assert_eq!(replay_json["transaction_hash"], json["transaction_hash"]);
    assert_eq!(replay_json["completion_status"], "complete");
    assert_eq!(mock_chain_broadcast_count("0x1234"), 1);

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::RevokeAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
    wallet_provider
        .assert_v2_approval_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::RequestApproval,
                WalletOperationKind::ListApprovals,
                WalletOperationKind::ApproveAndSignManaged,
                WalletOperationKind::AttachValidatedChainOutcome,
                WalletOperationKind::ListApprovals,
            ],
        )
        .await;

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

#[tokio::test]
async fn test_wallet_send_recovered_step_up_rejects_missing_transaction_effect() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let (state, _) = wallet_chain_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);
    let account_id = "wallet-account:missing";
    let send_intent = json!({
        "account_id": account_id,
        "chain_namespace": "eip155:20",
        "to": "0x2222222222222222222222222222222222222222",
        "amount": "0.000000000000000001",
    });
    let send_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.send",
        &send_intent,
    );
    let send_body = format!(
        r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","to":"0x2222222222222222222222222222222222222222","amount":"0.000000000000000001","step_up_token":"{send_token}"}}"#
    );

    let first = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/send")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(send_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::BAD_REQUEST);

    let recovered = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/send")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(send_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = recovered.status();
    let body = axum::body::to_bytes(recovered.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("no durable Runtime effect"));
}
