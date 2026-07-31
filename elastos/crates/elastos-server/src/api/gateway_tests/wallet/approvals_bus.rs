use super::super::*;

#[test]
fn approval_callers_use_wallet_bus_v2_while_deferred_paths_remain_explicit() {
    let migrated_sources = [
        include_str!("../../gateway_wallet_approvals.rs"),
        include_str!("../../gateway_inbox.rs"),
        include_str!("../../gateway_wallet_send.rs"),
        include_str!("../../gateway_browser_wallet.rs"),
        include_str!("../../gateway_wallet_app.rs"),
        include_str!("../../gateway_wallet_connectors.rs"),
        include_str!("../../gateway_home_system.rs"),
    ]
    .join("\n");
    for retired in [
        r#""op": "approval_requests""#,
        r#""op": "request_signature""#,
        r#""op": "reject_approval""#,
        r#""op": "approve_approval""#,
        r#""op": "sign_approved""#,
        r#""op": "complete_approval""#,
    ] {
        assert!(
            !migrated_sources.contains(retired),
            "migrated production caller still contains retired Wallet dispatch {retired}"
        );
    }
    assert!(!migrated_sources.contains(
        "browser_provider_resource_call(\n        \"wallet\",\n        \"request_signature\""
    ));

    let browser_source = include_str!("../../gateway_browser_wallet.rs");
    let send_source = include_str!("../../gateway_wallet_send.rs");
    let wallet_app_source = include_str!("../../gateway_wallet_app.rs");
    assert!(browser_source.contains(r#""op": "record_transaction_hash""#));
    assert!(send_source.contains(r#""op": "record_transaction_hash""#));
    assert!(wallet_app_source.contains(r#""op": "export_managed_secret""#));
    assert!(wallet_app_source.contains(r#""op": "import_managed_secret""#));
}

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

    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
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
