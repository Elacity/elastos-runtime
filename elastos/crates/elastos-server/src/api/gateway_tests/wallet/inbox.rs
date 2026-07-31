use super::super::*;

#[tokio::test]
async fn test_inbox_approves_wallet_requests_through_runtime_wallet_signing() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), INBOX_CAPSULE_ID, &token);
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
            "proof_type": "managed_evm",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id.clone(),
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let (state, wallet_provider) =
        wallet_test_state_with_recording_provider(dir.path(), provider).await;
    let app = gateway_router(state);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_json["notifications"]["attention_count"], 1);
    assert_eq!(
        summary_json["notifications"]["entries"][0]["action_ref"]["action_id"],
        "wallet-approve-request:wallet-approval:test"
    );
    assert_eq!(
        summary_json["notifications"]["entries"][0]["action_ref"]["app"],
        WALLET_CAPSULE_ID
    );
    assert_eq!(
        summary_json["notifications"]["entries"][0]["source_app"],
        "documents"
    );

    let missing_fresh_token = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"action_id":"wallet-approve-request:wallet-approval:test"}"#,
                ))
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

    let approval_token = step_up_token_for_app_context(
        dir.path(),
        INBOX_CAPSULE_ID,
        &token,
        "wallet.approve",
        &json!({
            "request_id": "wallet-approval:test",
            "reason": "Approved in Inbox",
        }),
    );
    let approved = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"wallet-approve-request:wallet-approval:test","step_up_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let approved_body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let approved_json: serde_json::Value = serde_json::from_slice(&approved_body).unwrap();
    assert_eq!(
        approved_json["message"].as_str().unwrap(),
        "Approved and signed by built-in wallet."
    );
    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.completed"
                && event.challenge_id.as_deref() == Some("wallet-approval:test")
        })
        .expect("wallet approval completion audit event");
    assert_eq!(event.capsule_id.as_deref(), Some(INBOX_CAPSULE_ID));
    wallet_provider
        .assert_v2_approval_operations(
            &wallet_authority,
            &[
                WalletOperationKind::ListApprovals,
                WalletOperationKind::ListApprovals,
                WalletOperationKind::ApproveAndSignManaged,
            ],
        )
        .await;
}

#[test]
fn test_capsule_capability_requests_render_as_inbox_notifications() {
    let mut notifications = HomeNotificationsSummary::default();
    append_runtime_capability_notifications(
        &mut notifications,
        vec![RuntimeCapabilityPendingRequest {
            request_id: "cap-1".to_string(),
            resource: "elastos://content/publish".to_string(),
            action: "execute".to_string(),
            requested_at: 42,
        }],
    );

    assert_eq!(notifications.unread_count, 1);
    assert_eq!(notifications.attention_count, 1);
    assert_eq!(notifications.entries[0].kind, "capability_request");
    assert_eq!(notifications.entries[0].source_app, SYSTEM_CAPSULE_ID);
    assert_eq!(
        notifications.entries[0]
            .action_ref
            .as_ref()
            .map(|action_ref| action_ref.action_id.as_str()),
        Some("capability-approve-request:cap-1")
    );
    assert!(notifications.entries[0]
        .body
        .contains("elastos://content/publish"));
}

#[tokio::test]
async fn test_wallet_approval_journey_creates_request_reviews_in_inbox_and_signs() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let system_token = authority.system_token.clone();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let documents_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &authority);
    let documents_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), DOCUMENTS_CAPSULE_ID, &documents_token);
    let home_token = authority.home_token.clone();
    let state = wallet_test_state(dir.path()).await;
    let app = gateway_router(state.clone());

    let managed = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/managed")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"chain_namespace":"eip155:20"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(managed.status(), StatusCode::OK);
    let managed_body = axum::body::to_bytes(managed.into_body(), usize::MAX)
        .await
        .unwrap();
    let managed_json: serde_json::Value = serde_json::from_slice(&managed_body).unwrap();
    assert_eq!(managed_json["linked_count"], 1);

    let account_id = managed_json["accounts"][0]["account_id"].as_str().unwrap();
    let default_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/default")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","intent":"capability_grant"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(default_response.status(), StatusCode::OK);

    let request_json = runtime_wallet_data(
        &state,
        &documents_wallet_authority,
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.to_string(),
            chain_namespace: "eip155:20".to_string(),
            intent: "capability_grant".to_string(),
            resource: "elastos://wallet/eip155:20/sign/capability_grant".to_string(),
            reason: "Documents publish approval".to_string(),
            payload: json!({
                "schema": "elastos.wallet.capability-request/v1",
                "requested_by": "documents",
            }),
            expires_at: crate::auth::now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .unwrap();
    let request_id = request_json["approval_request"]["request_id"]
        .as_str()
        .unwrap();

    let home = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    let home_body = axum::body::to_bytes(home.into_body(), usize::MAX)
        .await
        .unwrap();
    let home_json: serde_json::Value = serde_json::from_slice(&home_body).unwrap();
    assert_eq!(home_json["notifications"]["attention_count"], 1);
    assert_eq!(
        home_json["notifications"]["entries"][0]["action_ref"]["action_id"],
        format!("wallet-approve-request:{request_id}")
    );

    let inbox = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(inbox_json["notifications"]["attention_count"], 1);
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["action_ref"]["action_id"],
        format!("wallet-approve-request:{request_id}")
    );
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["source_app"],
        "documents"
    );

    let approval_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.approve",
        &json!({
            "request_id": request_id,
            "reason": "Approved in Wallet",
        }),
    );
    let approved = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/managed-approvals/{}/approve",
                    request_id.replace(':', "%3A")
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"reason":"Approved in Wallet","step_up_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let approved_body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let approved_json: serde_json::Value = serde_json::from_slice(&approved_body).unwrap();
    assert_eq!(
        approved_json["note"].as_str().unwrap(),
        "Approved and signed by built-in wallet."
    );

    let approvals = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/wallet/approvals")
                .header("x-elastos-home-token", system_token)
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
    assert_eq!(approvals_json["pending_count"], 0);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.completed"
                && event.challenge_id.as_deref() == Some(request_id)
        })
        .expect("wallet approval completion audit event");
    assert_eq!(event.result, "completed");
    assert_eq!(
        event.principal_id.as_deref(),
        Some(authority.principal_id.as_str())
    );
    assert!(!event.signature.as_deref().unwrap_or_default().is_empty());
    assert!(event
        .signer_did
        .as_deref()
        .unwrap_or_default()
        .starts_with("did:key:"));
}

#[tokio::test]
async fn test_btc_wallet_approval_journey_reviews_in_inbox_and_signs() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let system_token = authority.system_token.clone();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let documents_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &authority);
    let documents_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), DOCUMENTS_CAPSULE_ID, &documents_token);
    let home_token = authority.home_token.clone();
    let state = wallet_test_state(dir.path()).await;
    let app = gateway_router(state.clone());
    let btc_namespace = "bip122:000000000019d6689c085ae165831e93";

    let managed = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/managed")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"chain_namespace":"{btc_namespace}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(managed.status(), StatusCode::OK);
    let managed_body = axum::body::to_bytes(managed.into_body(), usize::MAX)
        .await
        .unwrap();
    let managed_json: serde_json::Value = serde_json::from_slice(&managed_body).unwrap();
    assert_eq!(managed_json["linked_count"], 1);
    let account = &managed_json["accounts"][0];
    assert_eq!(account["chain_namespace"], btc_namespace);
    assert_eq!(account["proof_type"], "managed_btc_p2wpkh");
    let account_id = account["account_id"].as_str().unwrap();
    let address = account["address"].as_str().unwrap();

    let default_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/default")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"{btc_namespace}","intent":"bitcoin_bip322_proof"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(default_response.status(), StatusCode::OK);

    let challenge = runtime_wallet_data(
        &state,
        &documents_wallet_authority,
        WalletProviderOperationV2::BitcoinChallenge {
            domain: "localhost".to_string(),
            uri: "https://localhost/apps/home/".to_string(),
            address: address.to_string(),
            network: elastos_wallet_contract::PublicNetwork::new("btc-mainnet").unwrap(),
            resources: vec![
                format!("elastos://principal/{}", authority.principal_id),
                "elastos://wallet/account/link".to_string(),
            ],
        },
    )
    .await
    .unwrap();
    let message = challenge["message"].as_str().unwrap();
    let request_json = runtime_wallet_data(
        &state,
        &documents_wallet_authority,
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.to_string(),
            chain_namespace: btc_namespace.to_string(),
            intent: "bitcoin_bip322_proof".to_string(),
            resource: "elastos://wallet/proof/bip322/sign".to_string(),
            reason: "Prove Bitcoin account ownership".to_string(),
            payload: json!({
                "schema": "elastos.wallet.bitcoin_bip322_request/v1",
                "wallet_intent": "bitcoin_bip322_proof",
                "network": "btc-mainnet",
                "address": address,
                "message": message,
            }),
            expires_at: crate::auth::now_ts().saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
        },
    )
    .await
    .unwrap();
    let request_id = request_json["approval_request"]["request_id"]
        .as_str()
        .unwrap();
    assert_eq!(
        request_json["approval_request"]["intent"],
        "bitcoin_bip322_proof"
    );
    assert_eq!(
        request_json["approval_request"]["proof_type"],
        "managed_btc_p2wpkh"
    );

    let home = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    let home_body = axum::body::to_bytes(home.into_body(), usize::MAX)
        .await
        .unwrap();
    let home_json: serde_json::Value = serde_json::from_slice(&home_body).unwrap();
    assert_eq!(home_json["notifications"]["attention_count"], 1);
    assert_eq!(
        home_json["notifications"]["entries"][0]["title"],
        "Bitcoin proof request"
    );
    assert_eq!(
        home_json["notifications"]["entries"][0]["action_ref"]["action_id"],
        format!("wallet-approve-request:{request_id}")
    );

    let inbox = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["title"],
        "Bitcoin proof request"
    );

    let approval_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.approve",
        &json!({
            "request_id": request_id,
            "reason": "Approved in Wallet",
        }),
    );
    let approved = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/managed-approvals/{}/approve",
                    request_id.replace(':', "%3A")
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"reason":"Approved in Wallet","step_up_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let approved_body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let approved_json: serde_json::Value = serde_json::from_slice(&approved_body).unwrap();
    assert_eq!(
        approved_json["note"].as_str().unwrap(),
        "Approved and signed by built-in wallet."
    );

    let approvals = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/wallet/approvals")
                .header("x-elastos-home-token", system_token)
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
    assert_eq!(approvals_json["pending_count"], 0);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.completed"
                && event.challenge_id.as_deref() == Some(request_id)
        })
        .expect("BTC wallet approval completion audit event");
    assert_eq!(event.result, "completed");
    assert_eq!(
        event.principal_id.as_deref(),
        Some(authority.principal_id.as_str())
    );
    assert!(!event.signature.as_deref().unwrap_or_default().is_empty());
    assert!(event
        .signer_did
        .as_deref()
        .unwrap_or_default()
        .starts_with("did:key:"));
}
