use super::super::*;

#[tokio::test]
async fn test_system_token_can_review_and_reject_wallet_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:test",
            "status": "pending",
            "intent": "publish_envelope",
            "capsule_id": "documents",
            "resource": "elastos://content/publish",
            "reason": "Publish document revision",
            "account_id": "wallet:eip155:20:0xabc",
            "address": "0xabc",
            "principal_id": context.principal_id.clone(),
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approvals = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/wallet/approvals")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approvals.status(), StatusCode::OK);
    let approvals_body = axum::body::to_bytes(approvals.into_body(), usize::MAX)
        .await
        .unwrap();
    let approvals_json: serde_json::Value = serde_json::from_slice(&approvals_body).unwrap();
    assert_eq!(approvals_json["available"], true);
    assert_eq!(approvals_json["pending_count"], 1);
    assert_eq!(
        approvals_json["approval_requests"][0]["intent"],
        "publish_envelope"
    );

    let rejected = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Atest/reject")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Not now"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected_body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .unwrap();
    let rejected_json: serde_json::Value = serde_json::from_slice(&rejected_body).unwrap();
    assert_eq!(rejected_json["pending_count"], 0);
    assert!(rejected_json["approval_requests"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_system_can_create_managed_wallet_account() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/wallet/managed")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["available"], true);
    assert_eq!(json["linked_count"], 3);
    let accounts = json["accounts"].as_array().unwrap();
    let namespaces = accounts
        .iter()
        .map(|account| account["chain_namespace"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        namespaces,
        BTreeSet::from([
            "eip155:20",
            "eip155:8453",
            "bip122:000000000019d6689c085ae165831e93"
        ])
    );
    let evm_addresses = accounts
        .iter()
        .filter(|account| account["proof_type"] == "managed_evm")
        .map(|account| account["address"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(evm_addresses.len(), 1);
    let bitcoin = accounts
        .iter()
        .find(|account| account["proof_type"] == "managed_btc_p2wpkh")
        .expect("Bitcoin managed account");
    assert_eq!(
        bitcoin["chain_namespace"],
        "bip122:000000000019d6689c085ae165831e93"
    );
    assert!(bitcoin["address"].as_str().unwrap().starts_with("bc1q"));
}

#[tokio::test]
async fn test_wallet_app_can_create_and_summarize_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_CAPSULE_ID, &context).unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let extra = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Agent Budget","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(extra.status(), StatusCode::OK);

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
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["app"]["title"], "Wallet");
    assert_eq!(
        json["approval_methods"]["walletconnect"]["available"],
        false
    );
    assert_eq!(
        json["approval_methods"]["walletconnect"]["requires_pinned_config"],
        true
    );
    assert_eq!(json["wallet_accounts"]["linked_count"], 4);
    assert_eq!(json["wallet_approvals"]["pending_count"], 0);
    let accounts = json["wallet_accounts"]["accounts"].as_array().unwrap();
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "Spending"));
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "ELA Wallet"));
    assert!(accounts.iter().any(|account| account["label"] == "Savings"));
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "Agent Budget"));
}

#[tokio::test]
async fn test_wallet_transaction_default_also_drives_browser_connect_default() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:20","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let created_json: serde_json::Value = serde_json::from_slice(&created_body).unwrap();
    let account_id = created_json["accounts"][0]["account_id"].as_str().unwrap();

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/default")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","intent":"transaction_intent"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated_json: serde_json::Value = serde_json::from_slice(&updated_body).unwrap();
    let defaults = updated_json["default_accounts"].as_array().unwrap();
    assert!(defaults.iter().any(|item| {
        item["account_id"] == account_id && item["intent"] == "transaction_intent"
    }));
    assert!(defaults
        .iter()
        .any(|item| { item["account_id"] == account_id && item["intent"] == "browser_connect" }));
}

#[test]
fn system_wallet_account_summary_defaults_missing_signing_availability_to_false() {
    let summary = system_wallet_account_summary(&json!({
        "account_id": "wallet:eip155:20:0xd2feb944c17ebbe1048d8afdac997b3660d6375d",
        "chain_namespace": "eip155:20",
        "address": "0xd2feb944c17ebbe1048d8afdac997b3660d6375d",
        "proof_type": "managed_evm",
        "linked_at": 1770000000
    }))
    .unwrap();

    assert!(!summary.signing_available);
}

#[tokio::test]
async fn test_wallet_app_can_delete_managed_account() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Temporary","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Temporary")
        .and_then(|account| account["account_id"].as_str())
        .unwrap()
        .to_string();
    let encoded = account_id.replace(':', "%3A");

    let missing_fresh = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"home_token":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_fresh.status(), StatusCode::FORBIDDEN);

    let delete_token = intent_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.account.delete",
        &json!({ "account_id": account_id }),
    );
    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"home_token":"{}"}}"#,
                    delete_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = axum::body::to_bytes(deleted.into_body(), usize::MAX)
        .await
        .unwrap();
    let deleted_payload: serde_json::Value = serde_json::from_slice(&deleted_body).unwrap();
    assert_eq!(deleted_payload["linked_count"], 0);

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
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["wallet_accounts"]["linked_count"], 0);
}

#[tokio::test]
async fn test_wallet_app_can_rename_account() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Spending")
        .and_then(|account| account["account_id"].as_str())
        .unwrap()
        .to_string();
    let encoded = account_id.replace(':', "%3A");

    let renamed = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"label":"Savings"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed_body = axum::body::to_bytes(renamed.into_body(), usize::MAX)
        .await
        .unwrap();
    let renamed_payload: serde_json::Value = serde_json::from_slice(&renamed_body).unwrap();
    assert!(renamed_payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| account["label"] == "Savings"));
    assert!(!renamed_payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| account["label"] == "Spending"));
}

#[tokio::test]
async fn test_wallet_recovery_key_requires_fresh_passkey_home_token() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Spending")
        .and_then(|account| account["account_id"].as_str())
        .unwrap();
    let encoded = account_id.replace(':', "%3A");

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "home_token": home_app_token(dir.path()),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let fresh_authority = passkey_authority(dir.path());
    let export_token = intent_token_for_authority_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &fresh_authority,
        "wallet.recovery-key.export",
        &json!({ "account_id": account_id }),
    );
    let mismatched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "home_token": export_token.clone(),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::FORBIDDEN);

    let exported = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", export_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "home_token": export_token,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    let exported_body = axum::body::to_bytes(exported.into_body(), usize::MAX)
        .await
        .unwrap();
    let exported_payload: serde_json::Value = serde_json::from_slice(&exported_body).unwrap();
    assert_eq!(exported_payload["schema"], "elastos.wallet.recovery-key/v1");
    assert_eq!(exported_payload["secret_type"], "secp256k1_private_key_hex");
    assert_eq!(
        exported_payload["private_key_hex"].as_str().unwrap().len(),
        64
    );
}

#[tokio::test]
async fn test_wallet_recovery_key_import_requires_fresh_passkey_home_token() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(wallet_test_state(dir.path()).await);

    let recovery_key = json!({
        "schema": "elastos.wallet.recovery-key/v1",
        "account_id": "wallet:eip155:8453:0x1111111111111111111111111111111111111111",
        "chain_namespace": "eip155:8453",
        "address": "0x1111111111111111111111111111111111111111",
        "secret_type": "secp256k1_private_key_hex",
        "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111",
    });

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "home_token": home_app_token(dir.path()),
                        "recovery_key": recovery_key.clone(),
                        "label": "Recovered Base",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let import_intent = json!({
        "recovery_key": recovery_key,
        "label": "Recovered Base",
    });
    let import_token = intent_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.import",
        &import_intent,
    );
    let imported = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "home_token": import_token,
                        "recovery_key": import_intent["recovery_key"],
                        "label": import_intent["label"],
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let body = axum::body::to_bytes(imported.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| {
            account["label"] == "Recovered Base"
                && account["signing_status"] == "managed_key_available"
        }));
}
