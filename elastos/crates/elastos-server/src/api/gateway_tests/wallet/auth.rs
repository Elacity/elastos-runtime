use super::super::*;

#[tokio::test]
async fn test_home_token_cannot_read_chain_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(chain_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), HOME_CAPSULE_ID).unwrap();

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/networks")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_evm_wallet_link_requires_passkey_authority_and_reuses_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[9u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let local_state = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": false },
                        "recent_targets": ["system", "../bad", "system", "chat-room"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(local_state.status(), StatusCode::OK);
    let local_state_body = axum::body::to_bytes(local_state.into_body(), usize::MAX)
        .await
        .unwrap();
    let local_state_json: serde_json::Value = serde_json::from_slice(&local_state_body).unwrap();
    assert_eq!(
        local_state_json["recent_targets"],
        json!(["system", "chat-room"])
    );

    let challenge = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token.clone())
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 8453
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(challenge.status(), StatusCode::OK);
    let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
    let message = challenge_json["message"].as_str().unwrap();
    assert!(message.contains("elastos.elacitylabs.com wants you to sign in"));
    assert!(message.contains("URI: https://elastos.elacitylabs.com/apps/home/"));
    assert!(message.contains("elastos://auth/challenge/"));
    assert!(message.contains("elastos://wallet/account/link"));
    assert!(message.contains(&format!("elastos://principal/{}", authority.principal_id)));

    let signature = evm_sign_message(&signing_key, message);
    let verified = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/verify")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "message": message,
                        "signature": signature,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let verified_cookies: Vec<_> = verified
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    assert!(
        verified_cookies.is_empty(),
        "wallet connector verification must not mint a Home session cookie: {verified_cookies:?}"
    );
    let verified_body = axum::body::to_bytes(verified.into_body(), usize::MAX)
        .await
        .unwrap();
    let verified_json: serde_json::Value = serde_json::from_slice(&verified_body).unwrap();
    assert_eq!(
        verified_json["proof_binding_id"].as_str().unwrap(),
        format!("proof:wallet:eip155:8453:{}", address.to_ascii_lowercase())
    );
    let session_id = verified_json["session_id"].as_str().unwrap();
    assert_eq!(session_id, authority.session_id);
    assert!(verified_json.get("home_token").is_none());
    assert!(verified_json.get("system_token").is_none());

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.clone())
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
    assert_eq!(summary_json["authority"]["wallet_connected"], false);
    assert_eq!(
        summary_json["authority"]["proof_binding_id"]
            .as_str()
            .unwrap(),
        authority.proof_binding_id.as_str()
    );
    assert_eq!(
        summary_json["browser_state"]["principal_id"]
            .as_str()
            .unwrap(),
        authority.principal_id.as_str()
    );
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    assert_eq!(
        summary_json["browser_state"]["localhost_root"]
            .as_str()
            .unwrap(),
        localhost_root.as_str()
    );
    assert_eq!(
        summary_json["browser_state"]["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(false)
    );
    let state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!("{localhost_root}/.AppData/ElastOS/Home/browser-state.json"),
    )
    .expect("principal-rooted Home state path");
    assert!(state_path.is_file(), "state path missing: {state_path:?}");
    let old_shared_path = dir.path().join("ElastOS").join("System").join("HomeState");
    assert!(
        !old_shared_path.exists(),
        "Home state must not use shared system bucket: {old_shared_path:?}"
    );

    let wallet_state = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": true },
                        "session": { "openWindows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wallet_state.status(), StatusCode::OK);
    let wallet_state_body = axum::body::to_bytes(wallet_state.into_body(), usize::MAX)
        .await
        .unwrap();
    let wallet_state_json: serde_json::Value = serde_json::from_slice(&wallet_state_body).unwrap();
    assert_eq!(
        wallet_state_json["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(true)
    );

    let launch_system = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"system"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch_system.status(), StatusCode::OK);
    let launch_body = axum::body::to_bytes(launch_system.into_body(), usize::MAX)
        .await
        .unwrap();
    let launch_json: serde_json::Value = serde_json::from_slice(&launch_body).unwrap();
    let system_token = launch_json["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();
    let system_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(system_summary.status(), StatusCode::OK);
    let system_body = axum::body::to_bytes(system_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let system_json: serde_json::Value = serde_json::from_slice(&system_body).unwrap();
    assert_eq!(
        system_json["authority"]["proof_binding_id"]
            .as_str()
            .unwrap(),
        authority.proof_binding_id.as_str()
    );
    assert_eq!(system_json["wallet_accounts"]["linked_count"], 1);
    assert_eq!(
        system_json["wallet_accounts"]["accounts"][0]["address"]
            .as_str()
            .unwrap(),
        address.to_ascii_lowercase()
    );
    assert_eq!(
        system_json["wallet_accounts"]["accounts"][0]["connector_id"]
            .as_str()
            .unwrap(),
        WALLET_METAMASK_CAPSULE_ID
    );

    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/auth/sessions/{session_id}/revoke"))
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);

    let standard = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(standard.status(), StatusCode::OK);
    let body = axum::body::to_bytes(standard.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], false);
}

#[tokio::test]
async fn test_auth_session_revoke_rejects_other_principal_home_and_system_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );

    for token in [guest.home_token.clone(), guest.system_token.clone()] {
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/auth/sessions/{}/revoke", admin.session_id))
                    .header("x-elastos-home-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        assert!(crate::auth::is_auth_session_active(
            dir.path(),
            &admin.session_id,
            crate::auth::now_ts()
        )
        .unwrap());
    }
}

#[tokio::test]
async fn test_auth_session_revoke_allows_admin_to_revoke_guest_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );

    let revoked = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/auth/sessions/{}/revoke", guest.session_id))
                .header("x-elastos-home-token", admin.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);
    assert!(!crate::auth::is_auth_session_active(
        dir.path(),
        &guest.session_id,
        crate::auth::now_ts()
    )
    .unwrap());
    assert!(crate::auth::is_auth_session_active(
        dir.path(),
        &admin.session_id,
        crate::auth::now_ts()
    )
    .unwrap());
}

#[tokio::test]
async fn test_metamask_connector_token_can_link_evm_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[11u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let display_address = evm_test_display_address(&address);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let challenge = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token.clone())
                .body(Body::from(
                    json!({
                        "address": display_address,
                        "chain_id": 8453
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(challenge.status(), StatusCode::OK);
    let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
    let message = challenge_json["message"].as_str().unwrap();
    assert!(message.contains(&format!("Ethereum account:\n{display_address}\n\n")));
    let signature = evm_sign_message(&signing_key, message);

    let verified = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/verify")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "message": message,
                        "signature": signature,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let body = axum::body::to_bytes(verified.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert_eq!(
        payload["proof_binding_id"].as_str().unwrap(),
        format!("proof:wallet:eip155:8453:{}", address.to_ascii_lowercase())
    );
    assert!(payload["app_token"].as_str().unwrap().len() > 40);
    assert!(payload.get("home_token").is_none());
    assert!(payload.get("system_token").is_none());
}

#[tokio::test]
async fn test_metamask_can_link_multiple_accounts_and_wallet_can_remove_one() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);
    let mut expected_addresses = Vec::new();

    for seed in [21u8, 22u8] {
        let signing_key = EvmSigningKey::from_bytes((&[seed; 32]).into()).unwrap();
        let address = evm_test_address(&signing_key);
        let display_address = evm_test_display_address(&address);
        expected_addresses.push(address.to_ascii_lowercase());

        let challenge = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/evm/challenge")
                    .header(CONTENT_TYPE, "application/json")
                    .header("host", "elastos.elacitylabs.com")
                    .header("x-elastos-home-token", connector_token.clone())
                    .body(Body::from(
                        json!({
                            "address": display_address,
                            "chain_id": 8453
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(challenge.status(), StatusCode::OK);
        let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
            .await
            .unwrap();
        let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
        let message = challenge_json["message"].as_str().unwrap();
        let signature = evm_sign_message(&signing_key, message);

        let verified = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/evm/verify")
                    .header(CONTENT_TYPE, "application/json")
                    .header("host", "elastos.elacitylabs.com")
                    .header("x-elastos-home-token", connector_token.clone())
                    .body(Body::from(
                        json!({
                            "message": message,
                            "signature": signature,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(verified.status(), StatusCode::OK);
    }

    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", wallet_token.clone())
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
    let accounts = summary_json["wallet_accounts"]["accounts"]
        .as_array()
        .unwrap();
    let metamask_accounts = accounts
        .iter()
        .filter(|account| account["connector_id"] == WALLET_METAMASK_CAPSULE_ID)
        .collect::<Vec<_>>();
    assert_eq!(metamask_accounts.len(), 2);

    let removed_id = metamask_accounts[0]["account_id"].as_str().unwrap();
    let encoded = removed_id.replace(':', "%3A");
    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"home_token":"{}"}}"#,
                    authority.home_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);

    let refreshed = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", wallet_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    let body = axum::body::to_bytes(refreshed.into_body(), usize::MAX)
        .await
        .unwrap();
    let refreshed_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let remaining = refreshed_json["wallet_accounts"]["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|account| account["connector_id"] == WALLET_METAMASK_CAPSULE_ID)
        .collect::<Vec<_>>();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0]["address"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase(),
        expected_addresses[1]
    );
}

#[tokio::test]
async fn test_metamask_connector_token_can_link_erc1271_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let contract = "0x00000000000000000000000000000000000000cc";
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let challenge = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token.clone())
                .body(Body::from(
                    json!({
                        "address": contract,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(challenge.status(), StatusCode::OK);
    let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
    let message = challenge_json["message"].as_str().unwrap();

    let verified = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/verify")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "message": message,
                        "signature": "0x01020304",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let body = axum::body::to_bytes(verified.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert_eq!(
        payload["proof_binding_id"].as_str().unwrap(),
        "proof:wallet:eip155:20:0x00000000000000000000000000000000000000cc"
    );
    assert!(payload["app_token"].as_str().unwrap().len() > 40);
    assert!(payload.get("home_token").is_none());
    assert!(payload.get("system_token").is_none());
}

#[tokio::test]
async fn test_evm_auth_challenge_uses_http_for_loopback_home() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[12u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "127.0.0.1:8090")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let message = json["message"].as_str().unwrap();
    assert!(message.contains("127.0.0.1:8090 wants you to sign in"));
    assert!(message.contains("URI: http://127.0.0.1:8090/apps/home/"));
}

#[tokio::test]
async fn test_evm_wallet_link_rejects_system_token_without_connector() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[15u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let authority = passkey_authority(dir.path());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .header("x-elastos-home-token", authority.system_token)
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_btc_wallet_link_rejects_system_token_without_connector() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority(dir.path());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/btc/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .header("x-elastos-home-token", authority.system_token)
                .body(Body::from(
                    json!({
                        "address": "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                        "network": "bitcoin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_wallet_token_cannot_link_bip322_account() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority(dir.path());
    let wallet_token =
        launch_token_for_authority_context(dir.path(), WALLET_CAPSULE_ID, &authority);
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/btc/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", wallet_token.clone())
                .body(Body::from(
                    json!({
                        "address": address,
                        "network": "bitcoin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains("home launch token is not authorized for this provider"),
        "response: {text}"
    );
}

#[tokio::test]
async fn test_unisat_token_can_link_bip322_account_without_minting_home_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_UNISAT_CAPSULE_ID, &authority);
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";

    let challenge = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/btc/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token.clone())
                .body(Body::from(
                    json!({
                        "address": address,
                        "network": "bitcoin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(challenge.status(), StatusCode::OK);
    let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
    let message = challenge_json["message"].as_str().unwrap();

    let verified = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/btc/verify")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.elacitylabs.com")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "message": message,
                        "signature": "mock-bip322-signature",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let verified_cookies: Vec<_> = verified
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    assert!(verified_cookies.is_empty());
    let verified_body = axum::body::to_bytes(verified.into_body(), usize::MAX)
        .await
        .unwrap();
    let verified_json: serde_json::Value = serde_json::from_slice(&verified_body).unwrap();
    assert_eq!(verified_json["principal_id"], authority.principal_id);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet-unisat/wallet/accounts")
                .header(
                    "x-elastos-home-token",
                    verified_json["app_token"].as_str().unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let accounts_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(accounts_json["linked_count"], 1);
    assert_eq!(
        accounts_json["accounts"][0]["connector_id"],
        "wallet-unisat"
    );
}

#[tokio::test]
async fn test_evm_auth_challenge_is_single_use() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[10u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let challenge = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .header("x-elastos-home-token", connector_token.clone())
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let challenge_body = axum::body::to_bytes(challenge.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge_json: serde_json::Value = serde_json::from_slice(&challenge_body).unwrap();
    let message = challenge_json["message"].as_str().unwrap();
    let signature = evm_sign_message(&signing_key, message);

    for expected_status in [StatusCode::OK, StatusCode::FORBIDDEN] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/evm/verify")
                    .header(CONTENT_TYPE, "application/json")
                    .header("x-elastos-home-token", connector_token.clone())
                    .body(Body::from(
                        json!({
                            "message": message,
                            "signature": signature,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
    }
}

#[tokio::test]
async fn test_evm_auth_challenge_rejects_client_supplied_origin_fields() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let signing_key = EvmSigningKey::from_bytes((&[11u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20,
                        "domain": "evil.example"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_evm_auth_challenge_requires_wallet_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let signing_key = EvmSigningKey::from_bytes((&[13u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);
    let authority = passkey_authority(dir.path());
    let connector_token =
        launch_token_for_authority_context(dir.path(), WALLET_METAMASK_CAPSULE_ID, &authority);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .header("x-elastos-home-token", connector_token)
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("wallet provider unavailable"));
}

#[tokio::test]
async fn test_evm_auth_challenge_requires_passkey_front_door() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let signing_key = EvmSigningKey::from_bytes((&[14u8; 32]).into()).unwrap();
    let address = evm_test_address(&signing_key);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/evm/challenge")
                .header(CONTENT_TYPE, "application/json")
                .header("host", "elastos.local")
                .body(Body::from(
                    json!({
                        "address": address,
                        "chain_id": 20
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("missing home launch token"));
}
