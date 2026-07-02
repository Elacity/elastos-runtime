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
        spend_policy: None,
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

#[tokio::test]
async fn discover_is_reachable_by_system_operator() {
    // discover is a System-scope op: a System-operator token reaches the handler
    // end-to-end. Over the probe-only fixture the goal is Unresolvable, which is a
    // NORMAL fail-closed answer (status ok) — proving the route admitted System and
    // the handler ran.
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/discover")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"intent":{"operation":"release"}}"#))
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
        text.contains("unresolvable"),
        "discover should run and report unresolvable over the probe-only set: {text}"
    );
}

#[tokio::test]
async fn discover_self_token_cannot_reach_discover() {
    // The cross-capsule capability map is a System surface: a browser/self token is
    // blocked at the allow-list (discover is NOT in the self arm).
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/discover")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"intent":{"operation":"release"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "a browser/self token must not reach the System discover op"
    );
}

#[tokio::test]
async fn discover_rejects_non_system_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/discover")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"intent":{"operation":"release"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "a non-System app must not reach discover"
    );
}

// ---------------------------------------------------------------------------
// Inspector approved-provider dispatch (request_act -> Inbox approval -> dispatch).
// Restored from the 0.5 source branch — the merge rewrote this file and dropped the
// approval-boundary tests while home-entropy-check still asserts them (Principle 12).
// ---------------------------------------------------------------------------

struct MockInspectActProvider {
    calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
}

#[async_trait::async_trait]
impl Provider for MockInspectActProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock inspect act provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-inspect-act"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.calls.lock().await.push(request.clone());
        if request
            .get("fail_dispatch")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return Err(ProviderError::Provider("mock dispatch failed".to_string()));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "schema": "elastos.inspect-act-test.result/v1",
                "op": request.get("op").cloned().unwrap_or(json!(null)),
                "source": request.pointer("/_runtime_invocation/source").cloned().unwrap_or(json!(null)),
                "target": request.pointer("/_runtime_invocation/target").cloned().unwrap_or(json!(null)),
            }
        }))
    }
}

/// Gateway state for the approved-dispatch tests: the catalog carries an installed
/// `exit-provider` capsule with authority metadata, the inspect provider is wired
/// `with_registry` (so `dispatch_approved` can route back through it), and a mock
/// `exit` provider captures every dispatched request.
async fn inspect_act_test_state(
    dir: &std::path::Path,
) -> (GatewayState, Arc<TokioMutex<Vec<serde_json::Value>>>) {
    seed_test_browser_capsules(dir);
    let capsule_dir = dir.join("capsules").join("exit-provider");
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec(&json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "exit-provider",
            "role": "provider",
            "type": "wasm",
            "entrypoint": "exit-provider.wasm",
            "provides": "elastos://exit/*",
            "authority": {
                "reason": "Runtime-owned Exit provider",
                "capabilities": [
                    { "resource": "elastos://exit/*", "actions": ["read"], "operations": ["status"] },
                    { "resource": "elastos://exit/*", "actions": ["execute"], "operations": ["open_stream"] }
                ],
                "audit_events": ["exit.open_stream.requested", "exit.open_stream.denied"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let registry = Arc::new(ProviderRegistry::new());
    let source: Arc<dyn crate::inspect_provider::InspectSource> =
        Arc::new(crate::inspect_provider::AggregateInspectSource::new(vec![
            Arc::new(crate::inspect_provider::CatalogInspectSource::new(
                dir.join("capsules"),
                Arc::downgrade(&registry),
            )),
            Arc::new(crate::inspect_provider::RegistryInspectSource::new(
                Arc::downgrade(&registry),
            )),
        ]));
    let provider: Arc<dyn Provider> = Arc::new(
        crate::inspect_provider::InspectProvider::with_registry(source, Arc::downgrade(&registry)),
    );
    registry.register(provider.clone()).await;
    registry
        .register_sub_provider("inspect", provider)
        .await
        .unwrap();

    let calls = Arc::new(TokioMutex::new(Vec::new()));
    let act_provider: Arc<dyn Provider> = Arc::new(MockInspectActProvider {
        calls: calls.clone(),
    });
    registry.register(act_provider.clone()).await;
    registry
        .register_sub_provider("exit", act_provider)
        .await
        .unwrap();

    let state = GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.to_path_buf(),
        data_dir: dir.to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
        spend_policy: None,
    };
    (state, calls)
}

#[tokio::test]
async fn inspect_action_requires_inbox_approval_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let authority = passkey_authority_with_name(dir.path(), Some("requester"));
    let system_token = authority.system_token.clone();
    let requested = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/request_act")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"probe":true}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::OK);
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    let request_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request_payload["status"], "pending");
    let request_id = request_payload["request_id"].as_str().unwrap();
    assert!(request_id.starts_with("inspect-act-"));
    assert!(calls.lock().await.is_empty());

    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let inbox = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entry = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "inspect_action_request")
        .expect("inspect action notification");
    assert_eq!(
        entry["action_ref"]["action_id"],
        format!("inspect-approve-request:{request_id}")
    );
    assert!(entry["body"]
        .as_str()
        .unwrap()
        .contains("Gate preview: Capability elastos://exit/*: read"));
    assert!(entry["body"]
        .as_str()
        .unwrap()
        .contains("Audit exit.open_stream.requested"));

    // A System launch token can CREATE the request but must not approve it.
    let system_approval_attempt = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(system_approval_attempt.status(), StatusCode::FORBIDDEN);
    assert!(calls.lock().await.is_empty());

    // Approval WITHOUT a fresh passkey home token fails closed and dispatches nothing.
    let missing_fresh_proof = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_fresh_proof.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(missing_fresh_proof.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("fresh passkey verification is required"));
    assert!(calls.lock().await.is_empty());

    // With the fresh same-principal passkey proof the approval dispatches exactly once.
    let approved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    authority.home_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let call_records = calls.lock().await;
    assert_eq!(call_records.len(), 1);
    assert_eq!(call_records[0]["op"], "status");
    assert_eq!(call_records[0]["probe"], true);
    assert_eq!(call_records[0]["_runtime_invocation"]["source"], "inspect");
    assert_eq!(call_records[0]["_runtime_invocation"]["target"], "exit");
    drop(call_records);

    // Replay of the approval is rejected; nothing dispatches twice.
    let approved_again = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    authority.home_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved_again.status(), StatusCode::BAD_REQUEST);
    assert_eq!(calls.lock().await.len(), 1);
}

#[tokio::test]
async fn inspect_action_rejects_stale_fresh_passkey_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("requester"));

    let requested = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/request_act")
                .header("x-elastos-home-token", authority.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"probe":true}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::OK);
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    let request_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = request_payload["request_id"].as_str().unwrap();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    // Revoke the auth session behind the passkey proof: the approval must fail closed.
    crate::auth::revoke_session_grant(dir.path(), &authority.session_id, crate::auth::now_ts())
        .unwrap();
    let rejected = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    authority.home_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = rejected.status();
    let body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "{message}");
    assert!(message.contains("auth session"));
    assert!(calls.lock().await.is_empty());
}

#[tokio::test]
async fn inspect_action_requests_are_principal_scoped_in_inbox_and_approval() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let requester = passkey_authority_with_name(dir.path(), Some("requester"));
    let other = passkey_authority_with_name_role(
        dir.path(),
        Some("other"),
        crate::auth::RuntimePrincipalRole::Guest,
    );

    let requested = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/request_act")
                .header("x-elastos-home-token", requester.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"probe":true}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::OK);
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    let request_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = request_payload["request_id"].as_str().unwrap();

    // Another principal's Inbox neither sees nor can approve/deny the request.
    let other_inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &other);
    let other_inbox = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", other_inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_inbox.status(), StatusCode::OK);
    let body = axum::body::to_bytes(other_inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let other_inbox_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let other_sees_request = other_inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "inspect_action_request"
                && entry["action_ref"]["action_id"]
                    == format!("inspect-approve-request:{request_id}")
        });
    assert!(!other_sees_request);

    for action_prefix in ["inspect-approve-request", "inspect-deny-request"] {
        let body = if action_prefix == "inspect-approve-request" {
            format!(
                r#"{{"action_id":"{action_prefix}:{request_id}","home_token":"{}"}}"#,
                other.home_token.as_str()
            )
        } else {
            format!(r#"{{"action_id":"{action_prefix}:{request_id}"}}"#)
        };
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/inbox/actions")
                    .header("x-elastos-home-token", other_inbox_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
            .await
            .unwrap();
        let message = String::from_utf8(body.to_vec()).unwrap();
        assert!(message.contains("different principal"));
    }
    assert!(calls.lock().await.is_empty());

    // The requesting principal sees and approves it.
    let requester_inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &requester);
    let requester_inbox = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", requester_inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(requester_inbox.status(), StatusCode::OK);
    let body = axum::body::to_bytes(requester_inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let requester_inbox_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let requester_sees_request = requester_inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "inspect_action_request"
                && entry["action_ref"]["action_id"]
                    == format!("inspect-approve-request:{request_id}")
        });
    assert!(requester_sees_request);

    let approved = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", requester_inbox_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    requester.home_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert_eq!(calls.lock().await.len(), 1);
}

#[tokio::test]
async fn inspect_action_can_be_denied_without_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let requested = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/request_act")
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"probe":true}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::OK);
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    let request_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = request_payload["request_id"].as_str().unwrap();

    let inbox_token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-deny-request:{request_id}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::OK);
    assert!(calls.lock().await.is_empty());
}

/// W5b browser-lane DEV MINT (sanctioned, run-gated): print a real, validator-accepted
/// SYSTEM home-launch token for an EXISTING runtime data dir (signed by that runtime's
/// own DID via `load_or_create_did`). It uses the validator's supported local path
/// (`proof_binding_id: None`, so no live auth-session is required) — exactly what the
/// gateway accepts at `require_home_launch_token_for_any_context`. This is NOT a product
/// path: the shipped binary mints SYSTEM tokens ONLY via the passkey/wallet auth-grant
/// flow (`issue_home_launch_token_for_auth_grant`); this `#[cfg(test)]` helper exists so
/// a local browser confirmation can drive the home-token gateway without WebAuthn.
/// Run:  ELASTOS_MINT_DATA_DIR=<data_dir> cargo test -p elastos-server \
///         mint_system_home_token_for_existing_data_dir -- --ignored --nocapture
#[tokio::test]
#[ignore = "dev mint: requires ELASTOS_MINT_DATA_DIR pointing at a provisioned runtime data dir"]
async fn mint_system_home_token_for_existing_data_dir() {
    let dir = std::env::var("ELASTOS_MINT_DATA_DIR")
        .expect("set ELASTOS_MINT_DATA_DIR to the runtime data dir to mint against");
    let token = issue_home_launch_token(std::path::Path::new(&dir), SYSTEM_CAPSULE_ID)
        .expect("mint SYSTEM home-launch token against the runtime DID");
    println!("ELASTOS_SYSTEM_TOKEN={token}");
}
