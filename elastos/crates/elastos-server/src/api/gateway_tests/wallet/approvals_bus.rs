use super::super::*;

#[tokio::test]
async fn approval_routes_reject_caller_supplied_authority_before_wallet_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Atest/reject")
                .header("x-elastos-home-token", authority.system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"reason":"No","principal_id":"attacker","session_id":"attacker","actor":"wallet-metamask","connector_id":"wallet-metamask","launch_id":"attacker","account_id":"attacker"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(wallet_provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn home_summary_queries_wallet_only_with_verified_home_authority() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let unsigned = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::FORBIDDEN);
    assert!(wallet_provider.requests.lock().await.is_empty());

    let request = test_browser_request("localhost:61180", "http://localhost:61180")
        .uri("/api/apps/home/summary")
        .header("x-elastos-home-token", authority.home_token.as_str())
        .body(Body::empty())
        .unwrap();
    let wallet_authority =
        require_home_runtime_wallet_authority(dir.path(), request.headers()).unwrap();
    let authenticated = app.oneshot(request).await.unwrap();
    assert_eq!(authenticated.status(), StatusCode::OK);
    wallet_provider
        .assert_v2_approval_operations(&wallet_authority, &[WalletOperationKind::ListApprovals])
        .await;
}
