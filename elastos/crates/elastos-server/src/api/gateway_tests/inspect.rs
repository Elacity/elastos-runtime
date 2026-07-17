use super::*;

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

async fn inspect_test_state(dir: &std::path::Path) -> GatewayState {
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
    let act_capsule_dir = dir.join("capsules").join("act-provider");
    std::fs::create_dir_all(&act_capsule_dir).unwrap();
    std::fs::write(
        act_capsule_dir.join("capsule.json"),
        serde_json::to_vec(&json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "act-provider",
            "role": "provider",
            "type": "wasm",
            "entrypoint": "act-provider.wasm",
            "provides": "elastos://act-test/*",
            "authority": {
                "reason": "Mock provider for Inspector dispatch tests",
                "capabilities": [
                    { "resource": "elastos://act-test/*", "actions": ["read"], "operations": ["status"] }
                ],
                "audit_events": ["act-test.status"]
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

    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.to_path_buf(),
        data_dir: dir.to_path_buf(),
    }
}

async fn inspect_act_test_state(
    dir: &std::path::Path,
) -> (GatewayState, Arc<TokioMutex<Vec<serde_json::Value>>>) {
    let state = inspect_test_state(dir).await;
    let calls = Arc::new(TokioMutex::new(Vec::new()));
    let registry = state.provider_registry.as_ref().unwrap().clone();
    let act_provider: Arc<dyn Provider> = Arc::new(MockInspectActProvider {
        calls: calls.clone(),
    });
    registry.register(act_provider.clone()).await;
    registry
        .register_sub_provider("exit", act_provider)
        .await
        .unwrap();
    (state, calls)
}

#[tokio::test]
async fn inspect_gateway_is_system_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(inspect_test_state(dir.path()).await);

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

    let library_token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();
    let non_system = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/capsules")
                .header("x-elastos-home-token", library_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_system.status(), StatusCode::FORBIDDEN);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/plan")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"open_stream"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(
        payload["data"]["capabilities"][0]["resource"],
        "elastos://exit/*"
    );
    assert_eq!(payload["data"]["capabilities"][0]["actions"][0], "execute");
    assert_eq!(
        payload["data"]["execution"]["schema"],
        "elastos.inspect.execution-policy/v1"
    );
    assert_eq!(payload["data"]["execution"]["mode"], "preview_only");
    assert_eq!(payload["data"]["execution"]["can_dispatch"], false);
    assert_eq!(payload["data"]["execution"]["can_mutate"], false);

    let direct_dispatch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/dispatch_approved")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_dispatch.status(), StatusCode::NOT_FOUND);

    let revoke = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/revoke")
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NOT_FOUND);
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
    assert_eq!(
        request_payload["request_binding"]["schema"],
        "elastos.esp.request-binding/v1"
    );
    assert_eq!(request_payload["request_binding"]["preview"]["probe"], true);
    let request_hash = request_payload["request_binding"]["sha256"]
        .as_str()
        .unwrap();
    assert_eq!(request_hash.len(), 64);
    assert_eq!(request_payload["request_binding"]["request_id"], request_id);
    assert_eq!(
        request_payload["request_binding"]["principal"],
        authority.principal_id
    );
    assert_eq!(
        request_payload["request_binding"]["capsule"],
        "capsule:exit-provider"
    );
    assert_eq!(
        request_payload["request_binding"]["interface"],
        serde_json::Value::Null
    );
    assert_eq!(request_payload["request_binding"]["method"], "status");
    assert_eq!(
        request_payload["request_binding"]["resources"],
        json!(["elastos://exit/*"])
    );
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
        .contains(&request_hash[..12]));
    assert!(entry["body"]
        .as_str()
        .unwrap()
        .contains("Gate preview: Capability elastos://exit/*: read"));
    assert!(entry["body"]
        .as_str()
        .unwrap()
        .contains("Audit exit.open_stream.requested"));

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

    let approval_token = intent_token_for_app_context(
        dir.path(),
        INBOX_CAPSULE_ID,
        &inbox_token,
        "inspect.approve",
        &json!({ "request_id": request_id }),
    );
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
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let approval: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        approval["result"]["schema"],
        "elastos.inspect.action-result/v1"
    );
    assert_eq!(approval["result"]["status"], "completed");
    assert_eq!(approval["result"]["request_id"], request_id);
    assert_eq!(
        approval["result"]["request_binding"]["request_id"],
        request_id
    );
    assert_eq!(
        approval["result"]["dispatch_result"]["request_binding"],
        approval["result"]["request_binding"]
    );
    let call_records = calls.lock().await;
    assert_eq!(call_records.len(), 1);
    assert_eq!(call_records[0]["op"], "status");
    assert_eq!(call_records[0]["probe"], true);
    assert_eq!(
        call_records[0]["_runtime_invocation"]["schema"],
        "elastos.provider.invocation/v1"
    );
    assert_eq!(call_records[0]["_runtime_invocation"]["source"], "inspect");
    assert_eq!(call_records[0]["_runtime_invocation"]["target"], "exit");
    assert_eq!(call_records[0]["_runtime_invocation"]["op"], "status");
    assert_eq!(
        call_records[0]["_runtime_invocation"]["capability"],
        "provider:inspect->exit:status"
    );
    assert_eq!(call_records[0]["_runtime_invocation"]["transfer"], "json");
    drop(call_records);

    let after_approve_inbox = app
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
    assert_eq!(after_approve_inbox.status(), StatusCode::OK);
    let body = axum::body::to_bytes(after_approve_inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let still_pending = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "inspect_action_request"
                && entry["action_ref"]["action_id"]
                    == format!("inspect-approve-request:{request_id}")
        });
    assert!(!still_pending);

    let record_path = dir
        .path()
        .join("inspect-actions")
        .join(format!("{request_id}.json"));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["status"], "completed");
    assert_eq!(record["result"]["status"], "ok");
    assert_eq!(
        record["result"]["data"]["schema"],
        "elastos.inspect.dispatch-result/v1"
    );
    assert_eq!(
        record["result"]["data"]["execution"]["mode"],
        "approved_dispatch"
    );
    assert_eq!(
        record["result"]["data"]["execution"]["approval_surface"],
        "inbox"
    );
    assert_eq!(record["result"]["data"]["execution"]["can_dispatch"], true);
    assert_eq!(record["result"]["data"]["execution"]["can_mutate"], true);
    assert_eq!(record["result"]["data"]["target"], "exit");
    assert_eq!(record["result"]["data"]["operation"], "status");
    assert_eq!(
        record["result"]["data"]["provider_response"]["data"]["schema"],
        "elastos.inspect-act-test.result/v1"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["data"]["op"],
        "status"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["data"]["source"],
        "inspect"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["data"]["target"],
        "exit"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["_runtime_transfer"]["schema"],
        "elastos.provider.transfer/v1"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["_runtime_transfer"]["capability"],
        "provider:inspect->exit:status"
    );
    assert_eq!(
        record["result"]["data"]["provider_response"]["_runtime_transfer"]["transfer"],
        "json"
    );

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let audit = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "inspect.action.completed"
                && event.challenge_id.as_deref() == Some(request_id)
        })
        .expect("approved dispatch completion audit event");
    assert_eq!(audit.result, "completed");
    assert_eq!(audit.capsule_id.as_deref(), Some(INBOX_CAPSULE_ID));
    assert_eq!(audit.reason, "Approved Inspector action through Inbox");

    let approved_again = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    inbox_token.as_str()
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
    let inbox_token = launch_token_for_authority_context(dir.path(), INBOX_CAPSULE_ID, &authority);

    crate::auth::revoke_session_grant(dir.path(), &authority.session_id, crate::auth::now_ts())
        .unwrap();
    let rejected = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    inbox_token.as_str()
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
async fn inspect_action_audits_approved_dispatch_failure() {
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
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"fail_dispatch":true}}"#,
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
    let approval_token = intent_token_for_app_context(
        dir.path(),
        INBOX_CAPSULE_ID,
        &inbox_token,
        "inspect.approve",
        &json!({ "request_id": request_id }),
    );
    let failed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    approval_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(failed.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("mock dispatch failed"));

    let call_records = calls.lock().await;
    assert_eq!(call_records.len(), 1);
    assert_eq!(call_records[0]["fail_dispatch"], true);
    drop(call_records);

    let record_path = dir
        .path()
        .join("inspect-actions")
        .join(format!("{request_id}.json"));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["status"], "failed");
    assert_eq!(record["error"], "provider error: mock dispatch failed");
    assert_eq!(record["result"]["code"], "dispatch_failed");

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let audit = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "inspect.action.failed"
                && event.challenge_id.as_deref() == Some(request_id)
        })
        .expect("approved dispatch failure audit event");
    assert_eq!(audit.result, "failed");
    assert_eq!(audit.capsule_id.as_deref(), Some(INBOX_CAPSULE_ID));
    assert_eq!(
        audit.reason,
        "Approved Inspector action dispatch failed through Inbox"
    );
}

#[tokio::test]
async fn inspect_action_rejects_runtime_metadata_before_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/request_act")
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status","request":{"_runtime_invocation":{"source":"fake"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("must not predeclare runtime metadata"));
    assert!(calls.lock().await.is_empty());

    let inbox_token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let inbox = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
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
    let has_inspect_action = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "inspect_action_request");
    assert!(!has_inspect_action);
}

#[tokio::test]
async fn inspect_action_rejects_raw_carrier_route_metadata_before_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    for reserved in ["connect_ticket", "carrier_route", "carrier"] {
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/provider/inspect/request_act")
                    .header("x-elastos-home-token", system_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "id": "capsule:exit-provider",
                            "operation": "status",
                            "request": {
                                reserved: "capsule-supplied-route-secret"
                            }
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
            .await
            .unwrap();
        let message = String::from_utf8(body.to_vec()).unwrap();
        assert!(message.contains("must not predeclare Runtime metadata field"));
        assert!(message.contains(reserved));
    }
    assert!(calls.lock().await.is_empty());

    let inbox_token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let inbox = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
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
    let has_inspect_action = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "inspect_action_request");
    assert!(!has_inspect_action);
}

#[tokio::test]
async fn inspect_action_rejects_stale_authority_plan_before_dispatch() {
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
    assert_eq!(
        request_payload["plan"]["capabilities"][0]["actions"][0],
        "read"
    );

    std::fs::write(
        dir.path()
            .join("capsules")
            .join("exit-provider")
            .join("capsule.json"),
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
                    { "resource": "elastos://exit/*", "actions": ["execute"], "operations": ["status"] }
                ],
                "audit_events": ["exit.status.requested"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let stale = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    inbox_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(stale.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("authority plan changed"));
    assert!(calls.lock().await.is_empty());

    let inbox = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
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
    let still_pending = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "inspect_action_request"
                && entry["action_ref"]["action_id"]
                    == format!("inspect-approve-request:{request_id}")
        });
    assert!(!still_pending);
}

#[tokio::test]
async fn inspect_action_rejects_changed_request_binding_before_dispatch() {
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

    let record_path = dir
        .path()
        .join("inspect-actions")
        .join(format!("{request_id}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["request"] = json!({ "probe": false });
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let changed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    inbox_token.as_str()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(changed.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("request binding changed"));
    assert!(calls.lock().await.is_empty());

    let inbox = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
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
    let still_pending = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "inspect_action_request"
                && entry["action_ref"]["action_id"]
                    == format!("inspect-approve-request:{request_id}")
        });
    assert!(!still_pending);
}

#[tokio::test]
async fn esp_inspect_action_rejects_every_mutated_binding_field_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("requester"));
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    for (field, replacement) in [
        ("schema", json!("elastos.esp.request-binding/v999")),
        ("request_id", json!("inspect-act-other")),
        ("principal", json!("person:other")),
        ("capsule", json!("capsule:other-provider")),
        ("interface", json!("elastos.other")),
        ("method", json!("other_operation")),
        ("resources", json!(["elastos://other/*"])),
        ("sha256", json!("00".repeat(32))),
        ("bytes", json!(999)),
        ("truncated", json!(true)),
        ("preview", json!({ "probe": false })),
    ] {
        for action in ["approve", "deny"] {
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
            let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let request_id = payload["request_id"].as_str().unwrap();
            let record_path = dir
                .path()
                .join("inspect-actions")
                .join(format!("{request_id}.json"));
            let mut record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
            record["request_binding"][field] = replacement.clone();
            std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

            let action_body = if action == "approve" {
                format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    inbox_token
                )
            } else {
                format!(r#"{{"action_id":"inspect-deny-request:{request_id}"}}"#)
            };
            let rejected = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/apps/inbox/actions")
                        .header("x-elastos-home-token", inbox_token.clone())
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(action_body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                rejected.status(),
                StatusCode::BAD_REQUEST,
                "action={action} field={field}"
            );
            assert!(
                calls.lock().await.is_empty(),
                "action={action} field={field}"
            );
        }
    }
}

#[tokio::test]
async fn inspect_action_duplicate_requests_keep_distinct_pending_records() {
    let dir = tempfile::tempdir().unwrap();
    let (state, calls) = inspect_act_test_state(dir.path()).await;
    let app = gateway_router(state);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let mut request_ids = Vec::new();
    for _ in 0..2 {
        let response = app
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
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        request_ids.push(payload["request_id"].as_str().unwrap().to_string());
    }
    assert_ne!(request_ids[0], request_ids[1]);
    assert!(calls.lock().await.is_empty());

    let inbox_token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let inbox = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
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
    let inspect_actions = inbox_payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "inspect_action_request")
        .collect::<Vec<_>>();
    assert_eq!(inspect_actions.len(), 2);
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
                other_inbox_token.as_str()
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

    let approval_token = intent_token_for_app_context(
        dir.path(),
        INBOX_CAPSULE_ID,
        &requester_inbox_token,
        "inspect.approve",
        &json!({ "request_id": request_id }),
    );
    let approved = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", requester_inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"action_id":"inspect-approve-request:{request_id}","home_token":"{}"}}"#,
                    approval_token
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
    let body = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .unwrap();
    let denial: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        denial["result"]["schema"],
        "elastos.inspect.action-result/v1"
    );
    assert_eq!(denial["result"]["status"], "denied");
    assert_eq!(denial["result"]["request_id"], request_id);
    assert_eq!(denial["result"]["dispatch_result"], serde_json::Value::Null);
    assert!(calls.lock().await.is_empty());
}
