use super::*;

/// Gateway state whose registry has the inspect provider registered, backed by
/// a catalog source pointing at a seeded installed-capsule directory.
async fn inspect_test_state(dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(dir);
    let registry = Arc::new(ProviderRegistry::new());

    // Seed an installed capsule manifest the catalog source will read from
    // <data_dir>/capsules/<name>/capsule.json.
    let capsule_dir = dir.join("capsules").join("probe-capsule");
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec(&json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "probe-capsule",
            "role": "app",
            "type": "wasm",
            "entrypoint": "probe.wasm",
            "capabilities": ["elastos://storage/probe"]
        }))
        .unwrap(),
    )
    .unwrap();

    let source: Arc<dyn crate::inspect_provider::InspectSource> =
        Arc::new(crate::inspect_provider::CatalogInspectSource::new(
            dir.join("capsules"),
            Arc::downgrade(&registry),
        ));
    registry
        .register(Arc::new(crate::inspect_provider::InspectProvider::new(
            source,
        )))
        .await;

    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.to_path_buf(),
        data_dir: dir.to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
    }
}

#[tokio::test]
async fn inspect_capsules_requires_token_and_lists_installed_capsule() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);

    // No home launch token: rejected before reaching the provider.
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/capsules")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // A System-operator token reaches the inspect provider end-to-end.
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let ok = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/capsules")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    // The installed capsule made it through the full browser leg.
    assert!(
        text.contains("probe-capsule"),
        "inspect/capsules did not list the installed capsule: {text}"
    );
}

#[tokio::test]
async fn inspect_self_returns_own_record_and_ignores_client_id() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);

    // A browser/app token whose authenticated principal IS the caller's own
    // capsule id (the catalog ids entries as "capsule:<name>").
    let mut ctx = local_home_launch_token_context(dir.path()).unwrap();
    ctx.principal_id = "capsule:probe-capsule".to_string();
    let token = issue_home_launch_token_with_context(dir.path(), BROWSER_CAPSULE_ID, &ctx).unwrap();

    // Positive: a SelfOnly caller reads ITS OWN record.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/self")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("capsule:probe-capsule"),
        "self did not return the caller's own record: {text}"
    );

    // Negative (Principle 16): a client-supplied id for ANOTHER capsule is ignored
    // — the target is forced to the authenticated principal, so the response is
    // STILL the caller's own record (a redirect would have been not_found).
    let resp2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/self")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"id":"capsule:someone-else"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
        .await
        .unwrap();
    let text2 = String::from_utf8_lossy(&body2);
    assert!(
        text2.contains("capsule:probe-capsule"),
        "client-supplied id must be ignored; expected own record, got: {text2}"
    );
}

#[tokio::test]
async fn inspect_self_token_cannot_reach_system_capsule_op() {
    // Escalation blocked at the allow-list: a browser self token cannot reach the
    // System-scope `capsule` detail op.
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/capsule")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"id":"capsule:probe-capsule"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "a browser/self token must not reach the System capsule op"
    );
}

#[tokio::test]
async fn inspect_write_op_revoke_is_not_browser_reachable() {
    // Least-privilege at the edge (#16): the write op `revoke` is deliberately
    // absent from the browser allow-list, so it is unreachable through the
    // gateway proxy even WITH a System operator token — mutation stays on the
    // capability-gated carrier/admin path, never the browser.
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);

    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/revoke")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    "{\"token_id\":\"00000000000000000000000000000000\"}",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Not OK, and specifically not found — the op never enters the proxy.
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "revoke must not be browser-reachable"
    );
}

#[tokio::test]
async fn inspect_capsules_rejects_non_system_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);

    // A token for a different app must not be authorized for inspect.
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/capsules")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "non-System app must not inspect"
    );
}
