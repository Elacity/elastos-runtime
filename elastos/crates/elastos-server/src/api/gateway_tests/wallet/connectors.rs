use super::super::*;

#[tokio::test]
async fn test_wallet_connector_route_requires_connector_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let system_token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-metamask/wallet/approvals")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_wallet_connector_route_rejects_unknown_connector_capsule() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), "wallet-unknown", &context).unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-unknown/wallet/accounts")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("unknown wallet connector capsule"));
}

#[tokio::test]
async fn test_walletconnect_connector_requires_pinned_config() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_WALLETCONNECT_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-walletconnect/wallet/accounts")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains("WalletConnect connector is not configured"),
        "WalletConnect must remain unavailable until the SDK/config are pinned and tested: {text}"
    );
}

#[tokio::test]
async fn test_walletconnect_connector_accepts_pinned_config() {
    let dir = tempfile::tempdir().unwrap();
    seed_walletconnect_connector_config(dir.path());
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_WALLETCONNECT_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-walletconnect/wallet/accounts")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
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
    assert_eq!(json["linked_count"], 0);
}

#[tokio::test]
async fn test_walletconnect_connector_config_returns_pinned_sdk_contract() {
    let dir = tempfile::tempdir().unwrap();
    seed_walletconnect_connector_config(dir.path());
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_WALLETCONNECT_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-walletconnect/wallet/config")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connector_id"], WALLET_WALLETCONNECT_CAPSULE_ID);
    assert_eq!(
        json["walletconnect"]["project_id"],
        "test_walletconnect_project"
    );
    assert_eq!(
        json["walletconnect"]["sdk_asset_path"],
        "/apps/wallet-walletconnect/vendor/reown-appkit.js"
    );
    assert_eq!(
        json["walletconnect"]["sdk_package"],
        WALLETCONNECT_SDK_PACKAGE
    );
    assert_eq!(json["evm_chains"][0]["chainId"], "0x14");
    assert_eq!(json["evm_chains"][0]["chainName"], "Elastos Smart Chain");
    assert_eq!(json["evm_chains"][1]["chainId"], "0x2105");
    assert_eq!(json["evm_chains"][1]["chainName"], "Base");
}

#[tokio::test]
async fn test_wallet_summary_reports_walletconnect_available_only_when_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_CAPSULE_ID, &context).unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let without_config = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(without_config.status(), StatusCode::OK);
    let body = axum::body::to_bytes(without_config.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["approval_methods"]["walletconnect"]["available"],
        false
    );
    assert_eq!(
        json["approval_methods"]["walletconnect"]["requires_pinned_config"],
        true
    );

    seed_walletconnect_connector_config(dir.path());

    let with_config = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_config.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_config.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["approval_methods"]["walletconnect"]["available"], true);
    assert_eq!(
        json["approval_methods"]["walletconnect"]["requires_pinned_config"],
        false
    );
}

#[tokio::test]
async fn test_metamask_connector_config_returns_runtime_evm_chain_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider::default();
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-metamask/wallet/config")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connector_id"], WALLET_METAMASK_CAPSULE_ID);
    assert_eq!(json["evm_chains"][0]["chainId"], "0x14");
    assert_eq!(json["evm_chains"][1]["chainId"], "0x2105");
    assert!(json.get("walletconnect").is_none());
}

fn seed_walletconnect_connector_config(data_dir: &std::path::Path) {
    let sdk = b"// pinned test WalletConnect SDK asset\n";
    let sdk_path = data_dir.join(WALLETCONNECT_SDK_PATH);
    std::fs::create_dir_all(sdk_path.parent().unwrap()).unwrap();
    std::fs::write(&sdk_path, sdk).unwrap();
    let sdk_sha256 = format!("{:x}", Sha256::digest(sdk));

    let config_path = data_dir.join(WALLETCONNECT_CONFIG_PATH);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        config_path,
        serde_json::to_vec(&json!({
            "schema": WALLETCONNECT_CONFIG_SCHEMA,
            "project_id": "test_walletconnect_project",
            "sdk_package": WALLETCONNECT_SDK_PACKAGE,
            "sdk_version": "1.0.0",
            "sdk_sha256": sdk_sha256
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn test_metamask_connector_lists_external_wallet_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![
            json!({
                "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
                "principal_id": context.principal_id.clone(),
                "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
                "chain_namespace": "eip155:20",
                "address": "0x1111111111111111111111111111111111111111",
                "proof_type": "managed_evm",
                "linked_at": 10
            }),
            json!({
                "account_id": "wallet:eip155:8453:0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C",
                "principal_id": context.principal_id.clone(),
                "proof_binding_id": "proof:wallet:siwe:eip155:8453:0xa4c02db8653dd0ca18a8736d693b0db85c5b246c",
                "chain_namespace": "eip155:8453",
                "address": "0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C",
                "proof_type": "siwe",
                "connector_id": "wallet-metamask",
                "linked_at": 20
            }),
            json!({
                "account_id": "wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "principal_id": context.principal_id,
                "proof_binding_id": "proof:wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "chain_namespace": "bip122:000000000019d6689c085ae165831e93",
                "address": "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "proof_type": "bip322_simple",
                "connector_id": "wallet",
                "linked_at": 30
            }),
        ]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-metamask/wallet/accounts")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
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
    assert_eq!(json["linked_count"], 1);
    assert_eq!(
        json["accounts"][0]["address"],
        "0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C"
    );
    assert_eq!(json["accounts"][0]["proof_type"], "siwe");
    assert_eq!(json["accounts"][0]["connector_id"], "wallet-metamask");
}

#[tokio::test]
async fn test_wallet_connector_approvals_are_scoped_to_connector() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![
            json!({
                "request_id": "wallet-approval:metamask",
                "principal_id": context.principal_id.clone(),
                "status": "pending",
                "intent": "transaction_intent",
                "capsule_id": "documents",
                "resource": "elastos://wallet/eip155:8453/sign/transaction_intent",
                "reason": "Sign transaction",
                "account_id": "wallet:eip155:8453:0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C",
                "address": "0xA4C02dB8653DD0cA18A8736D693B0dB85C5b246C",
                "proof_type": "siwe",
                "connector_id": "wallet-metamask",
                "created_at": 10,
                "expires_at": 100
            }),
            json!({
                "request_id": "wallet-approval:bitcoin",
                "principal_id": context.principal_id,
                "status": "pending",
                "intent": "bitcoin_bip322_proof",
                "capsule_id": "documents",
                "resource": "elastos://wallet/bip122:000000000019d6689c085ae165831e93/sign/bitcoin_bip322_proof",
                "reason": "Sign Bitcoin proof",
                "account_id": "wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "address": "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "proof_type": "bip322_simple",
                "connector_id": "wallet",
                "created_at": 11,
                "expires_at": 101
            }),
        ]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-metamask/wallet/approvals")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pending_count"], 1);
    assert_eq!(
        json["approval_requests"][0]["request_id"],
        "wallet-approval:metamask"
    );
    assert_eq!(
        json["approval_requests"][0]["connector_id"],
        "wallet-metamask"
    );
}

#[tokio::test]
async fn test_unisat_connector_lists_only_unisat_bitcoin_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_UNISAT_CAPSULE_ID, &context)
            .unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![
            json!({
                "account_id": "wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "principal_id": context.principal_id.clone(),
                "proof_binding_id": "proof:wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "chain_namespace": "bip122:000000000019d6689c085ae165831e93",
                "address": "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                "proof_type": "bip322_simple",
                "connector_id": "wallet-unisat",
                "linked_at": 30
            }),
            json!({
                "account_id": "wallet:bip122:000000000019d6689c085ae165831e93:bc1q0000000000000000000000000000000000000",
                "principal_id": context.principal_id,
                "proof_binding_id": "proof:wallet:bip122:000000000019d6689c085ae165831e93:bc1q0000000000000000000000000000000000000",
                "chain_namespace": "bip122:000000000019d6689c085ae165831e93",
                "address": "bc1q0000000000000000000000000000000000000",
                "proof_type": "bip322_simple",
                "connector_id": "wallet",
                "linked_at": 31
            }),
        ]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-unisat/wallet/accounts")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
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
    assert_eq!(json["accounts"][0]["connector_id"], "wallet-unisat");
    assert_eq!(
        json["accounts"][0]["address"],
        "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l"
    );
}

#[tokio::test]
async fn test_wallet_app_can_approve_wallet_scoped_external_request() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let principal_id = context.principal_id.clone();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_CAPSULE_ID, &context).unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:bitcoin",
            "principal_id": principal_id,
            "status": "pending",
            "intent": "bitcoin_bip322_proof",
            "capsule_id": "documents",
            "resource": "elastos://wallet/bip122:000000000019d6689c085ae165831e93/sign/bitcoin_bip322_proof",
            "reason": "Sign Bitcoin proof",
            "account_id": "wallet:bip122:000000000019d6689c085ae165831e93:bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
            "address": "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
            "proof_type": "bip322_simple",
            "connector_id": "wallet",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "created_at": 10,
            "expires_at": 100
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/approvals/wallet-approval%3Abitcoin/approve")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Looks correct"}"#))
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
    assert_eq!(json["note"], "Approved. Continue in Wallet.");
    assert_eq!(json["handoff"]["status"], "awaiting_wallet_signature");

    let completed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/approvals/wallet-approval%3Abitcoin/complete")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"payload_hash":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","signature":"mock-bip322-signature","signer":"bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::OK);
    let body = axum::body::to_bytes(completed.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pending_count"], 0);
    assert_eq!(json["note"], "Signed by Wallet.");
}

#[tokio::test]
async fn test_metamask_connector_approves_external_wallet_request_with_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let principal_id = context.principal_id.clone();
    let session_id = context.session_id.clone();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &context)
            .unwrap();
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
            "principal_id": principal_id,
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approved = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(
                    "/api/apps/wallet-metamask/wallet/approvals/wallet-approval%3Aexternal/approve",
                )
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Looks correct"}"#))
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
    assert_eq!(json["note"], "Approved. Continue in MetaMask.");
    assert_eq!(json["handoff"]["status"], "awaiting_wallet_signature");
    assert_eq!(
        json["handoff"]["payload_hash"],
        "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert!(json["handoff"]["message"]
        .as_str()
        .unwrap()
        .contains("ElastOS Wallet Approval"));

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.approved"
                && event.challenge_id.as_deref() == Some("wallet-approval:external")
        })
        .expect("external wallet approval audit event");
    assert_eq!(event.result, "approved");
    assert_eq!(event.principal_id.as_deref(), Some(principal_id.as_str()));
    assert_eq!(event.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(
        event.capsule_id.as_deref(),
        Some(WALLET_METAMASK_CAPSULE_ID)
    );
    assert!(!event.signature.as_deref().unwrap_or_default().is_empty());
    assert!(event
        .signer_did
        .as_deref()
        .unwrap_or_default()
        .starts_with("did:key:"));
}

#[tokio::test]
async fn test_metamask_connector_completes_external_wallet_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let principal_id = context.principal_id.clone();
    let session_id = context.session_id.clone();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &context)
            .unwrap();
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
            "principal_id": principal_id,
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(
                    "/api/apps/wallet-metamask/wallet/approvals/wallet-approval%3Aexternal/approve",
                )
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Looks correct"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);

    let completed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet-metamask/wallet/approvals/wallet-approval%3Aexternal/complete")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"payload_hash":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","signature":"0xsigned","signer":"0xabc"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::OK);
    let body = axum::body::to_bytes(completed.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pending_count"], 0);
    assert_eq!(json["note"], "Signed by MetaMask.");

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let approved_event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.approved"
                && event.challenge_id.as_deref() == Some("wallet-approval:external")
        })
        .expect("external wallet approval audit event");
    let completed_event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.approval.completed"
                && event.challenge_id.as_deref() == Some("wallet-approval:external")
        })
        .expect("external wallet completion audit event");
    assert_ne!(approved_event.event_id, completed_event.event_id);
    assert_eq!(completed_event.result, "completed");
    assert_eq!(
        completed_event.principal_id.as_deref(),
        Some(principal_id.as_str())
    );
    assert_eq!(
        completed_event.session_id.as_deref(),
        Some(session_id.as_str())
    );
    assert_eq!(
        completed_event.capsule_id.as_deref(),
        Some(WALLET_METAMASK_CAPSULE_ID)
    );
    assert!(!completed_event
        .signature
        .as_deref()
        .unwrap_or_default()
        .is_empty());
    assert!(completed_event
        .signer_did
        .as_deref()
        .unwrap_or_default()
        .starts_with("did:key:"));
}
